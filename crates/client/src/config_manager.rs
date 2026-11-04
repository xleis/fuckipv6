use crate::config_transaction::{
    ConfigurationApplyResult, ConfigurationDocument, ConfigurationHistoryEntry,
    ConfigurationPreview, ConfigurationTransactionError, ConfigurationTransactionStore,
};
use anyhow::{Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    preview_network_profile, AuditEntry, AuditId, AuditLog, AuditOutcome, ContentHash,
    JoinTermsAck, MembershipTerms, NetworkCreateProfile, NetworkId, NetworkMembership,
    NetworkProfileApplyResult, NetworkProfileDiffEntry, NetworkProfilePreview,
    NetworkProfileUpdate, NetworkRecord, NodeId as DomainNodeId, OperationId, PublicKey,
    NETWORK_PUBLIC_PROFILE_VALIDITY_MS,
};
use fuckipv6_common::i18n::LanguagePreference;
use fuckipv6_common::{ClientConfig, Identity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const NODE_IDENTITY_FILE: &str = "node_identity.pem";
const NODE_METADATA_FILE: &str = "node_identity.json";
const NETWORK_IDENTITY_FILE: &str = "network_identity.pem";
const NETWORK_RECORD_FILE: &str = "network.json";
const NETWORK_GOVERNANCE_FILE: &str = "governance.json";
const JOINED_MEMBERSHIPS_FILE: &str = "memberships.json";
const JOINED_MEMBERSHIPS_SCHEMA_VERSION: u16 = 1;

/// 网络配置信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub coordinator_url: String,
    pub created_at: String,
    pub last_used: String,
    /// Stable network identifier derived from the network public key. Legacy
    /// joined networks do not have this metadata yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// Whether this node owns and must restore the embedded signaling/relay
    /// services for this network.
    #[serde(default)]
    pub owns_embedded_services: bool,
    /// The concrete listening ports are persisted after the services bind so a
    /// restart can restore the same invitation endpoint. `None` keeps old
    /// network-list files readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signaling_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_pool: Option<String>,
}

/// 网络配置列表
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkList {
    pub networks: HashMap<String, NetworkConfig>,
    pub default_network: Option<String>,
}

/// Durable local relationship between a configured network directory and the
/// stable network membership it represents. Membership state is deliberately
/// separate from connection/runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JoinedNetworkMembership {
    pub network_name: String,
    pub membership: NetworkMembership,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinedMembershipFile {
    #[serde(default = "joined_membership_schema_version")]
    schema_version: u16,
    #[serde(default)]
    memberships: BTreeMap<String, JoinedNetworkMembership>,
}

impl Default for JoinedMembershipFile {
    fn default() -> Self {
        Self {
            schema_version: JOINED_MEMBERSHIPS_SCHEMA_VERSION,
            memberships: BTreeMap::new(),
        }
    }
}

fn joined_membership_schema_version() -> u16 {
    JOINED_MEMBERSHIPS_SCHEMA_VERSION
}

/// A successfully persisted owner-created network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedOwnedNetwork {
    pub config_path: PathBuf,
    pub record: NetworkRecord,
}

/// Stable, adapter-mappable reasons for a failed network creation attempt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetworkCreationError {
    #[error("网络资料无效: {0}")]
    Invalid(String),
    #[error("网络名称已存在: {0}")]
    DuplicateName(String),
    #[error("网络 ID 已存在: {0}")]
    DuplicateId(String),
    #[error("无权创建网络: {0}")]
    PermissionDenied(String),
    #[error("网络持久化失败: {0}")]
    Storage(String),
}

/// Stable, adapter-mappable profile preview/update failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetworkProfileError {
    #[error("网络资料无效: {0}")]
    Invalid(String),
    #[error("网络资料版本冲突: 期望 {expected}，当前 {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("网络资料不存在: {0}")]
    NotFound(String),
    #[error("网络资料持久化失败: {0}")]
    Storage(String),
}

/// Node-wide metadata that is independent from any one joined network.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default)]
    language: LanguagePreference,
    /// Unix timestamp of the current node identity. Legacy metadata is migrated
    /// once from the identity file's modification time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_created_at_unix_ms: Option<u64>,
}

/// 配置管理器
#[derive(Clone)]
pub struct ConfigManager {
    config_dir: PathBuf,
}

impl ConfigManager {
    /// 创建配置管理器
    pub fn new() -> Result<Self> {
        let config_dir = Self::get_config_dir()?;

        // 确保配置目录存在
        std::fs::create_dir_all(&config_dir).context("无法创建配置目录")?;

        Ok(Self { config_dir })
    }

