use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    CoordinatorCandidate, CoordinatorCandidateConfig, CoordinatorCandidateDeclaration,
    CoordinatorCandidateResources, CoordinatorCandidateStatus, CoordinatorCandidateWillingness,
    CoordinatorPublicReachability, CoordinatorWriteCredential, NetworkId, NetworkRecord, NodeId,
    RelayAuthorizationAuditSnapshot, RelayGovernanceAuditSnapshot, RelayHealthSnapshot,
    RelayLifecycleState,
};
use fuckipv6_common::{
    CoordinatorConfig, Identity, NetworkAllocation, NetworkTicket, RelayEndpoint,
};
use fuckipv6_coordinator::{
    relay::server::{RelayPolicyAuthority, RelayServer},
    signaling::blacklist::CoordinatorConfigBlacklistStore,
    signaling::coordinator_candidates::CoordinatorCandidateStore,
    signaling::exit_contribution::ExitContributionRequestStore,
    signaling::invitations::InvitationStore,
    signaling::join_requests::JoinRequestStore,
    signaling::member_lifecycle::MemberLifecycleStore,
    signaling::server::{RelayEndpointDirectory, SignalingServer},
    signaling::session::SessionManager,
};
use std::io::ErrorKind as IoErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

use crate::config_manager::ConfigManager;
use crate::policy_transaction::PolicySnapshot;
use relay_policy::DurableRelayPolicyAuthority;

mod actor;
mod relay_policy;

pub use actor::{
    EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceSnapshot,
    EmbeddedServiceStartRequest,
};

/// Stable explanation for a failed Relay lifecycle transition. The health
/// snapshot is retained with the coordinator so a failed start remains
/// observable after the command returns and stale healthy evidence is removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Relay 生命周期操作失败 [{reason_code}]: {message}")]
pub struct RelayLifecycleError {
    pub reason_code: &'static str,
    pub message: String,
    pub health: Box<RelayHealthSnapshot>,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn classify_relay_start_error(error: &anyhow::Error) -> (&'static str, RelayLifecycleState) {
    let kind = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind);
    match kind {
        Some(IoErrorKind::Unsupported) => (
            "relay.start.platform_unsupported",
            RelayLifecycleState::Unsupported,
        ),
        Some(IoErrorKind::AddrInUse) => ("relay.start.address_in_use", RelayLifecycleState::Failed),
        Some(IoErrorKind::PermissionDenied) => {
            ("relay.start.permission_denied", RelayLifecycleState::Failed)
        }
        Some(
            IoErrorKind::NetworkUnreachable
            | IoErrorKind::HostUnreachable
            | IoErrorKind::AddrNotAvailable
            | IoErrorKind::ConnectionRefused
            | IoErrorKind::ConnectionAborted
            | IoErrorKind::ConnectionReset
            | IoErrorKind::NotConnected
            | IoErrorKind::TimedOut,
        ) => (
            "relay.start.network_unreachable",
            RelayLifecycleState::Failed,
        ),
        _ => ("relay.start.unknown", RelayLifecycleState::Failed),
    }
}

fn relay_start_failure(error: anyhow::Error) -> RelayLifecycleError {
    let (reason_code, lifecycle) = classify_relay_start_error(&error);
    RelayLifecycleError {
        reason_code,
        message: format!("{error:#}"),
        health: Box::new(RelayHealthSnapshot::failed(
            lifecycle,
            reason_code,
            now_unix_ms(),
        )),
    }
}

fn relay_stop_failure(error: anyhow::Error) -> RelayLifecycleError {
    const REASON_CODE: &str = "relay.stop.cleanup_incomplete";
    RelayLifecycleError {
        reason_code: REASON_CODE,
        message: format!("{error:#}"),
        health: Box::new(RelayHealthSnapshot::failed(
            RelayLifecycleState::Failed,
            REASON_CODE,
            now_unix_ms(),
        )),
    }
}

