use super::audit::OwnerSecurityAudit;
use super::store::{CommitOutcome, OwnerLoad, OwnerPersistence, OwnerStore};
use super::webauthn::OwnerWebauthn;
use crate::config_manager::ConfigManager;
use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fuckipv6_common::domain::NodeId;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyRegistration, RegisterPublicKeyCredential, Url,
};
use zeroize::Zeroizing;

const CEREMONY_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_BOOTSTRAP_FAILURES: u32 = 5;
const BOOTSTRAP_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BOOTSTRAP_BACKOFF: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerBootstrapChannel {
    Local,
    Public,
}

impl OwnerBootstrapChannel {
    pub(crate) const fn audit_label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Public => "public",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BootstrapPhase {
    NoOwner,
    AwaitingPasskey,
    AwaitingRecoveryConfirmation,
    Complete,
}

impl BootstrapPhase {
    pub(crate) fn is_initialized(self) -> bool {
        self == Self::Complete
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BootstrapStatus {
    pub(crate) phase: BootstrapPhase,
    pub(crate) initialized: bool,
    pub(crate) origin: String,
}

#[derive(Error, Debug)]
pub(crate) enum OwnerSecurityError {
    #[error("所有者已完成初始化")]
    AlreadyInitialized,
    #[error("初始化码无效")]
    InvalidBootstrapCode,
    #[error("初始化尝试过多，请稍后重试")]
    RateLimited,
    #[error("已有 Passkey 注册流程正在进行")]
    CeremonyInProgress,
    #[error("Passkey 注册流程已过期或不存在")]
    CeremonyMissing,
    #[error("必须先确认已离线保存恢复码")]
    RecoveryNotConfirmed,
    #[error("所有者安全状态失败关闭")]
    CorruptState,
    #[error("所有者安全操作失败")]
    Internal(#[from] anyhow::Error),
}

type ServiceResult<T> = std::result::Result<T, OwnerSecurityError>;

struct AttemptState {
    failures: u32,
    blocked_until: Option<Instant>,
}

enum State {
    NoOwner,
    AwaitingPasskey {
        registration: PasskeyRegistration,
        channel: OwnerBootstrapChannel,
        started_at: Instant,
    },
    AwaitingRecoveryConfirmation {
        candidate: OwnerStore,
        recovery_verifier: [u8; 32],
        channel: OwnerBootstrapChannel,
        started_at: Instant,
    },
    Complete,
}

pub(crate) struct OwnerSecurityService {
    persistence: OwnerPersistence,
    audit: OwnerSecurityAudit,
    local_webauthn: OwnerWebauthn,
    public_webauthn: Option<OwnerWebauthn>,
    node_id: NodeId,
    display_name: String,
    state: Mutex<State>,
    attempts: Mutex<AttemptState>,
}

impl OwnerSecurityService {
    /// Initialize the owner bootstrap boundary for a running WebUI.
    ///
    /// When no owner exists, this rotates the one-time bootstrap verifier and
    /// returns the plaintext code for the caller to display once on the local
    /// terminal. Incomplete WebAuthn ceremonies never survive a process
    /// restart because registration state is held only in memory.
    #[cfg(test)]
    pub(crate) fn from_config(
        config: &ConfigManager,
        port: u16,
    ) -> Result<(Self, Option<Zeroizing<String>>)> {
        Self::from_config_with_public_origin(config, port, None)
    }

    /// Initialize the owner bootstrap boundary with both local and verified
    /// public WebAuthn origins. A public origin is optional and is only supplied
    /// after the surrounding exposure policy has validated it.
    pub(crate) fn from_config_with_public_origin(
        config: &ConfigManager,
        port: u16,
        public_origin: Option<&Url>,
    ) -> Result<(Self, Option<Zeroizing<String>>)> {
        let identity = config.load_node_identity()?;
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        let display_name = config
            .node_display_name()?
            .unwrap_or_else(|| "本机所有者".to_string());
        let persistence = OwnerPersistence::new(config.config_dir().to_path_buf())?;
        let audit = OwnerSecurityAudit::new(config.config_dir())?;
        let (state, bootstrap_code) = match persistence.load()? {
            OwnerLoad::Uninitialized => {
                let code = generate_secret()?;
                persistence
                    .provision_bootstrap_code(&code)
                    .context("无法生成本地所有者初始化上下文")?;
                (State::NoOwner, Some(code))
            }
            OwnerLoad::Complete(store) => {
                validate_owner_node(&store, &node_id)?;
                persistence
                    .consume_bootstrap_code()
                    .context("无法清理已完成的初始化码验证器")?;
                (State::Complete, None)
            }
            OwnerLoad::Corrupt(reason) => {
                anyhow::bail!("所有者安全状态损坏，拒绝重新初始化: {reason}")
            }
        };
        let local_webauthn = OwnerWebauthn::new(port)?;
        let public_webauthn = public_origin.map(OwnerWebauthn::public).transpose()?;
        Ok((
            Self {
                persistence,
                audit,
                local_webauthn,
                public_webauthn,
                node_id,
                display_name,
                state: Mutex::new(state),
                attempts: Mutex::new(AttemptState {
                    failures: 0,
                    blocked_until: None,
                }),
            },
            bootstrap_code,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_bootstrap_audit(
        &self,
        channel: Option<OwnerBootstrapChannel>,
        source_ip: Option<IpAddr>,
        method: &str,
        path: &str,
        status: Option<u16>,
        outcome: &str,
    ) -> Result<()> {
        self.audit.record(
            channel.map_or("unknown", OwnerBootstrapChannel::audit_label),
            source_ip,
            method,
            path,
            status,
            outcome,
        )
    }

    #[cfg(test)]
    pub(crate) fn audit_path(&self) -> &std::path::Path {
        self.audit.path()
    }

    pub(crate) fn status(&self) -> ServiceResult<BootstrapStatus> {
        self.status_for(OwnerBootstrapChannel::Local)
    }

    pub(crate) fn status_for(
        &self,
        channel: OwnerBootstrapChannel,
    ) -> ServiceResult<BootstrapStatus> {
        let webauthn = self.webauthn_for(channel)?;
        let persisted = self
            .persistence
            .load()
            .map_err(OwnerSecurityError::Internal)?;
        let phase = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            match persisted {
                OwnerLoad::Uninitialized => {
                    // An owner that disappeared from disk must never be
                    // reopened as an uninitialized node in this process.
                    if matches!(*state, State::Complete) {
                        return Err(OwnerSecurityError::CorruptState);
                    }
                }
                OwnerLoad::Complete(store) => {
                    validate_owner_node(&store, &self.node_id)
                        .map_err(OwnerSecurityError::Internal)?;
                    *state = State::Complete;
                }
                OwnerLoad::Corrupt(_) => return Err(OwnerSecurityError::CorruptState),
            }
            expire_ceremony(&mut state, Instant::now());
            phase_of(&state)
        };
        Ok(BootstrapStatus {
            phase,
            initialized: phase.is_initialized(),
            origin: webauthn.origin().to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn verify_bootstrap_code(&self, code: &str) -> ServiceResult<()> {
        self.verify_bootstrap_code_for(OwnerBootstrapChannel::Local, code)
    }

    pub(crate) fn verify_bootstrap_code_for(
        &self,
        channel: OwnerBootstrapChannel,
        code: &str,
    ) -> ServiceResult<()> {
        self.webauthn_for(channel)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| OwnerSecurityError::CorruptState)?;
        expire_ceremony(&mut state, Instant::now());
        if matches!(*state, State::Complete) {
            return Err(OwnerSecurityError::AlreadyInitialized);
        }
        self.check_bootstrap_code_locked(code)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn start_registration(
        &self,
        code: &str,
    ) -> ServiceResult<CreationChallengeResponse> {
        self.start_registration_for(OwnerBootstrapChannel::Local, code)
    }

    pub(crate) fn start_registration_for(
        &self,
        channel: OwnerBootstrapChannel,
        code: &str,
    ) -> ServiceResult<CreationChallengeResponse> {
        let webauthn = self.webauthn_for(channel)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| OwnerSecurityError::CorruptState)?;
        expire_ceremony(&mut state, Instant::now());
        match &*state {
            State::Complete => return Err(OwnerSecurityError::AlreadyInitialized),
            State::AwaitingPasskey { .. } | State::AwaitingRecoveryConfirmation { .. } => {
                return Err(OwnerSecurityError::CeremonyInProgress)
            }
            State::NoOwner => {}
        }

        // Another process may have completed initialization since this
        // process started. Never trust the in-memory state for ownership.
        match self.persistence.load()? {
            OwnerLoad::Uninitialized => {}
            OwnerLoad::Complete(store) => {
                validate_owner_node(&store, &self.node_id)?;
                *state = State::Complete;
                return Err(OwnerSecurityError::AlreadyInitialized);
            }
            OwnerLoad::Corrupt(_) => return Err(OwnerSecurityError::CorruptState),
        }
        self.check_bootstrap_code_locked(code)?;
        let (challenge, registration) = webauthn.start(&self.node_id, &self.display_name)?;
        *state = State::AwaitingPasskey {
            registration,
            channel,
            started_at: Instant::now(),
        };
        Ok(challenge)
    }

    pub(crate) fn finish_registration_for(
        &self,
        channel: OwnerBootstrapChannel,
        response: &RegisterPublicKeyCredential,
    ) -> ServiceResult<String> {
        let registration = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            expire_ceremony(&mut state, Instant::now());
            let State::AwaitingPasskey {
                channel: ceremony_channel,
                ..
            } = &*state
            else {
                return Err(OwnerSecurityError::CeremonyMissing);
            };
            if *ceremony_channel != channel {
                return Err(OwnerSecurityError::CeremonyMissing);
            }
            let State::AwaitingPasskey { registration, .. } =
                std::mem::replace(&mut *state, State::NoOwner)
            else {
                unreachable!("ceremony channel was checked while holding the state lock");
            };
            registration
        };
        let passkey = self
            .webauthn_for(channel)?
            .finish(response, &registration)?;
        self.finish_with_passkey(passkey, channel)
    }

    fn finish_with_passkey(
        &self,
        passkey: Passkey,
        channel: OwnerBootstrapChannel,
    ) -> ServiceResult<String> {
        let created_at_unix_ms = unix_time_ms()?;
        let recovery_code = generate_secret()?;
        let recovery_verifier = secret_verifier(&recovery_code);
        let flags = passkey_flags(&passkey)?;
        let candidate = OwnerStore::new(
            self.node_id.clone(),
            self.display_name.clone(),
            passkey,
            flags.backup_eligible,
            flags.backup_state,
            created_at_unix_ms,
        )?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| OwnerSecurityError::CorruptState)?;
        *state = State::AwaitingRecoveryConfirmation {
            candidate,
            recovery_verifier,
            channel,
            started_at: Instant::now(),
        };
        Ok(recovery_code.to_string())
    }

    #[cfg(test)]
    pub(crate) fn confirm_recovery(&self, confirmed: bool) -> ServiceResult<()> {
        self.confirm_recovery_for(OwnerBootstrapChannel::Local, confirmed)
    }

    pub(crate) fn confirm_recovery_for(
        &self,
        channel: OwnerBootstrapChannel,
        confirmed: bool,
    ) -> ServiceResult<()> {
        if !confirmed {
            return Err(OwnerSecurityError::RecoveryNotConfirmed);
        }
        let (candidate, recovery_verifier, ceremony_channel) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            expire_ceremony(&mut state, Instant::now());
            match &*state {
                State::AwaitingRecoveryConfirmation {
                    candidate,
                    recovery_verifier,
                    channel: ceremony_channel,
                    ..
                } => (candidate.clone(), *recovery_verifier, *ceremony_channel),
                State::Complete => return Err(OwnerSecurityError::AlreadyInitialized),
                _ => return Err(OwnerSecurityError::CeremonyMissing),
            }
        };
        if ceremony_channel != channel {
            return Err(OwnerSecurityError::CeremonyMissing);
        }

        let outcome = self.persistence.commit(&candidate, &recovery_verifier)?;

        // Once the owner record is durably committed, initialization must be
        // permanently closed in memory even if best-effort verifier cleanup
        // subsequently fails.
        let mut state = self
            .state
            .lock()
            .map_err(|_| OwnerSecurityError::CorruptState)?;
        *state = State::Complete;
        drop(state);

        if let Err(error) = self.persistence.consume_bootstrap_code() {
            return Err(OwnerSecurityError::Internal(error));
        }

        match outcome {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::AlreadyInitialized => Err(OwnerSecurityError::AlreadyInitialized),
        }
    }

    fn webauthn_for(&self, channel: OwnerBootstrapChannel) -> ServiceResult<&OwnerWebauthn> {
        match channel {
            OwnerBootstrapChannel::Local => Ok(&self.local_webauthn),
            OwnerBootstrapChannel::Public => self
                .public_webauthn
                .as_ref()
                .ok_or(OwnerSecurityError::CorruptState),
        }
    }

    fn check_bootstrap_code_locked(&self, code: &str) -> ServiceResult<()> {
        let now = Instant::now();
        {
            let mut attempts = self
                .attempts
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            if attempts.blocked_until.is_some_and(|until| until > now) {
                return Err(OwnerSecurityError::RateLimited);
            }
            if attempts.blocked_until.is_some_and(|until| until <= now) {
                attempts.blocked_until = None;
            }
        }

        if self.persistence.verify_bootstrap_code(code)? {
            let mut attempts = self
                .attempts
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            attempts.failures = 0;
            attempts.blocked_until = None;
            return Ok(());
        }

        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| OwnerSecurityError::CorruptState)?;
        attempts.failures = attempts.failures.saturating_add(1);
        if attempts.failures >= MAX_BOOTSTRAP_FAILURES {
            let exponent = attempts
                .failures
                .saturating_sub(MAX_BOOTSTRAP_FAILURES)
                .min(8);
            let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
            let backoff = BOOTSTRAP_BACKOFF
                .saturating_mul(multiplier)
                .min(MAX_BOOTSTRAP_BACKOFF);
            attempts.blocked_until = Some(now + backoff);
        }
        Err(OwnerSecurityError::InvalidBootstrapCode)
    }

    #[cfg(test)]
    pub(crate) fn finish_registration_for_test(&self, passkey: Passkey) -> ServiceResult<String> {
        let channel = {
            let state = self
                .state
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            let State::AwaitingPasskey { channel, .. } = &*state else {
                return Err(OwnerSecurityError::CeremonyMissing);
            };
            *channel
        };
        self.finish_registration_for_test_channel(channel, passkey)
    }

    #[cfg(test)]
    pub(crate) fn finish_registration_for_test_channel(
        &self,
        requested_channel: OwnerBootstrapChannel,
        passkey: Passkey,
    ) -> ServiceResult<String> {
        let channel = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| OwnerSecurityError::CorruptState)?;
            let State::AwaitingPasskey { channel, .. } = &*state else {
                return Err(OwnerSecurityError::CeremonyMissing);
            };
            if *channel != requested_channel {
                return Err(OwnerSecurityError::CeremonyMissing);
            }
            let channel = *channel;
            *state = State::NoOwner;
            channel
        };
        self.finish_with_passkey(passkey, channel)
    }

    #[cfg(test)]
    fn expire_ceremony_for_test(&self) {
        let mut state = self.state.lock().expect("owner service state");
        let expired = Instant::now()
            .checked_sub(CEREMONY_TTL + Duration::from_secs(1))
            .expect("test clock");
        match &mut *state {
            State::AwaitingPasskey { started_at, .. }
            | State::AwaitingRecoveryConfirmation { started_at, .. } => *started_at = expired,
            _ => {}
        }
    }
}

struct PasskeyFlags {
    backup_eligible: bool,
    backup_state: bool,
}

fn passkey_flags(passkey: &Passkey) -> Result<PasskeyFlags> {
    let value = serde_json::to_value(passkey).context("无法读取 Passkey 安全属性")?;
    let credential = value.get("cred").unwrap_or(&value);
    Ok(PasskeyFlags {
        backup_eligible: credential
            .get("backup_eligible")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        backup_state: credential
            .get("backup_state")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn validate_owner_node(store: &OwnerStore, expected: &NodeId) -> Result<()> {
    if &store.owner.node_id != expected {
        anyhow::bail!("所有者记录与当前节点身份不匹配");
    }
    Ok(())
}

fn phase_of(state: &State) -> BootstrapPhase {
    match state {
        State::NoOwner => BootstrapPhase::NoOwner,
        State::AwaitingPasskey { .. } => BootstrapPhase::AwaitingPasskey,
        State::AwaitingRecoveryConfirmation { .. } => BootstrapPhase::AwaitingRecoveryConfirmation,
        State::Complete => BootstrapPhase::Complete,
    }
}

fn expire_ceremony(state: &mut State, now: Instant) {
    let expired = match &*state {
        State::AwaitingPasskey { started_at, .. }
        | State::AwaitingRecoveryConfirmation { started_at, .. } => now
            .checked_duration_since(*started_at)
            .is_some_and(|age| age >= CEREMONY_TTL),
        _ => false,
    };
    if expired {
        *state = State::NoOwner;
    }
}

fn generate_secret() -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut()).context("无法生成所有者安全凭据")?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

fn secret_verifier(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn unix_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("系统时间超出范围")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner_security::store::fixture_passkey;
    use fuckipv6_common::Identity;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    fn service(suffix: &str) -> Result<(OwnerSecurityService, Zeroizing<String>, PathBuf)> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-service-{suffix}-{}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let persistence = OwnerPersistence::new(dir.clone())?;
        let code = generate_secret()?;
        persistence.provision_bootstrap_code(&code)?;
        let service = OwnerSecurityService {
            persistence,
            audit: OwnerSecurityAudit::new(&dir)?,
            local_webauthn: OwnerWebauthn::new(8080)?,
            public_webauthn: None,
            node_id: NodeId::from_public_key(b"owner-service-test"),
            display_name: "Owner".to_string(),
            state: Mutex::new(State::NoOwner),
            attempts: Mutex::new(AttemptState {
                failures: 0,
                blocked_until: None,
            }),
        };
        Ok((service, code, dir))
    }

    #[test]
    fn full_local_bootstrap_is_atomic_and_recovery_is_single_use() -> Result<()> {
        let (service, code, dir) = service("complete")?;
        assert!(matches!(
            service.verify_bootstrap_code("wrong"),
            Err(OwnerSecurityError::InvalidBootstrapCode)
        ));
        service
            .verify_bootstrap_code(&code)
            .expect("correct bootstrap code");
        service
            .start_registration(&code)
            .expect("registration challenge");
        assert_eq!(
            service.status().expect("status").phase,
            BootstrapPhase::AwaitingPasskey
        );

        let recovery_code = service
            .finish_registration_for_test(fixture_passkey())
            .expect("recovery code");
        assert!(!recovery_code.is_empty());
        assert_eq!(
            service.status().expect("status").phase,
            BootstrapPhase::AwaitingRecoveryConfirmation
        );
        assert!(matches!(
            service.finish_registration_for_test(fixture_passkey()),
            Err(OwnerSecurityError::CeremonyMissing)
        ));
        assert!(matches!(
            service.confirm_recovery(false),
            Err(OwnerSecurityError::RecoveryNotConfirmed)
        ));
        service.confirm_recovery(true).expect("commit owner");
        assert_eq!(
            service.status().expect("status").phase,
            BootstrapPhase::Complete
        );
        assert!(matches!(
            service.start_registration(&code),
            Err(OwnerSecurityError::AlreadyInitialized)
        ));

        let loaded = OwnerPersistence::new(dir.clone())?.load()?;
        assert!(matches!(loaded, OwnerLoad::Complete(_)));
        for entry in walkdir_files(&dir) {
            let bytes = std::fs::read(&entry)?;
            assert!(
                !contains_subslice(&bytes, recovery_code.as_bytes()),
                "recovery code was persisted in {}",
                entry.display()
            );
            assert!(
                !contains_subslice(&bytes, code.as_bytes()),
                "bootstrap code was persisted in {}",
                entry.display()
            );
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn interrupted_or_expired_ceremonies_do_not_create_an_owner() -> Result<()> {
        let (service, code, dir) = service("interrupted")?;
        service
            .start_registration(&code)
            .expect("registration challenge");
        service.expire_ceremony_for_test();
        assert_eq!(
            service.status().expect("status").phase,
            BootstrapPhase::NoOwner
        );
        assert!(matches!(
            service.finish_registration_for_test(fixture_passkey()),
            Err(OwnerSecurityError::CeremonyMissing)
        ));
        service
            .start_registration(&code)
            .expect("retry after expiry");
        drop(service);

        let persistence = OwnerPersistence::new(dir.clone())?;
        assert!(matches!(persistence.load()?, OwnerLoad::Uninitialized));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn completed_owner_survives_restart_and_permanently_closes_initialization() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-restart-{}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let config = ConfigManager::new_for_dir(dir.clone())?;
        let (service, code) = OwnerSecurityService::from_config(&config, 8080)?;
        let code = code.expect("initial bootstrap code");
        service.start_registration(&code)?;
        service.finish_registration_for_test(fixture_passkey())?;
        service.confirm_recovery(true)?;
        drop(service);

        let (reopened, bootstrap_code) = OwnerSecurityService::from_config(&config, 8080)?;
        assert!(bootstrap_code.is_none());
        assert_eq!(reopened.status()?.phase, BootstrapPhase::Complete);
        assert!(matches!(
            reopened.start_registration(&code),
            Err(OwnerSecurityError::AlreadyInitialized)
        ));
        match OwnerPersistence::new(dir.clone())?.load()? {
            OwnerLoad::Complete(store) => assert_eq!(store.passkeys.len(), 1),
            _ => panic!("committed owner must survive restart"),
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn owner_bound_to_a_different_node_fails_closed() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-identity-{}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let config = ConfigManager::new_for_dir(dir.clone())?;
        let first_identity = config.load_node_identity()?;
        let first_node = NodeId::from_public_key(&first_identity.public_key_bytes());
        let store = OwnerStore::new(
            first_node,
            "Owner".to_string(),
            fixture_passkey(),
            false,
            false,
            1_700_000_000_000,
        )?;
        OwnerPersistence::new(dir.clone())?.commit(&store, &[7u8; 32])?;

        let second_identity = Identity::generate()?;
        second_identity.save_to_file(config.node_identity_path())?;
        let error = OwnerSecurityService::from_config(&config, 8080)
            .err()
            .expect("owner bound to another node must not load");
        assert!(error.to_string().contains("身份不匹配"));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn concurrent_recovery_confirmation_commits_exactly_once() -> Result<()> {
        let (service, code, dir) = service("concurrent-confirm")?;
        service.start_registration(&code)?;
        service.finish_registration_for_test(fixture_passkey())?;
        let service = Arc::new(service);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let service = service.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    service.confirm_recovery(true)
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("confirmation thread"))
            .collect();
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|result| matches!(result, Err(OwnerSecurityError::AlreadyInitialized)))
                .count(),
            1
        );
        assert_eq!(service.status()?.phase, BootstrapPhase::Complete);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn concurrent_local_and_public_claim_has_exactly_one_registration_winner() -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-channel-race-{}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let config = ConfigManager::new_for_dir(dir.clone())?;
        let public_origin = Url::parse("https://relay.xlei.cc")?;
        let (service, code) = OwnerSecurityService::from_config_with_public_origin(
            &config,
            8080,
            Some(&public_origin),
        )?;
        let code = Arc::new(code.expect("initial bootstrap code"));
        let service = Arc::new(service);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [OwnerBootstrapChannel::Local, OwnerBootstrapChannel::Public]
            .into_iter()
            .map(|channel| {
                let service = service.clone();
                let barrier = barrier.clone();
                let code = code.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let outcome = service
                        .start_registration_for(channel, code.as_str())
                        .map(|_| ());
                    (channel, outcome)
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("registration thread"))
            .collect();

        let winners: Vec<_> = outcomes
            .iter()
            .filter(|(_, outcome)| outcome.is_ok())
            .collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|(_, outcome)| {
                    matches!(outcome, Err(OwnerSecurityError::CeremonyInProgress))
                })
                .count(),
            1
        );

        let winner = winners[0].0;
        let loser = match winner {
            OwnerBootstrapChannel::Local => OwnerBootstrapChannel::Public,
            OwnerBootstrapChannel::Public => OwnerBootstrapChannel::Local,
        };
        assert!(matches!(
            service.finish_registration_for_test_channel(loser, fixture_passkey()),
            Err(OwnerSecurityError::CeremonyMissing)
        ));
        let recovery_code =
            service.finish_registration_for_test_channel(winner, fixture_passkey())?;
        assert!(!recovery_code.is_empty());

        drop(service);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn verifier_cleanup_failure_cannot_reopen_initialization() -> Result<()> {
        let (service, code, dir) = service("cleanup-failure")?;
        service.start_registration(&code)?;
        service.finish_registration_for_test(fixture_passkey())?;
        let verifier_path = service.persistence.bootstrap_verifier_path();
        std::fs::remove_file(&verifier_path)?;
        std::fs::create_dir(&verifier_path)?;

        assert!(matches!(
            service.confirm_recovery(true),
            Err(OwnerSecurityError::Internal(_))
        ));
        assert_eq!(service.status()?.phase, BootstrapPhase::Complete);
        assert!(matches!(
            service.start_registration(&code),
            Err(OwnerSecurityError::AlreadyInitialized)
        ));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    fn walkdir_files(root: &std::path::Path) -> Vec<PathBuf> {
        let mut output = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.is_file() {
                    output.push(path);
                }
            }
        }
        output
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }
}