    /// Create a manager rooted at an explicit directory.
    ///
    /// This is used by embedders and tests that must not touch the user's
    /// default configuration directory.
    pub fn new_for_dir(config_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&config_dir).context("无法创建配置目录")?;
        Ok(Self { config_dir })
    }

    /// Read-only root used for node-local protected state.
    pub(crate) fn config_dir(&self) -> &std::path::Path {
        &self.config_dir
    }

    /// 获取配置目录
    fn get_config_dir() -> Result<PathBuf> {
        Self::resolve_config_dir(
            std::env::var_os("FUCKIPV6_CONFIG_DIR"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    fn resolve_config_dir(
        configured: Option<OsString>,
        xdg_config: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<PathBuf> {
        // Service managers need a fixed, writable configuration root that is
        // independent of the invoking user's desktop session.
        if let Some(configured) = configured {
            return Ok(PathBuf::from(configured));
        }

        // 优先使用 $XDG_CONFIG_HOME/fuckipv6
        if let Some(xdg_config) = xdg_config {
            return Ok(PathBuf::from(xdg_config).join("fuckipv6"));
        }

        // 其次使用 $HOME/.config/fuckipv6
        if let Some(home) = home {
            return Ok(PathBuf::from(home).join(".config").join("fuckipv6"));
        }

        anyhow::bail!("无法确定配置目录位置")
    }

    /// 获取网络配置目录
    fn get_network_dir(&self, network_name: &str) -> PathBuf {
        self.config_dir.join(network_name)
    }

    fn validate_network_name(network_name: &str) -> Result<()> {
        if network_name.is_empty() {
            anyhow::bail!("网络名称不能为空");
        }

        if network_name == "." || network_name == ".." {
            anyhow::bail!("网络名称不能为 '.' 或 '..'");
        }

        if network_name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        {
            anyhow::bail!("网络名称只能包含字母、数字、'.'、'-' 和 '_'");
        }

        Ok(())
    }

    /// 获取网络列表文件路径
    fn get_networks_file(&self) -> PathBuf {
        self.config_dir.join("networks.json")
    }

    fn joined_memberships_path(&self) -> PathBuf {
        self.config_dir.join(JOINED_MEMBERSHIPS_FILE)
    }

    fn joined_memberships_lock_path(&self) -> PathBuf {
        self.config_dir.join(".memberships.lock")
    }

    fn lock_joined_memberships(&self) -> Result<std::fs::File> {
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.joined_memberships_lock_path())
            .context("无法打开本地成员关系锁")?;
        lock_file.lock_exclusive().context("无法锁定本地成员关系")?;
        Ok(lock_file)
    }

    fn load_joined_membership_file(&self) -> Result<JoinedMembershipFile> {
        let path = self.joined_memberships_path();
        if !path.exists() {
            return Ok(JoinedMembershipFile::default());
        }
        let content = std::fs::read(&path)
            .with_context(|| format!("无法读取本地成员关系: {}", path.display()))?;
        let file: JoinedMembershipFile = serde_json::from_slice(&content)
            .with_context(|| format!("无法解析本地成员关系: {}", path.display()))?;
        if file.schema_version != JOINED_MEMBERSHIPS_SCHEMA_VERSION {
            anyhow::bail!("不支持的本地成员关系版本: {}", file.schema_version);
        }
        Ok(file)
    }

    fn persist_joined_membership_file(&self, file: &JoinedMembershipFile) -> Result<()> {
        let path = self.joined_memberships_path();
        let previous = if path.is_file() {
            Some(
                std::fs::read(&path)
                    .with_context(|| format!("无法备份本地成员关系: {}", path.display()))?,
            )
        } else {
            None
        };
        if let Err(error) = write_json_atomic(&path, file) {
            let rollback = match previous {
                Some(previous) => write_bytes_atomic(&path, &previous),
                None if path.is_file() => std::fs::remove_file(&path)
                    .with_context(|| format!("无法回滚本地成员关系: {}", path.display())),
                None => Ok(()),
            };
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(error).context(format!(
                    "本地成员关系写入失败且回滚失败: {rollback_error:#}"
                )),
            };
        }
        Ok(())
    }

    /// Read the durable membership projections without consulting runtime or
    /// discovery state. The file is replaced atomically, so readers never see a
    /// partial JSON document.
    pub(crate) fn list_joined_memberships(&self) -> Result<Vec<JoinedNetworkMembership>> {
        let _lock = self.lock_joined_memberships()?;
        Ok(self
            .load_joined_membership_file()?
            .memberships
            .into_values()
            .collect())
    }

    pub(crate) fn joined_membership(
        &self,
        network_id: &str,
    ) -> Result<Option<JoinedNetworkMembership>> {
        let network_id = NetworkId::parse(network_id.to_owned())
            .map_err(|error| anyhow::anyhow!("网络 ID 格式无效: {error}"))?;
        let _lock = self.lock_joined_memberships()?;
        Ok(self
            .load_joined_membership_file()?
            .memberships
            .get(network_id.as_str())
            .cloned())
    }

    pub(crate) fn joined_membership_for_network_name(
        &self,
        network_name: &str,
    ) -> Result<Option<JoinedNetworkMembership>> {
        Self::validate_network_name(network_name)?;
        let _lock = self.lock_joined_memberships()?;
        Ok(self
            .load_joined_membership_file()?
            .memberships
            .into_values()
            .find(|joined| joined.network_name == network_name))
    }

    /// Persist one accepted local membership. Repeating the same join returns
    /// the existing non-terminal record without changing the file. Terminal
    /// records may be replaced by a new explicit join.
    pub(crate) fn upsert_joined_membership(
        &self,
        network_name: &str,
        membership: NetworkMembership,
    ) -> Result<(JoinedNetworkMembership, bool)> {
        Self::validate_network_name(network_name)?;
        membership.validate().context("本地成员关系无效")?;
        if membership.state != fuckipv6_common::domain::MembershipState::Accepted {
            anyhow::bail!("只能持久化已接受的本地成员关系");
        }
        let network_id = membership.network_id.as_str().to_owned();
        let _lock = self.lock_joined_memberships()?;
        let mut file = self.load_joined_membership_file()?;
        if let Some(existing) = file.memberships.get(&network_id) {
            if existing.membership.node_id != membership.node_id {
                anyhow::bail!("网络 [{network_id}] 的本地成员关系属于其他节点身份");
            }
            if !existing.membership.state.is_terminal() {
                return Ok((existing.clone(), false));
            }
        }
        let stored = JoinedNetworkMembership {
            network_name: network_name.to_owned(),
            membership,
        };
        file.memberships.insert(network_id, stored.clone());
        self.persist_joined_membership_file(&file)?;
        Ok((stored, true))
    }

    /// Persist signed policy observations for joined memberships. A single
    /// lock and atomic replacement cover the whole batch so a partial policy
    /// refresh cannot leave memberships disagreeing with each other.
    pub(crate) fn observe_joined_membership_policies(
        &self,
        current_terms: &BTreeMap<String, MembershipTerms>,
        now_unix_ms: u64,
    ) -> Result<bool> {
        if current_terms.is_empty() {
            return Ok(false);
        }
        let _lock = self.lock_joined_memberships()?;
        let mut file = self.load_joined_membership_file()?;
        let mut changed = false;
        for (network_id, terms) in current_terms {
            if let Some(joined) = file.memberships.get_mut(network_id) {
                changed |= joined
                    .membership
                    .observe_current_terms(terms.clone(), now_unix_ms)
                    .context("记录本地成员关系策略变更失败")?;
            }
        }
        if changed {
            self.persist_joined_membership_file(&file)?;
        }
        Ok(changed)
    }

    /// Re-confirm the exact current terms for one existing membership. The
    /// network ID and expected hash pin the decision against policy changes.
    pub(crate) fn acknowledge_joined_membership_policy(
        &self,
        network_id: &str,
        expected_terms_hash: &ContentHash,
        ack: JoinTermsAck,
        now_unix_ms: u64,
    ) -> Result<JoinedNetworkMembership> {
        let network_id = NetworkId::parse(network_id.to_owned())
            .map_err(|error| anyhow::anyhow!("网络 ID 格式无效: {error}"))?;
        let _lock = self.lock_joined_memberships()?;
        let mut file = self.load_joined_membership_file()?;
        let joined = file
            .memberships
            .get_mut(network_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("本地不存在该网络的成员关系"))?;
        joined
            .membership
            .acknowledge_current_terms(expected_terms_hash, ack, now_unix_ms)
            .context("确认本地成员关系策略失败")?;
        joined.membership.validate().context("本地成员关系无效")?;
        let stored = joined.clone();
        self.persist_joined_membership_file(&file)?;
        Ok(stored)
    }

    /// Remove only one stable network membership. Other network relationships
    /// remain byte-for-byte equivalent in the persisted file.
    pub(crate) fn remove_joined_membership(
        &self,
        network_id: &str,
    ) -> Result<Option<JoinedNetworkMembership>> {
        let network_id = NetworkId::parse(network_id.to_owned())
            .map_err(|error| anyhow::anyhow!("网络 ID 格式无效: {error}"))?;
        let _lock = self.lock_joined_memberships()?;
        let mut file = self.load_joined_membership_file()?;
        let removed = file.memberships.remove(network_id.as_str());
        if removed.is_some() {
            self.persist_joined_membership_file(&file)?;
        }
        Ok(removed)
    }

    /// Stable node identity shared by every network hosted by this node.
    pub(crate) fn node_identity_path(&self) -> PathBuf {
        self.config_dir.join(NODE_IDENTITY_FILE)
    }

    /// Per-network coordinator configuration, including the persistent blacklist.
    pub(crate) fn coordinator_config_path(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        Ok(self.get_network_dir(network_name).join("coordinator.toml"))
    }

    /// Load or create the node-wide signing identity.
    pub(crate) fn load_node_identity(&self) -> Result<Identity> {
        crate::owner_security::rotation::load_reconciled_identity(self)
    }

    fn load_node_metadata(&self) -> Result<NodeMetadata> {
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        if !metadata_path.exists() {
            return Ok(NodeMetadata::default());
        }
        let content = std::fs::read_to_string(&metadata_path)
            .with_context(|| format!("无法读取节点元数据: {}", metadata_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("无法解析节点元数据: {}", metadata_path.display()))
    }

    fn save_node_metadata(&self, metadata: &NodeMetadata) -> Result<()> {
        let metadata_path = self.config_dir.join(NODE_METADATA_FILE);
        let content = serde_json::to_string_pretty(metadata).context("无法序列化节点元数据")?;
        let temporary_file =
            metadata_path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&temporary_file, content)
            .with_context(|| format!("无法写入临时节点元数据: {}", temporary_file.display()))?;
        if let Err(error) = std::fs::rename(&temporary_file, &metadata_path) {
            let _ = std::fs::remove_file(&temporary_file);
            return Err(error)
                .with_context(|| format!("无法替换节点元数据: {}", metadata_path.display()));
        }
        Ok(())
    }

    /// Load the optional display name from node-wide metadata.
    pub(crate) fn node_display_name(&self) -> Result<Option<String>> {
        normalize_display_name(self.load_node_metadata()?.display_name)
    }

    /// Return the stable creation time for the current node identity.
    ///
    /// Metadata written by older releases has no timestamp. In that case the
    /// identity file's modification time is the best available evidence; it is
    /// persisted immediately so later metadata edits and reopen operations do
    /// not make the displayed creation time drift.
    pub(crate) fn node_identity_created_at_unix_ms(&self) -> Result<u64> {
        let mut metadata = self.load_node_metadata()?;
        if let Some(created_at) = metadata.identity_created_at_unix_ms {
            return Ok(created_at);
        }

        let identity_path = self.node_identity_path();
        let modified = std::fs::metadata(&identity_path)
            .with_context(|| format!("无法读取节点身份信息: {}", identity_path.display()))?
            .modified()
            .with_context(|| format!("无法读取节点身份创建时间: {}", identity_path.display()))?;
        let created_at = modified
            .duration_since(std::time::UNIX_EPOCH)
            .context("节点身份创建时间早于 Unix epoch")?
            .as_millis()
            .try_into()
            .context("节点身份创建时间超出 u64 范围")?;
        metadata.identity_created_at_unix_ms = Some(created_at);
        self.save_node_metadata(&metadata)?;
        Ok(created_at)
    }

    /// Record the creation time of a newly generated identity while preserving
    /// unrelated node-wide metadata such as display name and language.
    pub(crate) fn set_node_identity_created_at_unix_ms(&self, created_at: u64) -> Result<()> {
        let mut metadata = self.load_node_metadata()?;
        metadata.identity_created_at_unix_ms = Some(created_at);
        self.save_node_metadata(&metadata)
    }

    /// Persist a node-wide display name. Empty or whitespace-only values clear it.
    pub(crate) fn set_node_display_name(&self, display_name: Option<&str>) -> Result<()> {
        let mut metadata = self.load_node_metadata()?;
        metadata.display_name = normalize_display_name(display_name.map(str::to_string))?;
        self.save_node_metadata(&metadata)
    }

    /// Node-local language choice. This is deliberately not synchronized as
    /// network state: each owner controls their own interface language.
    pub(crate) fn node_language_preference(&self) -> Result<LanguagePreference> {
        Ok(self.load_node_metadata()?.language)
    }

    pub(crate) fn set_node_language_preference(&self, language: LanguagePreference) -> Result<()> {
        let mut metadata = self.load_node_metadata()?;
        metadata.language = language;
        self.save_node_metadata(&metadata)
    }

    /// 加载网络列表
    fn load_network_list(&self) -> Result<NetworkList> {
        let networks_file = self.get_networks_file();

        if networks_file.exists() {
            let content = std::fs::read_to_string(&networks_file)
                .with_context(|| format!("无法读取网络列表: {}", networks_file.display()))?;
            Ok(serde_json::from_str(&content)
                .with_context(|| format!("无法解析网络列表: {}", networks_file.display()))?)
        } else {
            Ok(NetworkList::default())
        }
    }

    /// 保存网络列表
    fn save_network_list(&self, list: &NetworkList) -> Result<()> {
        let networks_file = self.get_networks_file();
        let content = serde_json::to_string_pretty(list).context("无法序列化网络列表")?;
        let temporary_file =
            networks_file.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&temporary_file, content)
            .with_context(|| format!("无法写入临时网络列表: {}", temporary_file.display()))?;
        if let Err(error) = std::fs::rename(&temporary_file, &networks_file) {
            let _ = std::fs::remove_file(&temporary_file);
            return Err(error)
                .with_context(|| format!("无法替换网络列表: {}", networks_file.display()));
        }
        Ok(())
    }

    pub(crate) fn network_identity_path(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        Ok(self
            .get_network_dir(network_name)
            .join(NETWORK_IDENTITY_FILE))
    }

    pub(crate) fn network_record_path(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        Ok(self.get_network_dir(network_name).join(NETWORK_RECORD_FILE))
    }

    pub(crate) fn network_governance_path(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        Ok(self
            .get_network_dir(network_name)
            .join(NETWORK_GOVERNANCE_FILE))
    }

    /// Load the durable governance record for an owner-created network.
    pub fn load_network_record(
        &self,
        network_name: &str,
    ) -> std::result::Result<NetworkRecord, NetworkCreationError> {
        let path = self
            .network_record_path(network_name)
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        let content = std::fs::read_to_string(&path).map_err(|error| {
            NetworkCreationError::Storage(format!("无法读取 {}: {error}", path.display()))
        })?;
        let record = serde_json::from_str::<NetworkRecord>(&content).map_err(|error| {
            NetworkCreationError::Storage(format!("无法解析 {}: {error}", path.display()))
        })?;
        record
            .validate()
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        Ok(record)
    }

    /// Preview an owner-editable public profile against the exact persisted
    /// record. Preview is read-only and does not create an audit entry.
    pub fn preview_network_profile(
        &self,
        network_name: &str,
        expected_revision: u64,
        update: NetworkProfileUpdate,
    ) -> std::result::Result<NetworkProfilePreview, NetworkProfileError> {
        let record = self
            .load_network_record(network_name)
            .map_err(map_network_profile_load_error)?;
        preview_network_profile(
            network_name,
            &record,
            expected_revision,
            &update,
            unix_time_ms().max(1),
        )
        .map_err(|error| NetworkProfileError::Invalid(error.to_string()))
    }

    /// Apply a previously previewed profile update under an exclusive
    /// filesystem lock. The record is re-read inside the lock, re-signed with
    /// the independent network identity, written atomically, and followed by a
    /// governance audit entry. If the audit write fails, the previous record is
    /// restored atomically.
    pub fn update_network_profile(
        &self,
        network_name: &str,
        expected_revision: u64,
        update: NetworkProfileUpdate,
        actor: &str,
        source: &str,
        operation_id: Option<OperationId>,
    ) -> std::result::Result<NetworkProfileApplyResult, NetworkProfileError> {
        let record_path = self
            .network_record_path(network_name)
            .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
        let identity_path = self
            .network_identity_path(network_name)
            .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
        let governance_path = self
            .network_governance_path(network_name)
            .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
        if !record_path.exists() {
            return Err(NetworkProfileError::NotFound(network_name.into()));
        }
        let lock_path = record_path.with_file_name(".profile.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| NetworkProfileError::Storage(format!("无法打开资料锁: {error}")))?;
        lock_file
            .lock_exclusive()
            .map_err(|error| NetworkProfileError::Storage(format!("无法锁定网络资料: {error}")))?;

        let result = (|| {
            let old_bytes = std::fs::read(&record_path).map_err(|error| {
                NetworkProfileError::Storage(format!("无法读取当前网络资料: {error}"))
            })?;
            let record = self
                .load_network_record(network_name)
                .map_err(map_network_profile_load_error)?;
            let now_unix_ms = unix_time_ms().max(1);
            let mut preview = preview_network_profile(
                network_name,
                &record,
                expected_revision,
                &update,
                now_unix_ms,
            )
            .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
            if preview.conflict {
                return Err(NetworkProfileError::RevisionConflict {
                    expected: expected_revision,
                    actual: record.revision,
                });
            }

            let identity = Identity::load_from_file(&identity_path).map_err(|error| {
                NetworkProfileError::Storage(format!("无法加载网络签名身份: {error:#}"))
            })?;
            let identity_public_key = PublicKey::from_bytes(identity.public_key_bytes());
            if identity_public_key != record.public_key {
                return Err(NetworkProfileError::Invalid(
                    "网络签名身份与网络公钥不匹配".into(),
                ));
            }

            let changed_paths = preview
                .diff
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>();
            let needs_signature = record.signed_profile.is_none()
                || record
                    .signed_profile
                    .as_ref()
                    .is_some_and(|profile| profile.document.is_expired_at(now_unix_ms));
            let needs_write = !changed_paths.is_empty() || needs_signature;
            let previous_revision = record.revision;
            let mut updated = record.clone();
            if !changed_paths.is_empty() {
                updated
                    .apply_profile_update(&update, expected_revision, now_unix_ms)
                    .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
            }
            if needs_write {
                updated
                    .sign_public_profile(
                        &identity,
                        now_unix_ms,
                        now_unix_ms.saturating_add(NETWORK_PUBLIC_PROFILE_VALIDITY_MS),
                    )
                    .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
                // Recompute the preview against the newly prepared record so
                // the public candidate, revision and warnings are the exact
                // values that will be persisted.
                let post_update = NetworkProfileUpdate::from_record(&updated);
                preview = preview_network_profile(
                    network_name,
                    &updated,
                    updated.revision,
                    &post_update,
                    now_unix_ms,
                )
                .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
                preview.diff = changed_paths
                    .iter()
                    .map(|path| NetworkProfileDiffEntry {
                        path: path.clone(),
                        before: preview
                            .diff
                            .iter()
                            .find(|entry| entry.path == *path)
                            .and_then(|entry| entry.before.clone()),
                        after: preview
                            .diff
                            .iter()
                            .find(|entry| entry.path == *path)
                            .and_then(|entry| entry.after.clone()),
                    })
                    .collect();
            }

            if needs_write {
                let mut governance = if governance_path.exists() {
                    let bytes = std::fs::read(&governance_path).map_err(|error| {
                        NetworkProfileError::Storage(format!("无法读取治理日志: {error}"))
                    })?;
                    serde_json::from_slice::<AuditLog>(&bytes).map_err(|error| {
                        NetworkProfileError::Storage(format!("无法解析治理日志: {error}"))
                    })?
                } else {
                    AuditLog::default()
                };
                let actor_id = DomainNodeId::parse(actor)
                    .map_err(|error| NetworkProfileError::Invalid(error.to_string()))?;
                let audit_id = AuditId::from_public_key(
                    format!(
                        "{}:network.profile.update:{}:{}",
                        updated.network_id, updated.revision, now_unix_ms
                    )
                    .as_bytes(),
                );
                let mut details = BTreeMap::new();
                details.insert("old_revision".into(), previous_revision.to_string());
                details.insert("new_revision".into(), updated.revision.to_string());
                details.insert("changed_paths".into(), changed_paths.join(","));
                details.insert("source".into(), source.to_string());
                details.insert("actor".into(), actor.to_string());
                governance
                    .append(AuditEntry {
                        audit_id,
                        operation_id,
                        actor: actor_id,
                        network_id: Some(updated.network_id.clone()),
                        action: "network.profile.update".into(),
                        resource: format!("network/{}", updated.network_id),
                        outcome: AuditOutcome::Succeeded,
                        occurred_at_unix_ms: now_unix_ms,
                        summary: format!("Network [{}] public profile updated", updated.name),
                        details,
                        sensitive_keys: Default::default(),
                    })
                    .map_err(|error| {
                        NetworkProfileError::Storage(format!("无法写入资料审计: {error}"))
                    })?;

                write_network_profile_record_and_audit(
                    &record_path,
                    &old_bytes,
                    &updated,
                    &governance_path,
                    &governance,
                )?;
            }

            let signed_profile = updated
                .signed_profile
                .clone()
                .ok_or_else(|| NetworkProfileError::Invalid("网络资料没有可返回的签名".into()))?;
            Ok(NetworkProfileApplyResult {
                network_name: network_name.into(),
                revision: updated.revision,
                previous_revision,
                applied_at_unix_ms: now_unix_ms,
                changed_paths,
                signed_profile,
                projections: preview.projections,
                warnings: preview.warnings,
                audit_action: if needs_write {
                    "network.profile.update".into()
                } else {
                    "network.profile.unchanged".into()
                },
            })
        })();
        let _ = lock_file.unlock();
        result
    }

    /// Create an owner-scoped network and its independent signing identity.
    ///
    /// The network identity is intentionally distinct from the node identity:
    /// one node may create several networks, and each must retain its own stable
    /// network ID. The caller is responsible for service startup, while this
    /// method owns all durable profile, governance and configuration writes.
    pub fn create_owned_network(
        &self,
        profile: NetworkCreateProfile,
        coordinator_url: &str,
        set_default: bool,
    ) -> std::result::Result<CreatedOwnedNetwork, NetworkCreationError> {
        let network_identity = Identity::generate().map_err(|error| {
            NetworkCreationError::Storage(format!("无法生成网络身份: {error:#}"))
        })?;
        self.create_owned_network_with_identity(
            profile,
            coordinator_url,
            set_default,
            network_identity,
        )
    }

    fn create_owned_network_with_identity(
        &self,
        profile: NetworkCreateProfile,
        coordinator_url: &str,
        set_default: bool,
        network_identity: Identity,
    ) -> std::result::Result<CreatedOwnedNetwork, NetworkCreationError> {
        let profile = profile
            .normalized()
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        let network_name = profile.name.clone();
        let network_dir = self.get_network_dir(&network_name);
        let network_identity_path = self
            .network_identity_path(&network_name)
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        let network_record_path = self
            .network_record_path(&network_name)
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        let network_governance_path = self
            .network_governance_path(&network_name)
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        let config_file = network_dir.join("config.toml");
        let mut list = self
            .load_network_list()
            .map_err(|error| NetworkCreationError::Storage(format!("{error:#}")))?;

        if list.networks.contains_key(&network_name)
            || config_file.exists()
            || network_record_path.exists()
        {
            return Err(NetworkCreationError::DuplicateName(network_name));
        }

        let node_identity = self.load_node_identity().map_err(|error| {
            NetworkCreationError::Storage(format!("无法加载节点身份: {error:#}"))
        })?;
        let public_key = PublicKey::from_bytes(network_identity.public_key_bytes());
        let owner = DomainNodeId::from_public_key(&node_identity.public_key_bytes());
        let now_unix_ms = unix_time_ms().max(1);
        let mut record =
            NetworkRecord::create_owned(profile, public_key, owner.clone(), now_unix_ms)
                .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;
        record
            .sign_public_profile(
                &network_identity,
                now_unix_ms,
                now_unix_ms.saturating_add(NETWORK_PUBLIC_PROFILE_VALIDITY_MS),
            )
            .map_err(|error| NetworkCreationError::Invalid(error.to_string()))?;

        self.ensure_network_id_unique(&list, &record)?;

        std::fs::create_dir_all(&network_dir).map_err(|error| {
            NetworkCreationError::Storage(format!(
                "无法创建网络目录 {}: {error}",
                network_dir.display()
            ))
        })?;

        let result = (|| -> std::result::Result<PathBuf, NetworkCreationError> {
            network_identity
                .save_to_file(&network_identity_path)
                .map_err(|error| {
                    NetworkCreationError::Storage(format!("无法保存网络身份: {error:#}"))
                })?;
            write_json_atomic(&network_record_path, &record).map_err(|error| {
                NetworkCreationError::Storage(format!("无法保存网络记录: {error:#}"))
            })?;

            let mut governance = AuditLog::default();
            let mut details = BTreeMap::new();
            details.insert("name".into(), record.name.clone());
            details.insert("owner".into(), record.owner.to_string());
            details.insert("revision".into(), record.revision.to_string());
            details.insert(
                "visibility".into(),
                format!("{:?}", record.visibility).to_ascii_lowercase(),
            );
            details.insert(
                "join_mode".into(),
                format!("{:?}", record.join_mode).to_ascii_lowercase(),
            );
            governance
                .append(AuditEntry {
                    audit_id: AuditId::from_public_key(
                        format!("{}:network.create:{now_unix_ms}", record.network_id).as_bytes(),
                    ),
                    operation_id: None,
                    actor: owner,
                    network_id: Some(record.network_id.clone()),
                    action: "network.create".into(),
                    resource: format!("network/{}", record.network_id),
                    outcome: AuditOutcome::Succeeded,
                    occurred_at_unix_ms: now_unix_ms,
                    summary: format!("Network [{}] created", record.name),
                    details,
                    sensitive_keys: Default::default(),
                })
                .map_err(|error| {
                    NetworkCreationError::Storage(format!("无法写入创建审计事件: {error}"))
                })?;
            write_json_atomic(&network_governance_path, &governance).map_err(|error| {
                NetworkCreationError::Storage(format!("无法保存治理日志: {error:#}"))
            })?;

            let config = ClientConfig {
                client_id: None,
                coordinator_url: coordinator_url.to_string(),
                private_key_path: self.node_identity_path(),
                network: Default::default(),
                tun: Default::default(),
                p2p: Default::default(),
                log: Default::default(),
                acl: Default::default(),
                routing: Default::default(),
                exit_node: Default::default(),
            };
            let transaction_store = self.configuration_store();
            transaction_store
                .add_network(
                    &network_name,
                    &config,
                    "local-owner",
                    "network-owner-create",
                )
                .map_err(|error| {
                    NetworkCreationError::Storage(format!("无法保存网络配置: {error:#}"))
                })?;

            let timestamp = chrono::Utc::now().to_rfc3339();
            list.networks.insert(
                network_name.clone(),
                NetworkConfig {
                    name: network_name.clone(),
                    coordinator_url: coordinator_url.to_string(),
                    created_at: timestamp.clone(),
                    last_used: timestamp,
                    network_id: Some(record.network_id.to_string()),
                    public_key: Some(record.public_key.to_string()),
                    owns_embedded_services: false,
                    signaling_port: None,
                    relay_port: None,
                    external_host: None,
                    ip_pool: None,
                },
            );
            if set_default || list.default_network.is_none() {
                list.default_network = Some(network_name.clone());
            }
            if let Err(error) = self.save_network_list(&list) {
                let rollback = transaction_store.remove_network(
                    &network_name,
                    "local-owner",
                    "network-owner-create-rollback",
                );
                if let Err(rollback_error) = rollback {
                    return Err(NetworkCreationError::Storage(format!(
                        "网络列表保存失败且配置回滚失败: {error:#}; {rollback_error:#}"
                    )));
                }
                return Err(NetworkCreationError::Storage(format!(
                    "网络列表保存失败: {error:#}"
                )));
            }

            let network_names = sorted_network_names(&list);
            transaction_store
                .current_network_path(&network_names, &network_name)
                .map_err(|error| {
                    NetworkCreationError::Storage(format!("无法定位网络配置: {error:#}"))
                })
        })();

        match result {
            Ok(config_path) => Ok(CreatedOwnedNetwork {
                config_path,
                record,
            }),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&network_dir);
                Err(error)
            }
        }
    }

    fn ensure_network_id_unique(
        &self,
        list: &NetworkList,
        record: &NetworkRecord,
    ) -> std::result::Result<(), NetworkCreationError> {
        let target = record.network_id.as_str();
        for network in list.networks.values() {
            if network.network_id.as_deref() == Some(target) {
                return Err(NetworkCreationError::DuplicateId(target.to_string()));
            }
        }

        let entries = std::fs::read_dir(&self.config_dir).map_err(|error| {
            NetworkCreationError::Storage(format!(
                "无法扫描网络目录 {}: {error}",
                self.config_dir.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                NetworkCreationError::Storage(format!("无法读取网络目录项: {error}"))
            })?;
            let record_path = entry.path().join(NETWORK_RECORD_FILE);
            if !record_path.is_file() {
                continue;
            }
            let content = std::fs::read_to_string(&record_path).map_err(|error| {
                NetworkCreationError::Storage(format!(
                    "无法读取 {}: {error}",
                    record_path.display()
                ))
            })?;
            let existing = serde_json::from_str::<NetworkRecord>(&content).map_err(|error| {
                NetworkCreationError::Storage(format!(
                    "无法解析 {}: {error}",
                    record_path.display()
                ))
            })?;
            if existing.network_id.as_str() == target {
                return Err(NetworkCreationError::DuplicateId(target.to_string()));
            }
        }
        Ok(())
    }

    /// 创建新网络配置
    pub fn create_network(
        &self,
        network_name: &str,
        coordinator_url: &str,
        set_default: bool,
    ) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        let network_dir = self.get_network_dir(network_name);
        let config_file = network_dir.join("config.toml");
        let mut list = self.load_network_list()?;

        // The coordinator persists coordinator.toml under the same per-network
        // directory before the client configuration is created. Only an
        // existing client config (or list entry) is a duplicate network.
        if list.networks.contains_key(network_name) || config_file.exists() {
            anyhow::bail!("网络配置已存在: {}", network_name);
        }

        // 创建网络目录，并确保基础配置进入当前原子 generation。
        std::fs::create_dir_all(&network_dir)
            .with_context(|| format!("无法创建网络配置目录: {}", network_dir.display()))?;
        // 生成配置文件
        let config = ClientConfig {
            client_id: None,
            coordinator_url: coordinator_url.to_string(),
            private_key_path: self.node_identity_path(),
            network: Default::default(),
            tun: Default::default(),
            p2p: Default::default(),
            log: Default::default(),
            acl: Default::default(),
            routing: Default::default(),
            exit_node: Default::default(),
        };

        let transaction_store = self.configuration_store();
        transaction_store.add_network(network_name, &config, "local-owner", "config-manager")?;

        // 更新网络列表
        let now = chrono::Utc::now().to_rfc3339();
        list.networks.insert(
            network_name.to_string(),
            NetworkConfig {
                name: network_name.to_string(),
                coordinator_url: coordinator_url.to_string(),
                created_at: now.clone(),
                last_used: now,
                network_id: None,
                public_key: None,
                owns_embedded_services: false,
                signaling_port: None,
                relay_port: None,
                external_host: None,
                ip_pool: None,
            },
        );

        if set_default || list.default_network.is_none() {
            list.default_network = Some(network_name.to_string());
        }

        if let Err(error) = self.save_network_list(&list) {
            let rollback = transaction_store.remove_network(
                network_name,
                "local-owner",
                "config-manager-rollback",
            );
            if let Err(rollback_error) = rollback {
                return Err(error).context(format!(
                    "网络列表保存失败，且配置事务回滚失败: {rollback_error:#}"
                ));
            }
            return Err(error).context("网络列表保存失败，已回滚配置事务");
        }

        let mut network_names = list.networks.keys().cloned().collect::<Vec<_>>();
        network_names.sort();
        let config_file = transaction_store.current_network_path(&network_names, network_name)?;

        Ok(config_file)
    }

    /// Record the concrete endpoints of embedded services owned by this node.
    /// This must happen after the listeners bind because port `0` is resolved by
    /// the operating system and the invitation ticket contains the actual port.
    pub fn set_embedded_service_metadata(
        &self,
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        external_host: Option<String>,
        ip_pool: Option<String>,
    ) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let mut list = self.load_network_list()?;
        let network = list
            .networks
            .get_mut(network_name)
            .with_context(|| format!("无法保存内嵌服务元数据：网络 [{network_name}] 不存在"))?;
        network.owns_embedded_services = true;
        network.signaling_port = Some(signaling_port);
        network.relay_port = Some(relay_port);
        network.external_host = external_host;
        network.ip_pool = ip_pool;
        self.save_network_list(&list)?;
        Ok(())
    }

    /// 获取网络配置文件路径
    pub fn get_network_config(&self, network_name: &str) -> Result<PathBuf> {
        Self::validate_network_name(network_name)?;
        let mut list = self.load_network_list()?;
        if !list.networks.contains_key(network_name) {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }
        let network_names = list.networks.keys().cloned().collect::<Vec<_>>();
        let config_file = self
            .configuration_store()
            .current_network_path(&network_names, network_name)?;

        if !config_file.is_file() {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }

        // 更新最后使用时间
        if let Some(network) = list.networks.get_mut(network_name) {
            network.last_used = chrono::Utc::now().to_rfc3339();
            let _ = self.save_network_list(&list);
        }

        Ok(config_file)
    }

    /// 删除网络配置
    pub fn delete_network(&self, network_name: &str) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let network_dir = self.get_network_dir(network_name);
        let mut list = self.load_network_list()?;
        let previous_list = list.clone();

        if list.networks.contains_key(network_name) {
            let transaction_store = self.configuration_store();
            let result =
                transaction_store.remove_network(network_name, "local-owner", "config-manager")?;

            list.networks.remove(network_name);
            if list.default_network.as_deref() == Some(network_name) {
                list.default_network = None;
            }

            if let Err(error) = self.save_network_list(&list) {
                let previous_names = previous_list.networks.keys().cloned().collect::<Vec<_>>();
                let rollback = transaction_store.rollback(
                    result.revision,
                    result.previous_revision,
                    &previous_names,
                    "local-owner",
                    "config-manager-rollback",
                );
                if let Err(rollback_error) = rollback {
                    return Err(error).context(format!(
                        "网络列表保存失败，且配置事务回滚失败: {rollback_error:#}"
                    ));
                }
                return Err(error).context("网络列表保存失败，已回滚配置事务");
            }
        }

        if network_dir.exists() {
            std::fs::remove_dir_all(&network_dir)
                .with_context(|| format!("无法删除网络配置目录: {}", network_dir.display()))?;
        }

        Ok(())
    }

    /// 列出所有网络配置
    pub fn list_networks(&self) -> Result<Vec<NetworkConfig>> {
        let list = self.load_network_list()?;
        let mut networks: Vec<_> = list.networks.into_values().collect();

        // 按最后使用时间排序
        networks.sort_by(|a, b| b.last_used.cmp(&a.last_used));

        Ok(networks)
    }

    /// 获取默认网络名称
    pub fn get_default_network(&self) -> Result<Option<String>> {
        let list = self.load_network_list()?;
        Ok(list.default_network)
    }

    /// 设置默认网络
    pub fn set_default_network(&self, network_name: &str) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let mut list = self.load_network_list()?;

        if !list.networks.contains_key(network_name) {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }

        list.default_network = Some(network_name.to_string());
        self.save_network_list(&list)?;

        Ok(())
    }

    /// 检查网络是否存在
    pub fn network_exists(&self, network_name: &str) -> bool {
        if Self::validate_network_name(network_name).is_err() {
            return false;
        }

        if let Ok(list) = self.load_network_list() {
            list.networks.contains_key(network_name)
        } else {
            false
        }
    }

    /// Attach the signed discovery identity to an existing local network.
    /// Repeating the same metadata is idempotent; a conflicting stable identity
    /// must never overwrite another network's configuration.
    pub(crate) fn set_network_identity_metadata(
        &self,
        network_name: &str,
        network_id: &str,
        public_key: &str,
    ) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let network_id = NetworkId::parse(network_id.to_owned())
            .map_err(|error| anyhow::anyhow!("网络 ID 格式无效: {error}"))?;
        let mut list = self.load_network_list()?;
        let network = list
            .networks
            .get_mut(network_name)
            .with_context(|| format!("无法设置身份元数据：网络 [{network_name}] 不存在"))?;
        if let Some(existing) = network.network_id.as_deref() {
            if existing != network_id.as_str() {
                anyhow::bail!(
                    "网络 [{network_name}] 已绑定稳定网络 ID [{existing}]，不能覆盖为 [{network_id}]"
                );
            }
        }
        if let Some(existing) = network.public_key.as_deref() {
            if existing != public_key {
                anyhow::bail!("网络 [{network_name}] 的公钥元数据与签名发现记录不一致");
            }
        }
        network.network_id = Some(network_id.to_string());
        network.public_key = Some(public_key.to_owned());
        self.save_network_list(&list)
    }

    /// Bind a stable network ID when an invitation was the authority and no
    /// signed public-network record was available. The public key remains
    /// unknown until a signed profile is discovered; the stable ID is still
    /// sufficient to prevent accidental joins to a different network.
    pub(crate) fn set_network_id_metadata(
        &self,
        network_name: &str,
        network_id: &str,
    ) -> Result<()> {
        Self::validate_network_name(network_name)?;
        let network_id = NetworkId::parse(network_id.to_owned())
            .map_err(|error| anyhow::anyhow!("网络 ID 格式无效: {error}"))?;
        let mut list = self.load_network_list()?;
        if list.networks.iter().any(|(name, network)| {
            name != network_name && network.network_id.as_deref() == Some(network_id.as_str())
        }) {
            anyhow::bail!("稳定网络 ID [{network_id}] 已绑定到其他本地配置");
        }
        let network = list
            .networks
            .get_mut(network_name)
            .with_context(|| format!("无法设置身份元数据：网络 [{network_name}] 不存在"))?;
        if let Some(existing) = network.network_id.as_deref() {
            if existing != network_id.as_str() {
                anyhow::bail!(
                    "网络 [{network_name}] 已绑定稳定网络 ID [{existing}]，不能覆盖为 [{network_id}]"
                );
            }
        }
        network.network_id = Some(network_id.to_string());
        self.save_network_list(&list)
    }

    fn configuration_store(&self) -> ConfigurationTransactionStore {
        ConfigurationTransactionStore::new(self.config_dir.clone())
    }

    fn configured_network_names(&self) -> Result<Vec<String>> {
        let mut names = self
            .load_network_list()?
            .networks
            .into_keys()
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    /// Complete, redacted configuration for every configured network.
    pub fn configuration_document(&self) -> Result<ConfigurationDocument> {
        let names = self.configured_network_names()?;
        self.configuration_store().current_document(&names)
    }

    /// Import and preview a complete configuration candidate without writing it.
    pub fn preview_configuration(
        &self,
        expected_revision: u64,
        networks: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<ConfigurationPreview> {
        let names = self.configured_network_names()?;
        self.configuration_store()
            .preview(expected_revision, &names, networks)
    }

    /// Atomically commit every network configuration under a new revision.
    pub fn apply_configuration(
        &self,
        expected_revision: u64,
        networks: std::collections::BTreeMap<String, serde_json::Value>,
        actor: &str,
        source: &str,
    ) -> std::result::Result<ConfigurationApplyResult, ConfigurationTransactionError> {
        let names = self
            .configured_network_names()
            .map_err(|error| ConfigurationTransactionError::Persistence(format!("{error:#}")))?;
        self.configuration_store()
            .apply(expected_revision, &names, networks, actor, source)
    }

    /// Restore a previous generation as a new, auditable revision.
    pub fn rollback_configuration(
        &self,
        expected_revision: u64,
        target_revision: u64,
        actor: &str,
        source: &str,
    ) -> std::result::Result<ConfigurationApplyResult, ConfigurationTransactionError> {
        let names = self
            .configured_network_names()
            .map_err(|error| ConfigurationTransactionError::Persistence(format!("{error:#}")))?;
        self.configuration_store().rollback(
            expected_revision,
            target_revision,
            &names,
            actor,
            source,
        )
    }

    /// Append-only configuration revision history, newest first.
    pub fn configuration_history(&self) -> Result<Vec<ConfigurationHistoryEntry>> {
        let names = self.configured_network_names()?;
        self.configuration_store().history(&names)
    }

    /// 从URL提取网络名称
    pub fn extract_network_name(url: &str) -> String {
        // 从URL中提取域名作为网络名称
        // 例如: ws://example.com:6666 -> example
        //      wss://vpn.example.com -> vpn.example

        let url = url.trim_start_matches("ws://").trim_start_matches("wss://");

        let domain = if let Some(rest) = url.strip_prefix('[') {
            rest.split(']').next().unwrap_or("ipv6")
        } else {
            url.split(':').next().unwrap_or(url)
        };
        let domain = url_to_network_slug(domain.split('/').next().unwrap_or(domain));

        // 移除顶级域名，保留主域名和子域名
        let parts: Vec<&str> = domain.split('.').collect();
        if parts.len() > 2 {
            parts[..parts.len() - 1].join(".")
        } else if !parts.is_empty() {
            parts[0].to_string()
        } else {
            "default".to_string()
        }
    }
}

fn write_network_profile_record_and_audit(
    record_path: &Path,
    old_record_bytes: &[u8],
    record: &NetworkRecord,
    governance_path: &Path,
    governance: &AuditLog,
) -> std::result::Result<(), NetworkProfileError> {
    write_json_atomic(record_path, record)
        .map_err(|error| NetworkProfileError::Storage(format!("无法保存网络资料: {error:#}")))?;
    if let Err(error) = write_json_atomic(governance_path, governance) {
        let _ = write_bytes_atomic(record_path, old_record_bytes);
        return Err(NetworkProfileError::Storage(format!(
            "治理审计保存失败，网络资料已回滚: {error:#}"
        )));
    }
    Ok(())
}

fn map_network_profile_load_error(error: NetworkCreationError) -> NetworkProfileError {
    match error {
        NetworkCreationError::Invalid(message) => NetworkProfileError::Invalid(message),
        NetworkCreationError::DuplicateName(name)
        | NetworkCreationError::DuplicateId(name)
        | NetworkCreationError::PermissionDenied(name) => NetworkProfileError::Invalid(name),
        NetworkCreationError::Storage(message) => {
            if message.contains("No such file") || message.contains("os error 2") {
                NetworkProfileError::NotFound(message)
            } else {
                NetworkProfileError::Storage(message)
            }
        }
    }
}

fn sorted_network_names(list: &NetworkList) -> Vec<String> {
    let mut names = list.networks.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let content = serde_json::to_vec_pretty(value).context("无法序列化 JSON")?;
    write_bytes_atomic(path, &content)
}

fn write_bytes_atomic(path: &Path, content: &[u8]) -> Result<()> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("目标文件没有父目录: {}", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("目标文件没有文件名: {}", path.display()))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary_path = parent.join(temporary_name);
    let write_result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .with_context(|| format!("无法创建临时文件: {}", temporary_path.display()))?;
        file.write_all(content)
            .with_context(|| format!("无法写入临时文件: {}", temporary_path.display()))?;
        file.sync_all()
            .with_context(|| format!("无法同步临时文件: {}", temporary_path.display()))?;
        std::fs::rename(&temporary_path, path)
            .with_context(|| format!("无法替换文件: {}", path.display()))?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("无法同步目录: {}", parent.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

fn normalize_display_name(display_name: Option<String>) -> Result<Option<String>> {
    let Some(display_name) = display_name else {
        return Ok(None);
    };
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Ok(None);
    }
    if display_name.chars().any(char::is_control) {
        anyhow::bail!("节点显示名不能包含控制字符");
    }
    if display_name.chars().count() > 80 {
        anyhow::bail!("节点显示名不能超过 80 个字符");
    }
    Ok(Some(display_name.to_string()))
}

fn url_to_network_slug(value: &str) -> String {
    let mut slug = String::new();

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            slug.push(ch);
        } else if matches!(ch, ':' | ' ' | '\t') && !slug.ends_with('-') {
            slug.push('-');
        }
    }

    let slug = slug.trim_matches(['.', '-', '_']).to_string();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use fuckipv6_common::domain::{
        CapabilityKind, ExitContributionObligation, ExitContributionScope, JoinTermsAck,
        MembershipRole, MembershipState, MembershipTerms, NetworkJoinMode, NetworkVisibility,
    };

    use super::*;

    fn temp_config_dir(name: &str) -> PathBuf {
        let unique = format!("fuckipv6-config-manager-{}-{}", name, std::process::id());
        std::env::temp_dir().join(unique)
    }

    fn owned_profile(name: &str) -> NetworkCreateProfile {
        NetworkCreateProfile {
            name: name.into(),
            purpose: "A private family mesh".into(),
            icon: "home".into(),
            language: "zh-CN".into(),
            region: "CN".into(),
            tags: vec!["family".into()],
            capability_declarations: Vec::new(),
            visibility: NetworkVisibility::Unlisted,
            join_mode: NetworkJoinMode::InviteOnly,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
        }
    }

    fn accepted_membership(seed: &str) -> NetworkMembership {
        let network_id = NetworkId::from_public_key(seed.as_bytes());
        let node_id = DomainNodeId::from_public_key(b"local-node");
        let terms = MembershipTerms {
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: ExitContributionScope::JoiningMembers,
            member_limit: Some(25),
        };
        let now = 1_000;
        let mut membership =
            NetworkMembership::new(network_id, node_id, MembershipRole::Member, now);
        membership
            .begin_apply(
                terms.clone(),
                JoinTermsAck {
                    terms_hash: terms.content_hash().expect("terms hash"),
                    acknowledged_at_unix_ms: now,
                },
                now,
            )
            .expect("begin membership application");
        membership.accept(now + 1).expect("accept membership");
        membership
    }

    #[test]
    fn creates_owner_network_with_independent_identity_and_governance() -> anyhow::Result<()> {
        let dir = temp_config_dir("owner-create");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let created = manager
            .create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)
            .expect("create owned network");

        assert!(created.config_path.is_file());
        assert!(manager
            .get_network_dir("home")
            .join(NETWORK_IDENTITY_FILE)
            .is_file());
        assert!(manager
            .get_network_dir("home")
            .join(NETWORK_RECORD_FILE)
            .is_file());
        assert!(manager
            .get_network_dir("home")
            .join(NETWORK_GOVERNANCE_FILE)
            .is_file());
        assert_eq!(created.record.owner, created.record.members[0]);
        assert!(created.record.administrators.is_empty());
        assert_eq!(created.record.revision, 1);
        assert_eq!(created.record.policy_revision, 1);

        let persisted = manager.load_network_record("home")?;
        assert_eq!(persisted.network_id, created.record.network_id);
        let listed = manager
            .list_networks()?
            .into_iter()
            .find(|network| network.name == "home")
            .expect("created network in list");
        assert_eq!(
            listed.network_id.as_deref(),
            Some(persisted.network_id.as_str())
        );
        assert_eq!(
            listed.public_key.as_deref(),
            Some(persisted.public_key.as_str())
        );

        let governance: AuditLog = serde_json::from_str(&std::fs::read_to_string(
            manager
                .get_network_dir("home")
                .join(NETWORK_GOVERNANCE_FILE),
        )?)?;
        assert_eq!(governance.entries().len(), 1);
        assert_eq!(governance.entries()[0].action, "network.create");
        assert_eq!(
            governance.entries()[0].network_id.as_ref(),
            Some(&created.record.network_id)
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                std::fs::metadata(manager.get_network_dir("home").join(NETWORK_IDENTITY_FILE))?
                    .permissions()
                    .mode()
                    & 0o777;
            assert_eq!(mode, 0o600);
        }

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        assert_eq!(
            reopened.load_network_record("home")?.network_id,
            created.record.network_id
        );
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn profile_preview_is_read_only_and_revision_conflict_does_not_write() -> anyhow::Result<()> {
        let dir = temp_config_dir("profile-preview");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let created = manager
            .create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)
            .expect("create owned network");
        let record_path = manager.network_record_path("home")?;
        let governance_path = manager.network_governance_path("home")?;
        let record_before = std::fs::read(&record_path)?;
        let governance_before = std::fs::read(&governance_path)?;
        let update = NetworkProfileUpdate {
            purpose: "Preview only".into(),
            region: "JP".into(),
            tags: vec!["Tokyo".into(), "Asia".into()],
            capability_declarations: vec![CapabilityKind::Relay],
            ..NetworkProfileUpdate::from_record(&created.record)
        };

        let preview = manager
            .preview_network_profile("home", created.record.revision, update.clone())
            .expect("preview profile");
        assert_eq!(preview.candidate_revision, created.record.revision + 1);
        assert_eq!(
            preview.projections.administrator_view.revision,
            preview.candidate_revision
        );
        assert!(preview.diff.iter().any(|entry| entry.path == "purpose"));
        assert_eq!(std::fs::read(&record_path)?, record_before);
        assert_eq!(std::fs::read(&governance_path)?, governance_before);
        assert!(!manager
            .get_network_dir("home")
            .join(".profile.lock")
            .exists());

        assert!(matches!(
            manager.update_network_profile(
                "home",
                0,
                update,
                &DomainNodeId::from_public_key(&manager.load_node_identity()?.public_key_bytes(),)
                    .to_string(),
                "profile-test",
                None,
            ),
            Err(NetworkProfileError::RevisionConflict {
                expected: 0,
                actual: 1
            })
        ));
        assert_eq!(std::fs::read(&record_path)?, record_before);
        assert_eq!(std::fs::read(&governance_path)?, governance_before);

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn profile_update_resigns_persists_and_audits_operation_id() -> anyhow::Result<()> {
        let dir = temp_config_dir("profile-update");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let created = manager
            .create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)
            .expect("create owned network");
        let actor =
            DomainNodeId::from_public_key(&manager.load_node_identity()?.public_key_bytes())
                .to_string();
        let operation_id = OperationId::from_public_key(b"profile-operation");
        let update = NetworkProfileUpdate {
            purpose: "Updated public purpose".into(),
            icon: "mesh".into(),
            language: "ja".into(),
            region: "JP".into(),
            tags: vec!["Tokyo".into(), "Asia".into()],
            capability_declarations: vec![CapabilityKind::Exit, CapabilityKind::Relay],
            visibility: NetworkVisibility::Public,
            join_mode: NetworkJoinMode::ApprovalRequired,
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
        };

        let result = manager
            .update_network_profile(
                "home",
                created.record.revision,
                update,
                &actor,
                "webui",
                Some(operation_id.clone()),
            )
            .expect("update profile");
        assert_eq!(result.previous_revision, 1);
        assert_eq!(result.revision, 2);
        assert_eq!(result.audit_action, "network.profile.update");
        assert!(result
            .changed_paths
            .iter()
            .any(|path| path == "capability_declarations"));
        result.signed_profile.validate()?;
        assert_eq!(result.signed_profile.document.revision, 2);
        assert_eq!(result.signed_profile.document.region, "JP");
        assert_eq!(
            result.signed_profile.document.tags,
            vec!["asia".to_string(), "tokyo".to_string()]
        );
        assert!(
            result.signed_profile.document.expires_at_unix_ms
                > result.signed_profile.document.issued_at_unix_ms
        );

        let persisted = manager.load_network_record("home")?;
        assert_eq!(persisted.revision, 2);
        assert_eq!(persisted.purpose, "Updated public purpose");
        persisted
            .signed_profile
            .as_ref()
            .expect("signed")
            .validate()?;

        let governance: AuditLog =
            serde_json::from_slice(&std::fs::read(manager.network_governance_path("home")?)?)?;
        assert_eq!(governance.entries().len(), 2);
        let audit = governance.entries().last().expect("profile audit");
        assert_eq!(audit.action, "network.profile.update");
        assert_eq!(audit.operation_id.as_ref(), Some(&operation_id));
        assert_eq!(audit.actor.as_str(), actor);

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn unchanged_expired_profile_is_resigned_without_revision_bump() -> anyhow::Result<()> {
        let dir = temp_config_dir("profile-resign");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let created = manager
            .create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)
            .expect("create owned network");
        let identity = Identity::load_from_file(&manager.network_identity_path("home")?)?;
        let mut expired = created.record.clone();
        expired.sign_public_profile(&identity, 1, 2)?;
        write_json_atomic(&manager.network_record_path("home")?, &expired)?;

        let result = manager
            .update_network_profile(
                "home",
                expired.revision,
                NetworkProfileUpdate::from_record(&expired),
                &DomainNodeId::from_public_key(&manager.load_node_identity()?.public_key_bytes())
                    .to_string(),
                "profile-test",
                None,
            )
            .expect("resign profile");
        assert_eq!(result.previous_revision, 1);
        assert_eq!(result.revision, 1);
        assert!(result.changed_paths.is_empty());
        assert!(result.signed_profile.document.expires_at_unix_ms > 2);
        assert!(!result.warnings.iter().any(|warning| {
            warning.code == fuckipv6_common::domain::NetworkProfileWarningCode::ProfileExpired
        }));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn profile_record_is_rolled_back_when_audit_write_fails() -> anyhow::Result<()> {
        let dir = temp_config_dir("profile-audit-rollback");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let created = manager
            .create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)
            .expect("create owned network");
        let record_path = manager.network_record_path("home")?;
        let governance_path = manager.network_governance_path("home")?;
        let old_bytes = std::fs::read(&record_path)?;
        let identity = Identity::load_from_file(&manager.network_identity_path("home")?)?;
        let mut updated = created.record.clone();
        updated.purpose = "Must roll back".into();
        updated.revision += 1;
        updated.sign_public_profile(&identity, unix_time_ms().max(1), u64::MAX)?;
        let governance: AuditLog = serde_json::from_slice(&std::fs::read(&governance_path)?)?;

        std::fs::remove_file(&governance_path)?;
        std::fs::create_dir(&governance_path)?;
        let error = write_network_profile_record_and_audit(
            &record_path,
            &old_bytes,
            &updated,
            &governance_path,
            &governance,
        )
        .expect_err("audit directory should reject atomic file replacement");
        assert!(
            matches!(error, NetworkProfileError::Storage(message) if message.contains("已回滚"))
        );
        assert_eq!(std::fs::read(&record_path)?, old_bytes);
        assert!(governance_path.is_dir());

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_name_and_duplicate_network_identity() -> anyhow::Result<()> {
        let dir = temp_config_dir("owner-create-duplicate");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let identity = Identity::generate()?;
        manager
            .create_owned_network_with_identity(
                owned_profile("first"),
                "ws://127.0.0.1:6666",
                true,
                identity.clone(),
            )
            .expect("first owner network");

        assert!(matches!(
            manager.create_owned_network_with_identity(
                owned_profile("first"),
                "ws://127.0.0.1:6667",
                false,
                Identity::generate()?,
            ),
            Err(NetworkCreationError::DuplicateName(name)) if name == "first"
        ));
        assert!(matches!(
            manager.create_owned_network_with_identity(
                owned_profile("second"),
                "ws://127.0.0.1:6667",
                false,
                identity,
            ),
            Err(NetworkCreationError::DuplicateId(id)) if id.starts_with("net_")
        ));
        assert!(!manager.network_exists("second"));
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn storage_failure_rolls_back_network_directory_and_list() -> anyhow::Result<()> {
        let dir = temp_config_dir("owner-create-storage-failure");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        std::fs::write(dir.join(".configuration"), b"not a directory")?;

        assert!(matches!(
            manager.create_owned_network(owned_profile("failed"), "ws://127.0.0.1:6666", true,),
            Err(NetworkCreationError::Storage(_))
        ));
        assert!(!manager.get_network_dir("failed").exists());
        assert!(!manager.network_exists("failed"));
        assert!(manager.list_networks()?.is_empty());
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn manages_multiple_networks_and_switches_default() -> anyhow::Result<()> {
        let dir = temp_config_dir("switch");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        let alpha = manager.create_network("alpha", "ws://alpha.example:6666", true)?;
        let beta = manager.create_network("beta", "wss://beta.example:6666", false)?;

        assert!(alpha.to_string_lossy().contains("alpha/config.toml"));
        assert!(beta.to_string_lossy().contains("beta/config.toml"));
        assert!(alpha
            .to_string_lossy()
            .contains(".configuration/generations"));
        assert_eq!(manager.configuration_document()?.revision, 3);
        assert_eq!(manager.get_default_network()?, Some("alpha".to_string()));

        manager.set_default_network("beta")?;
        assert_eq!(manager.get_default_network()?, Some("beta".to_string()));
        assert_eq!(manager.get_network_config("beta")?, beta);

        let networks = manager.list_networks()?;
        assert_eq!(networks.len(), 2);
        assert!(networks.iter().any(|network| network.name == "alpha"));
        assert!(networks.iter().any(|network| network.name == "beta"));

        manager.delete_network("beta")?;
        assert_eq!(manager.get_default_network()?, None);
        assert!(!manager.network_exists("beta"));
        let document = manager.configuration_document()?;
        assert_eq!(document.revision, 4);
        assert_eq!(document.networks.keys().collect::<Vec<_>>(), vec!["alpha"]);
        let history = manager.configuration_history()?;
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].revision, 4);
        assert_eq!(history[0].source, "config-manager");

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn reads_legacy_network_list_without_new_optional_fields() -> anyhow::Result<()> {
        let dir = temp_config_dir("legacy-network-list");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        std::fs::write(
            dir.join("networks.json"),
            r#"{
                "networks": {
                    "legacy": {
                        "name": "legacy",
                        "coordinator_url": "ws://legacy.example:6666",
                        "created_at": "2025-01-01T00:00:00Z",
                        "last_used": "2025-01-01T00:00:00Z"
                    }
                },
                "default_network": "legacy"
            }"#,
        )?;

        let networks = manager.list_networks()?;
        let legacy = networks
            .iter()
            .find(|network| network.name == "legacy")
            .expect("legacy network");
        assert!(!legacy.owns_embedded_services);
        assert_eq!(legacy.signaling_port, None);
        assert_eq!(legacy.relay_port, None);
        assert_eq!(legacy.external_host, None);
        assert_eq!(legacy.ip_pool, None);
        assert_eq!(manager.get_default_network()?, Some("legacy".into()));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_and_unsafe_network_names() -> anyhow::Result<()> {
        let dir = temp_config_dir("reject");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        manager.create_network("prod", "ws://prod.example:6666", true)?;
        assert!(manager
            .create_network("prod", "ws://other.example:6666", false)
            .is_err());
        assert!(manager
            .create_network("../escape", "ws://escape.example:6666", false)
            .is_err());
        assert!(!manager.network_exists("../escape"));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn creates_client_config_when_coordinator_state_directory_exists() -> anyhow::Result<()> {
        let dir = temp_config_dir("coordinator-dir");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let coordinator_config = manager.coordinator_config_path("hosted")?;
        std::fs::create_dir_all(coordinator_config.parent().expect("coordinator parent"))?;
        std::fs::write(&coordinator_config, "persisted coordinator state")?;

        let config = manager.create_network("hosted", "ws://127.0.0.1:6666", true)?;

        assert!(config.exists());
        assert!(coordinator_config.exists());
        assert!(manager.network_exists("hosted"));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn extracts_safe_network_names_from_urls() {
        assert_eq!(
            ConfigManager::extract_network_name("ws://vpn.example.com:6666"),
            "vpn.example"
        );
        assert_eq!(
            ConfigManager::extract_network_name("wss://example.com/connect"),
            "example"
        );
        assert_eq!(
            ConfigManager::extract_network_name("ws://[2001:db8::1]:6666"),
            "2001-db8-1"
        );
    }

    #[test]
    fn service_config_override_wins_over_desktop_environment() -> anyhow::Result<()> {
        let resolved = ConfigManager::resolve_config_dir(
            Some(OsString::from("/etc/fuckipv6")),
            Some(OsString::from("/home/operator/.config")),
            Some(OsString::from("/home/operator")),
        )?;
        assert_eq!(resolved, PathBuf::from("/etc/fuckipv6"));
        Ok(())
    }

    #[test]
    fn node_identity_and_display_name_are_stable_and_node_wide() -> anyhow::Result<()> {
        let dir = temp_config_dir("identity");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        let first = manager.load_node_identity()?;
        let first_id = first.client_id().to_string();
        let first_fingerprint = Identity::public_key_fingerprint(&first.public_key_bytes());
        assert!(manager.node_identity_path().exists());
        assert_eq!(manager.node_display_name()?, None);
        let created_at = manager.node_identity_created_at_unix_ms()?;
        assert!(created_at > 0);

        manager.set_node_display_name(Some("  edge-gateway  "))?;
        assert_eq!(manager.node_display_name()?, Some("edge-gateway".into()));

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        let second = reopened.load_node_identity()?;
        assert_eq!(second.client_id(), first_id);
        assert_eq!(
            Identity::public_key_fingerprint(&second.public_key_bytes()),
            first_fingerprint
        );
        assert_eq!(reopened.node_display_name()?, Some("edge-gateway".into()));
        assert_eq!(reopened.node_identity_created_at_unix_ms()?, created_at);

        let config_path = reopened.create_network("home", "ws://home.example:6666", true)?;
        let network_config = ClientConfig::from_file(config_path)?;
        assert_eq!(
            network_config.private_key_path,
            reopened.node_identity_path()
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn node_display_name_rejects_control_characters_and_can_be_cleared() -> anyhow::Result<()> {
        let dir = temp_config_dir("display-name");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        assert!(manager.set_node_display_name(Some("bad\nname")).is_err());
        manager.set_node_display_name(Some("gateway"))?;
        manager.set_node_display_name(Some("   "))?;
        assert_eq!(manager.node_display_name()?, None);

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn node_display_name_counts_unicode_code_points_and_enforces_the_limit() -> anyhow::Result<()> {
        let dir = temp_config_dir("display-name-unicode");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;

        manager.set_node_display_name(Some(&"界".repeat(80)))?;
        assert_eq!(manager.node_display_name()?, Some("界".repeat(80)));
        assert!(manager
            .set_node_display_name(Some(&"界".repeat(81)))
            .is_err());

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn language_preference_defaults_for_legacy_metadata_and_persists() -> anyhow::Result<()> {
        let dir = temp_config_dir("language-preference");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        std::fs::write(
            dir.join(NODE_METADATA_FILE),
            r#"{"display_name":"legacy-node"}"#,
        )?;

        assert_eq!(
            manager.node_language_preference()?,
            LanguagePreference::System
        );
        manager.set_node_display_name(Some("gateway"))?;
        manager.set_node_language_preference(LanguagePreference::ZhCn)?;

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        assert_eq!(reopened.node_display_name()?, Some("gateway".into()));
        assert_eq!(
            reopened.node_language_preference()?,
            LanguagePreference::ZhCn
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn legacy_metadata_gets_a_stable_identity_creation_timestamp() -> anyhow::Result<()> {
        let dir = temp_config_dir("identity-created-at");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        manager.load_node_identity()?;
        std::fs::write(
            dir.join(NODE_METADATA_FILE),
            r#"{"display_name":"legacy-node"}"#,
        )?;

        let first = manager.node_identity_created_at_unix_ms()?;
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(NODE_METADATA_FILE))?)?;
        assert_eq!(stored["identity_created_at_unix_ms"], first);

        std::thread::sleep(std::time::Duration::from_millis(20));
        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        assert_eq!(reopened.node_identity_created_at_unix_ms()?, first);
        assert_eq!(reopened.node_display_name()?, Some("legacy-node".into()));

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn accepted_memberships_are_idempotent_and_removed_by_stable_id() -> anyhow::Result<()> {
        let dir = temp_config_dir("memberships");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let alpha = accepted_membership("membership-alpha");
        let beta = accepted_membership("membership-beta");

        let (stored, inserted) = manager.upsert_joined_membership("alpha", alpha.clone())?;
        assert!(inserted);
        assert_eq!(stored.membership, alpha);
        let (replayed, inserted) = manager.upsert_joined_membership("alpha", alpha.clone())?;
        assert!(!inserted);
        assert_eq!(replayed, stored);
        manager.upsert_joined_membership("beta", beta.clone())?;

        assert_eq!(manager.list_joined_memberships()?.len(), 2);
        assert_eq!(
            manager
                .joined_membership(alpha.network_id.as_str())?
                .expect("alpha membership")
                .network_name,
            "alpha"
        );

        let removed = manager
            .remove_joined_membership(alpha.network_id.as_str())?
            .expect("removed alpha membership");
        assert_eq!(removed.network_name, "alpha");
        assert!(manager
            .joined_membership(alpha.network_id.as_str())?
            .is_none());
        assert_eq!(
            manager
                .joined_membership_for_network_name("beta")?
                .expect("beta membership remains")
                .membership,
            beta
        );

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        let memberships = reopened.list_joined_memberships()?;
        assert_eq!(memberships.len(), 1);
        assert_eq!(memberships[0].network_name, "beta");

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn policy_observations_are_atomic_and_acknowledgements_persist() -> anyhow::Result<()> {
        let dir = temp_config_dir("membership-policy-observation");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let joining = accepted_membership("policy-joining-members");
        let all_members = accepted_membership("policy-all-members");
        manager.upsert_joined_membership("joining", joining.clone())?;
        manager.upsert_joined_membership("all", all_members.clone())?;

        let mut joining_terms = joining.terms.clone().expect("joining terms");
        joining_terms.network_policy_revision += 1;
        let mut all_terms = all_members.terms.clone().expect("all terms");
        all_terms.network_policy_revision += 1;
        all_terms.exit_contribution_scope = ExitContributionScope::AllMembers;
        let current = [
            (
                joining.network_id.as_str().to_owned(),
                joining_terms.clone(),
            ),
            (
                all_members.network_id.as_str().to_owned(),
                all_terms.clone(),
            ),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();

        assert!(manager.observe_joined_membership_policies(&current, 1_100)?);
        let persisted_joining = manager
            .joined_membership(joining.network_id.as_str())?
            .expect("joining membership");
        let persisted_all = manager
            .joined_membership(all_members.network_id.as_str())?
            .expect("all-members membership");
        assert_eq!(
            persisted_joining.membership.current_terms.as_ref(),
            Some(&joining_terms)
        );
        assert!(!persisted_joining.membership.policy_confirmation_required());
        assert!(persisted_all.membership.policy_confirmation_required());

        let terms_hash = all_terms.content_hash()?;
        manager.acknowledge_joined_membership_policy(
            all_members.network_id.as_str(),
            &terms_hash,
            JoinTermsAck {
                terms_hash: terms_hash.clone(),
                acknowledged_at_unix_ms: 1_101,
            },
            1_101,
        )?;

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        let persisted_all = reopened
            .joined_membership(all_members.network_id.as_str())?
            .expect("reopened all-members membership");
        assert_eq!(persisted_all.membership.terms.as_ref(), Some(&all_terms));
        assert!(!persisted_all.membership.policy_confirmation_required());
        assert_eq!(
            persisted_all.membership.local_policy_revision,
            all_members.local_policy_revision + 1
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn membership_file_rejects_non_accepted_or_inconsistent_records() -> anyhow::Result<()> {
        let dir = temp_config_dir("membership-validation");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let accepted = accepted_membership("membership-valid");
        manager.upsert_joined_membership("valid", accepted.clone())?;

        let mut applying = accepted;
        applying.state = MembershipState::Applying;
        assert!(manager.upsert_joined_membership("valid", applying).is_err());

        let mut other_node = accepted_membership("membership-valid");
        other_node.node_id = DomainNodeId::from_public_key(b"other-node");
        assert!(manager
            .upsert_joined_membership("valid", other_node)
            .is_err());

        assert_eq!(manager.list_joined_memberships()?.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn concurrent_membership_updates_do_not_overwrite_each_other() -> anyhow::Result<()> {
        let dir = temp_config_dir("membership-concurrency");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut workers = Vec::new();
        for index in 0..8 {
            let manager = manager.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let seed = format!("membership-concurrent-{index}");
                barrier.wait();
                manager
                    .upsert_joined_membership(
                        &format!("network-{index}"),
                        accepted_membership(&seed),
                    )
                    .map(|(_, inserted)| inserted)
            }));
        }
        for worker in workers {
            assert!(worker.join().expect("membership worker")?);
        }

        let memberships = manager.list_joined_memberships()?;
        assert_eq!(memberships.len(), 8);
        assert_eq!(
            memberships
                .iter()
                .map(|membership| membership.network_name.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            (0..8)
                .map(|index| format!("network-{index}"))
                .collect::<std::collections::BTreeSet<_>>()
                .iter()
                .map(String::as_str)
                .collect()
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn stable_network_id_metadata_is_idempotent_and_cannot_be_rebound() -> anyhow::Result<()> {
        let dir = temp_config_dir("network-id-metadata");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone())?;
        let home =
            manager.create_owned_network(owned_profile("home"), "ws://127.0.0.1:6666", true)?;
        let lab =
            manager.create_owned_network(owned_profile("lab"), "ws://127.0.0.1:6667", false)?;

        manager.set_network_id_metadata("home", home.record.network_id.as_str())?;
        manager.set_network_id_metadata("home", home.record.network_id.as_str())?;
        assert_eq!(
            manager
                .load_network_list()?
                .networks
                .get("home")
                .and_then(|network| network.network_id.as_deref()),
            Some(home.record.network_id.as_str())
        );

        let original = manager.load_network_list()?;
        assert!(manager
            .set_network_id_metadata("home", lab.record.network_id.as_str())
            .is_err());
        assert!(manager
            .set_network_id_metadata("lab", home.record.network_id.as_str())
            .is_err());
        assert!(manager
            .set_network_id_metadata("missing", home.record.network_id.as_str())
            .is_err());
        let after_failures = manager.load_network_list()?;
        assert_eq!(
            after_failures.networks["home"].network_id,
            original.networks["home"].network_id
        );
        assert_eq!(
            after_failures.networks["lab"].network_id,
            original.networks["lab"].network_id
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }
}