/// 内嵌协调端 Actor
///
/// 赋予普通客户端节点完整的中心协调与中继服务能力，
/// 使每个节点都能一键创建自己的对等网络。
pub struct EmbeddedCoordinator {
    pub ticket: NetworkTicket,
    pub signaling_addr: SocketAddr,
    pub relay_addr: Option<SocketAddr>,
    pub session_manager: Arc<SessionManager>,
    blacklist_store: Arc<CoordinatorConfigBlacklistStore>,
    coordinator_candidate_store: Option<Arc<CoordinatorCandidateStore>>,
    network_id: Option<NetworkId>,
    signaling_task: Option<JoinHandle<Result<()>>>,
    relay_directory: RelayEndpointDirectory,
    relay_server: Option<RelayServer>,
    relay_task: Option<JoinHandle<Result<()>>>,
    relay_health: Option<RelayHealthSnapshot>,
    relay_bind_port: u16,
    external_host: Option<String>,
    relay_policy_authority: Arc<DurableRelayPolicyAuthority>,
    relay_provider_node_id: NodeId,
}

impl EmbeddedCoordinator {
    /// 启动内嵌协调 Actor
    pub async fn start(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        host_public_key: Option<[u8; 32]>,
        external_host: Option<String>,
        ip_pool: Option<&str>,
    ) -> Result<Self> {
        let config_manager = ConfigManager::new().context("无法确定节点配置目录")?;
        let request = EmbeddedServiceStartRequest {
            network_name: network_name.to_string(),
            signaling_port,
            relay_port,
            relay_enabled: true,
            host_public_key,
            external_host,
            ip_pool: ip_pool.map(str::to_string),
        };
        Self::start_with_config_manager(&request, config_manager).await
    }

    /// Start an embedded coordinator using an explicitly isolated config
    /// directory. Intended for embedders and tests that must not read or write
    /// the user's default config directory.
    pub async fn start_with_config_dir(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        host_public_key: Option<[u8; 32]>,
        external_host: Option<String>,
        ip_pool: Option<&str>,
        config_dir: std::path::PathBuf,
    ) -> Result<Self> {
        let config_manager =
            ConfigManager::new_for_dir(config_dir).context("无法初始化内嵌协调端配置目录")?;
        let request = EmbeddedServiceStartRequest {
            network_name: network_name.to_string(),
            signaling_port,
            relay_port,
            relay_enabled: true,
            host_public_key,
            external_host,
            ip_pool: ip_pool.map(str::to_string),
        };
        Self::start_with_config_manager(&request, config_manager).await
    }

    pub(crate) async fn start_with_config_manager(
        request: &EmbeddedServiceStartRequest,
        config_manager: ConfigManager,
    ) -> Result<Self> {
        let network_name = request.network_name.as_str();
        let pool = request.ip_pool.as_deref().unwrap_or("fd00::1000/120");
        let blacklist_path = config_manager
            .coordinator_config_path(network_name)
            .context("无法确定协调端持久化路径")?;
        let network_record_path = config_manager
            .network_record_path(network_name)
            .context("无法确定审批授权网络资料路径")?;
        // Validate the owner-authored roster before any coordinator defaults are
        // created. A later review re-reads this file for every authorization,
        // but startup should still surface malformed authority immediately.
        // Missing records keep legacy signaling/Relay services available while
        // disabling candidate management fail-closed; production network
        // creation persists the authoritative record before starting services.
        let network_id = if network_record_path.exists() {
            let record: NetworkRecord = serde_json::from_slice(
                &std::fs::read(&network_record_path).with_context(|| {
                    format!(
                        "无法读取审批授权网络资料: {}",
                        network_record_path.display()
                    )
                })?,
            )
            .with_context(|| {
                format!(
                    "无法解析审批授权网络资料: {}",
                    network_record_path.display()
                )
            })?;
            record
                .validate()
                .context("审批授权网络资料校验失败，拒绝启动内嵌协调器")?;
            Some(record.network_id)
        } else {
            tracing::warn!(
                network = network_name,
                path = %network_record_path.display(),
                "审批授权网络资料不存在，内嵌 Coordinator 候选组能力保持禁用"
            );
            None
        };
        let node_identity = config_manager
            .load_node_identity()
            .context("无法加载内嵌协调器 owner 身份")?;
        let provider_node_id = NodeId::from_public_key(&node_identity.public_key());
        let owner_node_id = provider_node_id.to_string();
        let mut default_coordinator_config = if blacklist_path.exists() {
            CoordinatorConfig::from_file(&blacklist_path).context("无法加载内嵌协调端持久配置")?
        } else {
            let mut config = CoordinatorConfig::default();
            config.network.ipv6_pool = pool.to_string();
            config.network.ip_pool = Some(pool.to_string());
            config
        };
        default_coordinator_config.network_record_path = Some(network_record_path.clone());
        default_coordinator_config.owner_node_id = Some(owner_node_id.clone());
        let blacklist_store = CoordinatorConfigBlacklistStore::with_default_config(
            blacklist_path.clone(),
            default_coordinator_config.clone(),
        );
        // Authorization lookups deliberately fail closed when the persistent
        // coordinator config is absent. A newly created embedded network has
        // no config yet, so bootstrap it here before attaching the store. The
        // store will then assign a stable network id under its file lock.
        if !blacklist_path.exists() {
            if let Some(parent) = blacklist_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("无法创建内嵌协调端配置目录: {}", parent.display()))?;
            }
            default_coordinator_config
                .save_to_file(&blacklist_path)
                .context("无法初始化内嵌协调端持久配置")?;
        } else {
            default_coordinator_config
                .save_to_file(&blacklist_path)
                .context("无法更新内嵌协调端审批授权配置")?;
        }
        let persisted_allocation = blacklist_store
            .load_network_allocation()
            .context("无法加载持久化协调端地址分配")?;
        let join_request_store = Arc::new(
            JoinRequestStore::new(
                blacklist_path.with_file_name("join_requests.json"),
                Some(network_record_path.clone()),
                Some(owner_node_id.clone()),
                Vec::new(),
            )
            .context("无法初始化审批请求存储")?,
        );
        let member_lifecycle_store = Arc::new(
            MemberLifecycleStore::new(
                blacklist_path.with_file_name("member_lifecycle.json"),
                network_record_path.clone(),
            )
            .context("无法初始化成员生命周期存储")?,
        );
        let invitation_store = Arc::new(
            InvitationStore::new(
                blacklist_path.with_file_name("invitations.json"),
                Some(network_record_path.clone()),
                Some(owner_node_id.clone()),
                Vec::new(),
            )
            .context("无法初始化私有邀请存储")?
            .with_member_lifecycle_store(member_lifecycle_store.clone()),
        );
        let exit_contribution_store = Arc::new(ExitContributionRequestStore::new(
            blacklist_path.with_file_name("exit_contribution_requests.json"),
            network_record_path.clone(),
        ));
        let coordinator_candidate_store = network_id
            .as_ref()
            .map(|_| {
                CoordinatorCandidateStore::new(
                    blacklist_path.with_file_name("coordinator_candidates.json"),
                    network_record_path.clone(),
                )
                .map(Arc::new)
                .context("无法初始化 Coordinator 候选存储")
            })
            .transpose()?;
        let session_manager = Arc::new(
            SessionManager::new_with_allocation(&persisted_allocation)
                .context("无法初始化内嵌协调器地址池")?,
        );
        session_manager
            .attach_data_plane_membership(network_record_path.clone())
            .context("无法加载数据面成员名册")?;
        let blacklist_store = Arc::new(blacklist_store);
        session_manager
            .attach_blacklist_store(blacklist_store.clone())
            .await
            .context("无法加载节点持久黑名单")?;

        let signaling_bind = format!("0.0.0.0:{}", request.signaling_port);
        let mut signaling_server =
            SignalingServer::new_with_manager(signaling_bind, session_manager.clone())
                .with_join_request_store(join_request_store)
                .with_invitation_store(invitation_store)
                .with_member_lifecycle_store(member_lifecycle_store)
                .with_exit_contribution_store(exit_contribution_store);
        if let Some(store) = coordinator_candidate_store.clone() {
            signaling_server = signaling_server.with_coordinator_candidate_store(store);
        }
        let signaling_server = signaling_server
            .with_network_record_path(network_record_path)
            .with_node_identity(node_identity)
            .bind()
            .await
            .context("无法绑定内嵌信令服务")?;
        let actual_signaling_addr = signaling_server.local_addr()?;
        let relay_directory = signaling_server.relay_endpoint_directory();
        signaling_server.spawn_timeout_cleanup(120);

        // Runtime-owned services supply the node-wide persisted public key so
        // recreating a service keeps the invitation identity stable. Direct
        // embedders that omit it retain the old ephemeral-ticket behavior.
        let host_pubkey = match request.host_public_key {
            Some(public_key) => public_key,
            None => Identity::generate()?.public_key_bytes(),
        };

        // 确定票据暴露的接入地址
        let actual_signaling_port = actual_signaling_addr.port();
        let ticket_endpoint = if let Some(host) = request.external_host.as_deref() {
            if host.contains(':') {
                host.to_string()
            } else {
                format!("{host}:{actual_signaling_port}")
            }
        } else {
            // 本机探测默认地址
            format!("127.0.0.1:{actual_signaling_port}")
        };

        let mut ticket = NetworkTicket::new(host_pubkey, ticket_endpoint, network_name);
        if persisted_allocation.mode.is_ipv4_enabled() {
            ticket = ticket.with_ipv4(persisted_allocation.ipv4_pool.clone());
        }
        if persisted_allocation.mode.is_ipv6_enabled() {
            ticket = ticket.with_ipv6(persisted_allocation.effective_ipv6_pool());
        }

        let signaling_task = tokio::spawn(async move { signaling_server.run().await });

        let signaling_addr = actual_signaling_addr;
        let relay_policy_authority = Arc::new(DurableRelayPolicyAuthority::new(
            config_manager.clone(),
            network_name,
            PolicySnapshot::default(),
        ));
        let mut coordinator = Self {
            ticket,
            signaling_addr,
            relay_addr: None,
            session_manager,
            blacklist_store,
            coordinator_candidate_store,
            network_id,
            signaling_task: Some(signaling_task),
            relay_directory,
            relay_server: None,
            relay_task: None,
            relay_health: None,
            relay_bind_port: request.relay_port,
            external_host: request.external_host.clone(),
            relay_policy_authority,
            relay_provider_node_id: provider_node_id,
        };

        if request.relay_enabled {
            if let Err(error) = coordinator.enable_relay().await {
                let _ = coordinator.shutdown().await;
                return Err(error.into());
            }
        }

        tracing::info!(
            "🚀 内嵌协调 Actor 启动成功: 信令={}, QUIC中继={}",
            signaling_addr,
            coordinator
                .relay_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "已由节点 owner 禁用".to_string())
        );
        tracing::info!(
            "🎫 网络 [{}] 邀请票据已生成（敏感值不写入日志）",
            network_name
        );

        Ok(coordinator)
    }

    pub fn relay_addr(&self) -> Option<SocketAddr> {
        self.relay_addr
    }

    /// Return the Relay server's authoritative health projection. A disabled
    /// Relay has no health snapshot; callers must not infer health from the
    /// advertised address or the owner's enable intent.
    pub async fn relay_health(&self) -> Option<RelayHealthSnapshot> {
        match self.relay_server.as_ref() {
            Some(server) => Some(server.health_snapshot().await),
            None => self.relay_health.clone(),
        }
    }

    /// Return the owner-visible, sanitized authorization projection for the
    /// current Relay policy. A missing or invalid policy remains visible as a
    /// health failure and never produces a fabricated allow decision.
    pub fn relay_authorization_audit_snapshot(&self) -> Option<RelayAuthorizationAuditSnapshot> {
        self.relay_server.as_ref()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis()
            .try_into()
            .ok()?;
        match self
            .relay_policy_authority
            .current_policy()
            .and_then(|policy| policy.authorization_audit_snapshot(now).map_err(Into::into))
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                tracing::warn!("无法读取 Relay 授权诊断: {error:#}");
                None
            }
        }
    }

    /// Return the Relay's bounded, payload-free governance ledger. The ledger
    /// is owned by the current Relay instance and therefore disappears when the
    /// Relay is explicitly disabled; durable lifecycle evidence remains in the
    /// node command audit.
    pub fn relay_governance_snapshot(&self) -> Option<RelayGovernanceAuditSnapshot> {
        self.relay_server
            .as_ref()
            .map(RelayServer::governance_snapshot)
    }

    /// Start the local Relay independently of signaling and other node
    /// capabilities. Re-enabling after a stop restores the last bound port.
    pub async fn enable_relay(&mut self) -> Result<SocketAddr, RelayLifecycleError> {
        if let Some(addr) = self.relay_addr {
            return Ok(addr);
        }

        let relay_bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), self.relay_bind_port);
        let relay_server = match RelayServer::bind_with_generated_certificate(
            relay_bind,
            self.session_manager.clone(),
            self.relay_policy_authority.clone(),
        ) {
            Ok(server) => server,
            Err(error) => {
                let failure = relay_start_failure(error.context("无法启动内嵌 QUIC 中继服务"));
                self.relay_health = Some((*failure.health).clone());
                return Err(failure);
            }
        };
        let actual_relay_addr = relay_server.local_addr();
        let relay_pin = relay_server.certificate().public_key_pin;
        let advertised_relay_addr =
            match resolve_relay_advertisement(self.external_host.as_deref(), actual_relay_addr)
                .await
            {
                Ok(addr) => addr,
                Err(error) => {
                    let failure = relay_start_failure(error);
                    self.relay_health = Some((*failure.health).clone());
                    drop(relay_server);
                    return Err(failure);
                }
            };
        let relay_endpoint = RelayEndpoint {
            addr: advertised_relay_addr,
            server_name: "fuckipv6.local".to_string(),
            public_key_pin: relay_pin,
            provider_node_id: Some(self.relay_provider_node_id.clone()),
        };

        let run_server = relay_server.clone();
        let relay_task = tokio::spawn(async move { run_server.run().await });
        self.relay_directory.set_primary(Some(relay_endpoint));
        self.relay_server = Some(relay_server);
        self.relay_task = Some(relay_task);
        self.relay_addr = Some(actual_relay_addr);
        self.relay_health = None;
        self.relay_bind_port = actual_relay_addr.port();
        Ok(actual_relay_addr)
    }

    /// Withdraw this node's Relay endpoint, stop accepting new sessions, close
    /// existing QUIC sessions, and release the listener before returning.
    pub async fn disable_relay(&mut self) -> Result<(), RelayLifecycleError> {
        self.relay_directory.set_primary(None);
        let Some(server) = self.relay_server.take() else {
            self.relay_addr = None;
            return Ok(());
        };

        server.shutdown().await;
        let task_result = match self.relay_task.take() {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(anyhow::anyhow!("QUIC 中继任务异常退出: {error}")),
            },
            None => Ok(()),
        };
        drop(server);
        self.relay_addr = None;
        self.relay_health = None;
        match task_result {
            Ok(()) => Ok(()),
            Err(error) => {
                let failure = relay_stop_failure(error);
                self.relay_health = Some((*failure.health).clone());
                Err(failure)
            }
        }
    }

    pub async fn persisted_network_allocation(&self) -> Result<NetworkAllocation> {
        let store = self.blacklist_store.clone();
        tokio::task::spawn_blocking(move || store.load_network_allocation())
            .await
            .context("等待协调端地址分配读取任务失败")?
    }

    /// Persist the new allocation first, then hot-update the runtime allocator.
    /// If the runtime rejects the update, restore the previous persisted value
    /// and fail closed so callers never report a half-applied configuration.
    pub async fn update_network_allocation(&self, allocation: NetworkAllocation) -> Result<()> {
        let previous = self.session_manager.current_network_allocation().await;
        let store = self.blacklist_store.clone();
        let next = allocation.clone();
        tokio::task::spawn_blocking(move || store.save_network_allocation(&next))
            .await
            .context("等待协调端地址分配持久化任务失败")??;

        if let Err(runtime_error) = self
            .session_manager
            .update_network_allocation(allocation)
            .await
        {
            let store = self.blacklist_store.clone();
            let rollback =
                tokio::task::spawn_blocking(move || store.save_network_allocation(&previous))
                    .await
                    .context("等待协调端地址分配回滚任务失败")?;
            return match rollback {
                Ok(()) => Err(runtime_error).context("运行时拒绝地址分配，持久化配置已回滚"),
                Err(rollback_error) => Err(runtime_error).context(format!(
                    "运行时拒绝地址分配，且持久化配置回滚失败: {rollback_error:#}"
                )),
            };
        }
        Ok(())
    }

    /// Stop Relay independently, then stop signaling. Both listener ports are
    /// released before the caller treats the embedded service as stopped.
    pub(crate) async fn coordinator_candidate_status(
        &self,
        actor: NodeId,
    ) -> Result<CoordinatorCandidateStatus> {
        let store = self
            .coordinator_candidate_store
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        let network_id = self
            .network_id
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        tokio::task::spawn_blocking(move || store.status(&actor, &network_id, now_unix_ms()))
            .await
            .context("Coordinator 候选状态查询任务失败")?
    }

    pub(crate) async fn update_coordinator_candidate_config(
        &self,
        identity: &Identity,
        expected_revision: u64,
        candidates: Vec<CoordinatorCandidate>,
        effective_from_unix_ms: u64,
        expires_at_unix_ms: u64,
        credential: Option<CoordinatorWriteCredential>,
    ) -> Result<CoordinatorCandidateStatus> {
        let actor = NodeId::from_public_key(&identity.public_key());
        let network_id = self
            .network_id
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        let config = CoordinatorCandidateConfig::signed(
            network_id,
            expected_revision
                .checked_add(1)
                .context("Coordinator 候选配置 revision 溢出")?,
            candidates,
            effective_from_unix_ms,
            expires_at_unix_ms,
            identity,
        )?;
        let store = self
            .coordinator_candidate_store
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        tokio::task::spawn_blocking(move || match credential {
            Some(credential) => store.update_config_authorized(
                &actor,
                expected_revision,
                config,
                &credential,
                now_unix_ms(),
            ),
            None => store.update_config(&actor, expected_revision, config, now_unix_ms()),
        })
        .await
        .context("Coordinator 候选配置更新任务失败")?
    }

    pub(crate) async fn publish_coordinator_candidate_declaration(
        &self,
        identity: &Identity,
        willingness: CoordinatorCandidateWillingness,
        public_reachability: CoordinatorPublicReachability,
        resources: CoordinatorCandidateResources,
        validity_ms: u64,
    ) -> Result<CoordinatorCandidateStatus> {
        let actor = NodeId::from_public_key(&identity.public_key());
        let network_id = self
            .network_id
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        let store = self
            .coordinator_candidate_store
            .clone()
            .context("Coordinator 候选组不可用：缺少审批授权网络资料")?;
        let observed_at_unix_ms = now_unix_ms();
        let config = store
            .status(&actor, &network_id, observed_at_unix_ms)?
            .config
            .context("Coordinator 候选配置尚未生效")?;
        let expires_at_unix_ms = observed_at_unix_ms
            .checked_add(validity_ms)
            .context("Coordinator 候选声明有效期溢出")?;
        let declaration = CoordinatorCandidateDeclaration::signed(
            network_id,
            config.revision,
            public_reachability,
            resources,
            willingness,
            observed_at_unix_ms,
            expires_at_unix_ms,
            identity,
        )?;
        tokio::task::spawn_blocking(move || {
            store.publish_declaration(&actor, declaration, now_unix_ms())
        })
        .await
        .context("Coordinator 候选声明发布任务失败")?
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        if let Err(error) = self.disable_relay().await {
            failures.push(format!("QUIC 中继服务关闭失败: {error:#}"));
        }

        if let Some(task) = self.signaling_task.take() {
            task.abort();
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("信令服务异常退出: {error:#}")),
                Err(error) if error.is_cancelled() => {}
                Err(error) => failures.push(format!("信令服务任务异常: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("{}", failures.join("; "))
        }
    }
}

async fn resolve_relay_advertisement(
    external_host: Option<&str>,
    bound_addr: SocketAddr,
) -> Result<SocketAddr> {
    let Some(host) = external_host else {
        return Ok(bound_addr);
    };

    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok(addr);
    }

    let bracketed_ip = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .and_then(|host| host.parse::<IpAddr>().ok());
    if let Some(ip) = host.parse::<IpAddr>().ok().or(bracketed_ip) {
        return Ok(SocketAddr::new(ip, bound_addr.port()));
    }

    let explicit_port = host
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok());
    let addresses = if explicit_port {
        tokio::net::lookup_host(host)
            .await
            .map(|addresses| addresses.collect::<Vec<_>>())
    } else {
        tokio::net::lookup_host((host, bound_addr.port()))
            .await
            .map(|addresses| addresses.collect::<Vec<_>>())
    }
    .with_context(|| format!("无法解析中继外部主机 {host}"))?;

    addresses
        .into_iter()
        .next()
        .with_context(|| format!("中继外部主机 {host} 没有可用地址"))
}

impl Drop for EmbeddedCoordinator {
    fn drop(&mut self) {
        if let Some(task) = self.signaling_task.take() {
            task.abort();
        }
        self.relay_directory.set_primary(None);
        if let Some(task) = self.relay_task.take() {
            task.abort();
        }
        self.relay_server.take();
        self.relay_health = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_start_errors_have_stable_explanations() {
        let cases = [
            (
                std::io::ErrorKind::Unsupported,
                "relay.start.platform_unsupported",
                RelayLifecycleState::Unsupported,
            ),
            (
                std::io::ErrorKind::AddrInUse,
                "relay.start.address_in_use",
                RelayLifecycleState::Failed,
            ),
            (
                std::io::ErrorKind::PermissionDenied,
                "relay.start.permission_denied",
                RelayLifecycleState::Failed,
            ),
            (
                std::io::ErrorKind::NetworkUnreachable,
                "relay.start.network_unreachable",
                RelayLifecycleState::Failed,
            ),
            (
                std::io::ErrorKind::InvalidInput,
                "relay.start.unknown",
                RelayLifecycleState::Failed,
            ),
            (
                std::io::ErrorKind::Other,
                "relay.start.unknown",
                RelayLifecycleState::Failed,
            ),
        ];

        for (kind, expected_reason, expected_lifecycle) in cases {
            let error = anyhow::Error::new(std::io::Error::new(kind, "relay bind failed"))
                .context("start Relay");
            let (reason_code, lifecycle) = classify_relay_start_error(&error);
            assert_eq!(reason_code, expected_reason, "unexpected code for {kind:?}");
            assert_eq!(
                lifecycle, expected_lifecycle,
                "unexpected state for {kind:?}"
            );
        }
    }

    #[tokio::test]
    async fn relay_advertisement_uses_external_ipv4_host() {
        let bound_addr = "0.0.0.0:16667".parse().expect("bound address");
        let advertised = resolve_relay_advertisement(Some("101.36.116.21"), bound_addr)
            .await
            .expect("resolve external relay host");

        assert_eq!(
            advertised,
            "101.36.116.21:16667".parse().expect("advertised address")
        );
    }

    #[tokio::test]
    async fn relay_advertisement_preserves_bind_address_without_external_host() {
        let bound_addr = "10.7.53.243:16667".parse().expect("bound address");
        let advertised = resolve_relay_advertisement(None, bound_addr)
            .await
            .expect("use bound relay address");

        assert_eq!(advertised, bound_addr);
    }

    #[tokio::test]
    async fn allocation_update_rolls_back_persistence_when_live_lease_blocks_it() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-allocation-rollback-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "rollback-mesh",
            0,
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator");
        let previous = coordinator
            .session_manager
            .current_network_allocation()
            .await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        coordinator
            .session_manager
            .register_client(
                "live-client".to_string(),
                vec![7; 32],
                "192.0.2.10:41000".parse().expect("socket address"),
                51820,
                fuckipv6_common::NatType::Open,
                tx,
            )
            .await
            .expect("register live client");

        let error = coordinator
            .update_network_allocation(NetworkAllocation {
                mode: fuckipv6_common::NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ip_pool: Some("fd00::1000/120".to_string()),
            })
            .await
            .expect_err("live IPv6 lease must reject IPv4-only allocation");
        assert!(format!("{error:#}").contains("持久化配置已回滚"));
        assert_eq!(
            coordinator
                .session_manager
                .current_network_allocation()
                .await,
            previous
        );
        assert_eq!(
            coordinator
                .persisted_network_allocation()
                .await
                .expect("read rolled back allocation"),
            previous
        );

        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn allocation_persistence_failure_keeps_runtime_unchanged() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-allocation-persist-failure-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");
        let config_path = configs
            .coordinator_config_path("persist-failure-mesh")
            .expect("coordinator config path");
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "persist-failure-mesh",
            0,
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator");
        let previous = coordinator
            .session_manager
            .current_network_allocation()
            .await;
        std::fs::remove_file(&config_path).expect("remove config file");
        std::fs::create_dir_all(&config_path).expect("replace config with directory");

        let error = coordinator
            .update_network_allocation(NetworkAllocation {
                mode: fuckipv6_common::NetworkMode::Ipv4Only,
                ipv4_pool: "192.168.100.0/24".to_string(),
                ipv6_pool: "fd00::1000/120".to_string(),
                ip_pool: Some("fd00::1000/120".to_string()),
            })
            .await
            .expect_err("persistence failure must reject update");
        assert!(format!("{error:#}").contains("读取协调端持久配置失败"));
        assert_eq!(
            coordinator
                .session_manager
                .current_network_allocation()
                .await,
            previous
        );

        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[tokio::test]
    async fn test_embedded_coordinator_startup() {
        let config_dir = std::env::temp_dir().join(format!(
            "fuckipv6-embedded-coordinator-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config_dir);
        // 使用端口 0 避免冲突 (信令使用系统空闲端口)
        let mut coordinator = EmbeddedCoordinator::start_with_config_dir(
            "test-mesh",
            0, // random free port
            0,
            None,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
            config_dir.clone(),
        )
        .await
        .expect("start embedded coordinator failed");

        assert_eq!(coordinator.ticket.network_name, "test-mesh");
        assert!(coordinator.ticket.to_string().starts_with("fk://"));
        assert_ne!(coordinator.signaling_addr.port(), 0);
        assert_ne!(
            coordinator
                .relay_addr()
                .expect("relay should be enabled by default")
                .port(),
            0
        );
        coordinator.shutdown().await.expect("shutdown coordinator");
        let _ = std::fs::remove_dir_all(config_dir);
    }
}
