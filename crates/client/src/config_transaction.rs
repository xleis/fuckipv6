use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use fuckipv6_common::ClientConfig;
use ipnet::IpNet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONFIGURATION_SCHEMA_VERSION: u16 = 1;
const STORE_DIR: &str = ".configuration";
const STATE_FILE: &str = "state.json";
const GENERATIONS_DIR: &str = "generations";
const HISTORY_DIR: &str = "history";
const LOCK_FILE: &str = "lock";
const REDACTED_PATH: &str = "<redacted:path>";
const REDACTED_URL: &str = "<redacted:url>";

/// Complete, redacted configuration document returned to every UI adapter.
#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationDocument {
    pub schema_version: u16,
    pub revision: u64,
    /// Network name to complete TOML-equivalent JSON object.
    pub networks: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationImpact {
    /// The running data plane consumes this change without a process restart.
    Immediate,
    /// Persisted now; the affected network process must restart to consume it.
    RestartRequired,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationDiffEntry {
    /// Unambiguous JSON Pointer rooted at the configuration document.
    pub path: String,
    pub before: Value,
    pub after: Value,
    pub impact: ConfigurationImpact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationValidationCode {
    InvalidType,
    MissingRequired,
    UnknownNetwork,
    MissingNetwork,
    InvalidValue,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationValidationIssue {
    pub path: String,
    pub code: ConfigurationValidationCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationPreview {
    pub expected_revision: u64,
    pub current_revision: u64,
    pub candidate_revision: u64,
    pub revision_conflict: bool,
    pub current: ConfigurationDocument,
    pub candidate: ConfigurationDocument,
    pub diffs: Vec<ConfigurationDiffEntry>,
    pub validation_errors: Vec<ConfigurationValidationIssue>,
    pub has_changes: bool,
    pub can_apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationChangeAction {
    Apply,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationHistoryEntry {
    pub transaction_id: String,
    pub revision: u64,
    pub previous_revision: u64,
    pub action: ConfigurationChangeAction,
    pub target_revision: Option<u64>,
    pub actor: String,
    pub source: String,
    pub occurred_at: String,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct ConfigurationApplyResult {
    pub previous_revision: u64,
    pub revision: u64,
    pub no_op: bool,
    pub document: ConfigurationDocument,
    pub diffs: Vec<ConfigurationDiffEntry>,
    pub audit: Option<ConfigurationHistoryEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigurationTransactionError {
    #[error("配置 revision 冲突：期望 {expected}，当前 {current}")]
    RevisionConflict { expected: u64, current: u64 },
    #[error("配置校验失败")]
    InvalidCandidate(Box<ConfigurationPreview>),
    #[error("配置 revision {0} 不存在")]
    RevisionNotFound(u64),
    #[error("配置持久化失败: {0}")]
    Persistence(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigurationTransactionStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredState {
    schema_version: u16,
    revision: u64,
    generation: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_change: Option<StoredChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredChange {
    transaction_id: String,
    revision: u64,
    previous_revision: u64,
    action: ConfigurationChangeAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_revision: Option<u64>,
    actor: String,
    source: String,
    occurred_at: String,
    changed_paths: Vec<String>,
}

impl From<&ConfigurationHistoryEntry> for StoredChange {
    fn from(entry: &ConfigurationHistoryEntry) -> Self {
        Self {
            transaction_id: entry.transaction_id.clone(),
            revision: entry.revision,
            previous_revision: entry.previous_revision,
            action: entry.action,
            target_revision: entry.target_revision,
            actor: entry.actor.clone(),
            source: entry.source.clone(),
            occurred_at: entry.occurred_at.clone(),
            changed_paths: entry.changed_paths.clone(),
        }
    }
}

impl From<&StoredChange> for ConfigurationHistoryEntry {
    fn from(change: &StoredChange) -> Self {
        Self {
            transaction_id: change.transaction_id.clone(),
            revision: change.revision,
            previous_revision: change.previous_revision,
            action: change.action,
            target_revision: change.target_revision,
            actor: change.actor.clone(),
            source: change.source.clone(),
            occurred_at: change.occurred_at.clone(),
            changed_paths: change.changed_paths.clone(),
        }
    }
}

struct LockedStore {
    _lock: File,
}

struct PreparedCandidate {
    preview: ConfigurationPreview,
    configs: BTreeMap<String, ClientConfig>,
}

struct ConfigurationCommitContext<'a> {
    action: ConfigurationChangeAction,
    target_revision: Option<u64>,
    actor: &'a str,
    source: &'a str,
}

impl ConfigurationTransactionStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn current_network_path(
        &self,
        network_names: &[String],
        network_name: &str,
    ) -> Result<PathBuf> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(network_names)?;
        Ok(self.network_path_in_generation(&state.generation, network_name))
    }

    pub(crate) fn current_document(
        &self,
        network_names: &[String],
    ) -> Result<ConfigurationDocument> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(network_names)?;
        let configs = self.read_generation_configs(&state.generation, network_names)?;
        Ok(ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision: state.revision,
            networks: configs
                .into_iter()
                .map(|(name, config)| {
                    redacted_value(&config)
                        .map(|value| (name.clone(), value))
                        .with_context(|| format!("无法生成网络 [{name}] 的配置预览"))
                })
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn add_network(
        &self,
        network_name: &str,
        config: &ClientConfig,
        actor: &str,
        source: &str,
    ) -> Result<ConfigurationApplyResult> {
        validate_stored_network_name(network_name)?;
        let _locked = self.lock()?;
        let mut network_names = self.configured_names()?;
        let state = self.ensure_initialized_locked(&network_names)?;
        if network_names.iter().any(|name| name == network_name) {
            anyhow::bail!("网络配置已存在: {}", network_name);
        }

        let current_configs = self.read_generation_configs(&state.generation, &network_names)?;
        let mut candidate_configs = current_configs.clone();
        candidate_configs.insert(network_name.to_string(), config.clone());
        let prepared = self.prepare_configs_candidate(
            state.revision,
            &state,
            current_configs,
            candidate_configs,
            Vec::new(),
        )?;
        network_names.push(network_name.to_string());
        network_names.sort();
        self.commit_prepared(
            state,
            &network_names,
            prepared,
            ConfigurationCommitContext {
                action: ConfigurationChangeAction::Apply,
                target_revision: None,
                actor,
                source,
            },
        )
    }

    pub(crate) fn remove_network(
        &self,
        network_name: &str,
        actor: &str,
        source: &str,
    ) -> Result<ConfigurationApplyResult> {
        validate_stored_network_name(network_name)?;
        let _locked = self.lock()?;
        let mut network_names = self.configured_names()?;
        let state = self.ensure_initialized_locked(&network_names)?;
        let current_configs = self.read_generation_configs(&state.generation, &network_names)?;
        if !current_configs.contains_key(network_name) {
            anyhow::bail!("网络配置不存在: {}", network_name);
        }
        let mut candidate_configs = current_configs.clone();
        candidate_configs.remove(network_name);
        network_names.retain(|name| name != network_name);
        let prepared = self.prepare_configs_candidate(
            state.revision,
            &state,
            current_configs,
            candidate_configs,
            Vec::new(),
        )?;
        self.commit_prepared(
            state,
            &network_names,
            prepared,
            ConfigurationCommitContext {
                action: ConfigurationChangeAction::Apply,
                target_revision: None,
                actor,
                source,
            },
        )
    }

    pub(crate) fn preview(
        &self,
        expected_revision: u64,
        network_names: &[String],
        candidate_networks: BTreeMap<String, Value>,
    ) -> Result<ConfigurationPreview> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(network_names)?;
        let configs = self.read_generation_configs(&state.generation, network_names)?;
        Ok(self
            .prepare_candidate(
                expected_revision,
                &state,
                network_names,
                candidate_networks,
                configs,
            )?
            .preview)
    }

    pub(crate) fn apply(
        &self,
        expected_revision: u64,
        network_names: &[String],
        candidate_networks: BTreeMap<String, Value>,
        actor: &str,
        source: &str,
    ) -> std::result::Result<ConfigurationApplyResult, ConfigurationTransactionError> {
        let _locked = self.lock().map_err(persistence_error)?;
        let state = self
            .ensure_initialized_locked(network_names)
            .map_err(persistence_error)?;
        if expected_revision != state.revision {
            return Err(ConfigurationTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        let configs = self
            .read_generation_configs(&state.generation, network_names)
            .map_err(persistence_error)?;
        let prepared = self
            .prepare_candidate(
                expected_revision,
                &state,
                network_names,
                candidate_networks,
                configs,
            )
            .map_err(persistence_error)?;

        if !prepared.preview.validation_errors.is_empty() {
            return Err(ConfigurationTransactionError::InvalidCandidate(Box::new(
                prepared.preview,
            )));
        }
        if !prepared.preview.has_changes {
            return Ok(ConfigurationApplyResult {
                previous_revision: state.revision,
                revision: state.revision,
                no_op: true,
                document: prepared.preview.current,
                diffs: Vec::new(),
                audit: None,
            });
        }

        self.commit_prepared(
            state,
            network_names,
            prepared,
            ConfigurationCommitContext {
                action: ConfigurationChangeAction::Apply,
                target_revision: None,
                actor,
                source,
            },
        )
        .map_err(persistence_error)
    }

    pub(crate) fn rollback(
        &self,
        expected_revision: u64,
        target_revision: u64,
        network_names: &[String],
        actor: &str,
        source: &str,
    ) -> std::result::Result<ConfigurationApplyResult, ConfigurationTransactionError> {
        let _locked = self.lock().map_err(persistence_error)?;
        let state = self
            .ensure_initialized_locked(network_names)
            .map_err(persistence_error)?;
        if expected_revision != state.revision {
            return Err(ConfigurationTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        if target_revision >= state.revision || target_revision == 0 {
            return Err(ConfigurationTransactionError::RevisionNotFound(
                target_revision,
            ));
        }
        let target_generation = generation_name(target_revision);
        let target_dir = self.generations_dir().join(&target_generation);
        if !target_dir.is_dir() {
            return Err(ConfigurationTransactionError::RevisionNotFound(
                target_revision,
            ));
        }
        let target_configs = self
            .read_generation_configs(&target_generation, network_names)
            .map_err(persistence_error)?;
        let current_configs = self
            .read_generation_configs(&state.generation, network_names)
            .map_err(persistence_error)?;
        let candidate_networks = target_configs
            .iter()
            .map(|(name, config)| Ok((name.clone(), serde_json::to_value(config)?)))
            .collect::<Result<BTreeMap<_, _>>>()
            .map_err(persistence_error)?;
        let prepared = self
            .prepare_candidate(
                expected_revision,
                &state,
                network_names,
                candidate_networks,
                current_configs,
            )
            .map_err(persistence_error)?;
        if !prepared.preview.validation_errors.is_empty() {
            return Err(ConfigurationTransactionError::InvalidCandidate(Box::new(
                prepared.preview,
            )));
        }
        if !prepared.preview.has_changes {
            return Ok(ConfigurationApplyResult {
                previous_revision: state.revision,
                revision: state.revision,
                no_op: true,
                document: prepared.preview.current,
                diffs: Vec::new(),
                audit: None,
            });
        }
        self.commit_prepared(
            state,
            network_names,
            prepared,
            ConfigurationCommitContext {
                action: ConfigurationChangeAction::Rollback,
                target_revision: Some(target_revision),
                actor,
                source,
            },
        )
        .map_err(persistence_error)
    }

    pub(crate) fn history(
        &self,
        network_names: &[String],
    ) -> Result<Vec<ConfigurationHistoryEntry>> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(network_names)?;
        let mut entries = BTreeMap::<u64, ConfigurationHistoryEntry>::new();
        let history_dir = self.history_dir();
        if history_dir.is_dir() {
            for entry in std::fs::read_dir(&history_dir)
                .with_context(|| format!("无法读取配置历史目录: {}", history_dir.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("无法读取配置历史: {}", path.display()))?;
                let history: ConfigurationHistoryEntry = serde_json::from_slice(&bytes)
                    .with_context(|| format!("无法解析配置历史: {}", path.display()))?;
                entries.insert(history.revision, history);
            }
        }
        if let Some(change) = &state.last_change {
            entries
                .entry(change.revision)
                .or_insert_with(|| ConfigurationHistoryEntry::from(change));
        }
        let mut entries: Vec<_> = entries.into_values().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.revision));
        Ok(entries)
    }

    fn commit_prepared(
        &self,
        state: StoredState,
        network_names: &[String],
        prepared: PreparedCandidate,
        context: ConfigurationCommitContext<'_>,
    ) -> Result<ConfigurationApplyResult> {
        let revision = state
            .revision
            .checked_add(1)
            .context("配置 revision 已耗尽")?;
        let generation = generation_name(revision);
        let staging = self.generations_dir().join(format!(
            ".staging-{revision}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let committed_generation = self.generations_dir().join(&generation);
        let history_path = self.history_dir().join(format!("{revision:020}.json"));
        let occurred_at = chrono::Utc::now().to_rfc3339();
        let changed_paths = prepared
            .preview
            .diffs
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let transaction_id = transaction_id(revision, &changed_paths, &occurred_at);
        let history = ConfigurationHistoryEntry {
            transaction_id,
            revision,
            previous_revision: state.revision,
            action: context.action,
            target_revision: context.target_revision,
            actor: context.actor.to_string(),
            source: context.source.to_string(),
            occurred_at: occurred_at.clone(),
            changed_paths,
        };
        let next_state = StoredState {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision,
            generation: generation.clone(),
            updated_at: occurred_at,
            last_change: Some(StoredChange::from(&history)),
        };
        let document = ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision,
            networks: prepared
                .configs
                .iter()
                .map(|(name, config)| {
                    redacted_value(config)
                        .map(|value| (name.clone(), value))
                        .with_context(|| format!("无法生成网络 [{name}] 的配置预览"))
                })
                .collect::<Result<_>>()?,
        };
        let result = ConfigurationApplyResult {
            previous_revision: state.revision,
            revision,
            no_op: false,
            document,
            diffs: prepared.preview.diffs,
            audit: Some(history.clone()),
        };

        let mut state_committed = false;
        let commit = (|| -> Result<ConfigurationApplyResult> {
            std::fs::create_dir_all(&staging)
                .with_context(|| format!("无法创建配置暂存目录: {}", staging.display()))?;
            for name in network_names {
                let config = prepared
                    .configs
                    .get(name)
                    .with_context(|| format!("候选配置缺少网络 [{name}]"))?;
                let network_dir = staging.join(name);
                std::fs::create_dir_all(&network_dir).with_context(|| {
                    format!("无法创建配置暂存网络目录: {}", network_dir.display())
                })?;
                let path = network_dir.join("config.toml");
                config
                    .save_to_file(&path)
                    .with_context(|| format!("无法暂存网络 [{name}] 的配置"))?;
                sync_dir(&network_dir)?;
            }
            sync_dir(&staging)?;

            if committed_generation.exists() {
                anyhow::bail!("配置 generation 已存在: {}", committed_generation.display());
            }
            std::fs::rename(&staging, &committed_generation).with_context(|| {
                format!(
                    "无法发布配置 generation: {} -> {}",
                    staging.display(),
                    committed_generation.display()
                )
            })?;
            sync_dir(&self.generations_dir())?;

            let history_dir = self.history_dir();
            std::fs::create_dir_all(&history_dir)
                .with_context(|| format!("无法创建配置历史目录: {}", history_dir.display()))?;
            atomic_write_json(&history_path, &history)?;
            sync_dir(&history_dir)?;

            atomic_write_json(&self.state_path(), &next_state)?;
            state_committed = true;
            Ok(result)
        })();

        if commit.is_err() && !state_committed {
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_dir_all(&committed_generation);
            let _ = std::fs::remove_file(&history_path);
        }
        commit
    }

    fn prepare_candidate(
        &self,
        expected_revision: u64,
        state: &StoredState,
        network_names: &[String],
        candidate_networks: BTreeMap<String, Value>,
        current_configs: BTreeMap<String, ClientConfig>,
    ) -> Result<PreparedCandidate> {
        let expected_names = network_names.iter().cloned().collect::<BTreeSet<_>>();
        let candidate_names = candidate_networks.keys().cloned().collect::<BTreeSet<_>>();
        let mut validation_errors = Vec::new();
        for name in expected_names.difference(&candidate_names) {
            validation_errors.push(ConfigurationValidationIssue {
                path: format!("/networks/{}", escape_pointer(name)),
                code: ConfigurationValidationCode::MissingNetwork,
                message: format!("候选配置缺少网络 [{name}]"),
            });
        }
        for name in candidate_names.difference(&expected_names) {
            validation_errors.push(ConfigurationValidationIssue {
                path: format!("/networks/{}", escape_pointer(name)),
                code: ConfigurationValidationCode::UnknownNetwork,
                message: format!("候选配置包含未知网络 [{name}]"),
            });
        }

        let current_values = current_configs
            .iter()
            .map(|(name, config)| Ok((name.clone(), redacted_value(config)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut redacted_candidates = BTreeMap::new();
        let mut parsed_configs = BTreeMap::new();

        for (name, current_config) in &current_configs {
            let Some(candidate) = candidate_networks.get(name) else {
                continue;
            };
            let candidate = restore_redacted_values(candidate, current_config);
            let candidate_path = format!("/networks/{}", escape_pointer(name));
            match serde_path_to_error::deserialize::<_, ClientConfig>(candidate.clone()) {
                Ok(config) => {
                    let issues = validate_configuration(&candidate_path, &config);
                    if issues.is_empty() {
                        match redacted_value(&config) {
                            Ok(redacted) => {
                                redacted_candidates.insert(name.clone(), redacted);
                                parsed_configs.insert(name.clone(), config);
                            }
                            Err(error) => validation_errors.push(ConfigurationValidationIssue {
                                path: candidate_path,
                                code: ConfigurationValidationCode::InvalidValue,
                                message: format!("无法生成脱敏配置预览: {error:#}"),
                            }),
                        }
                    } else {
                        validation_errors.extend(issues);
                    }
                }
                Err(error) => {
                    let suffix = normalized_serde_path(error.path().to_string().as_str());
                    let path = if suffix.is_empty() {
                        candidate_path
                    } else {
                        format!("{candidate_path}/{suffix}")
                    };
                    let message = error.into_inner().to_string();
                    let code = if message.contains("missing field") {
                        ConfigurationValidationCode::MissingRequired
                    } else if message.contains("invalid type") {
                        ConfigurationValidationCode::InvalidType
                    } else {
                        ConfigurationValidationCode::InvalidValue
                    };
                    validation_errors.push(ConfigurationValidationIssue {
                        path,
                        code,
                        message,
                    });
                }
            }
        }

        for (name, candidate) in candidate_networks {
            if !current_configs.contains_key(&name) {
                redacted_candidates.insert(name, redact_raw_value(&candidate));
            }
        }

        let current = ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision: state.revision,
            networks: current_values,
        };
        let candidate = ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision: state.revision.saturating_add(1),
            networks: redacted_candidates,
        };
        let diffs = diff_documents(&current, &candidate);
        let revision_conflict = expected_revision != state.revision;
        let has_changes = !diffs.is_empty();
        let can_apply = !revision_conflict
            && validation_errors.is_empty()
            && has_changes
            && !network_names.is_empty();
        let preview = ConfigurationPreview {
            expected_revision,
            current_revision: state.revision,
            candidate_revision: state.revision.saturating_add(1),
            revision_conflict,
            current,
            candidate,
            diffs,
            validation_errors,
            has_changes,
            can_apply,
        };
        Ok(PreparedCandidate {
            preview,
            configs: parsed_configs,
        })
    }

    fn prepare_configs_candidate(
        &self,
        expected_revision: u64,
        state: &StoredState,
        current_configs: BTreeMap<String, ClientConfig>,
        candidate_configs: BTreeMap<String, ClientConfig>,
        validation_errors: Vec<ConfigurationValidationIssue>,
    ) -> Result<PreparedCandidate> {
        let current = ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision: state.revision,
            networks: current_configs
                .iter()
                .map(|(name, config)| Ok((name.clone(), redacted_value(config)?)))
                .collect::<Result<_>>()?,
        };
        let candidate = ConfigurationDocument {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision: state.revision.saturating_add(1),
            networks: candidate_configs
                .iter()
                .map(|(name, config)| Ok((name.clone(), redacted_value(config)?)))
                .collect::<Result<_>>()?,
        };
        let diffs = diff_documents(&current, &candidate);
        let has_changes = !diffs.is_empty();
        let revision_conflict = expected_revision != state.revision;
        let can_apply = !revision_conflict && validation_errors.is_empty() && has_changes;
        Ok(PreparedCandidate {
            preview: ConfigurationPreview {
                expected_revision,
                current_revision: state.revision,
                candidate_revision: state.revision.saturating_add(1),
                revision_conflict,
                current,
                candidate,
                diffs,
                validation_errors,
                has_changes,
                can_apply,
            },
            configs: candidate_configs,
        })
    }

    fn read_generation_configs(
        &self,
        generation: &str,
        network_names: &[String],
    ) -> Result<BTreeMap<String, ClientConfig>> {
        let mut configs = BTreeMap::new();
        for name in network_names {
            let path = self.network_path_in_generation(generation, name);
            let config = ClientConfig::from_file(&path).with_context(|| {
                format!(
                    "无法加载配置 generation [{}] 中网络 [{}] 的配置: {}",
                    generation,
                    name,
                    path.display()
                )
            })?;
            configs.insert(name.clone(), config);
        }
        Ok(configs)
    }

    fn ensure_initialized_locked(&self, network_names: &[String]) -> Result<StoredState> {
        self.ensure_directories()?;
        let state_path = self.state_path();
        if state_path.exists() {
            let bytes = std::fs::read(&state_path)
                .with_context(|| format!("无法读取配置状态: {}", state_path.display()))?;
            let state: StoredState = serde_json::from_slice(&bytes)
                .with_context(|| format!("无法解析配置状态: {}", state_path.display()))?;
            if state.schema_version != CONFIGURATION_SCHEMA_VERSION {
                anyhow::bail!(
                    "不支持的配置事务 schema {}，期望 {}",
                    state.schema_version,
                    CONFIGURATION_SCHEMA_VERSION
                );
            }
            let generation_dir = self.generations_dir().join(&state.generation);
            if !generation_dir.is_dir() {
                anyhow::bail!("配置 generation 缺失: {}", generation_dir.display());
            }
            self.recover_incomplete_commits(&state)?;
            return Ok(state);
        }

        let revision = 1;
        let generation = generation_name(revision);
        let staging = self.generations_dir().join(format!(
            ".migrate-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&staging)
            .with_context(|| format!("无法创建配置迁移目录: {}", staging.display()))?;
        for name in network_names {
            let legacy = self.legacy_network_path(name);
            if !legacy.is_file() {
                continue;
            }
            let config = ClientConfig::from_file(&legacy)
                .with_context(|| format!("无法迁移网络 [{}] 的配置", name))?;
            let target_dir = staging.join(name);
            std::fs::create_dir_all(&target_dir)
                .with_context(|| format!("无法创建迁移网络目录: {}", target_dir.display()))?;
            config.save_to_file(target_dir.join("config.toml"))?;
        }
        sync_dir(&staging)?;
        let target = self.generations_dir().join(&generation);
        if target.exists() {
            std::fs::remove_dir_all(&staging)?;
        } else {
            std::fs::rename(&staging, &target).with_context(|| {
                format!(
                    "无法发布初始配置 generation: {} -> {}",
                    staging.display(),
                    target.display()
                )
            })?;
        }
        sync_dir(&self.generations_dir())?;

        let state = StoredState {
            schema_version: CONFIGURATION_SCHEMA_VERSION,
            revision,
            generation,
            updated_at: chrono::Utc::now().to_rfc3339(),
            last_change: None,
        };
        atomic_write_json(&state_path, &state)?;
        self.recover_incomplete_commits(&state)?;
        Ok(state)
    }

    fn recover_incomplete_commits(&self, state: &StoredState) -> Result<()> {
        let generations_dir = self.generations_dir();
        for entry in std::fs::read_dir(&generations_dir).with_context(|| {
            format!(
                "无法读取配置 generation 目录: {}",
                generations_dir.display()
            )
        })? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let incomplete = name.starts_with(".staging-") || name.starts_with(".migrate-");
            let unpublished =
                parse_generation_name(name).is_some_and(|revision| revision > state.revision);
            if incomplete || unpublished {
                if path.is_dir() {
                    std::fs::remove_dir_all(&path).with_context(|| {
                        format!("无法清理未完成的配置 generation: {}", path.display())
                    })?;
                } else {
                    std::fs::remove_file(&path).with_context(|| {
                        format!("无法清理未完成的配置 generation: {}", path.display())
                    })?;
                }
            }
        }
        sync_dir(&generations_dir)?;

        let history_dir = self.history_dir();
        for entry in std::fs::read_dir(&history_dir)
            .with_context(|| format!("无法读取配置历史目录: {}", history_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let Some(revision) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<u64>().ok())
            else {
                continue;
            };
            if revision > state.revision {
                std::fs::remove_file(&path)
                    .with_context(|| format!("无法清理未发布的配置历史: {}", path.display()))?;
            }
        }
        sync_dir(&history_dir)
    }

    fn configured_names(&self) -> Result<Vec<String>> {
        let networks_file = self.root.join("networks.json");
        if !networks_file.is_file() {
            return Ok(Vec::new());
        }
        #[derive(Deserialize)]
        struct StoredNetworkList {
            networks: BTreeMap<String, Value>,
        }
        let bytes = std::fs::read(&networks_file)
            .with_context(|| format!("无法读取网络列表: {}", networks_file.display()))?;
        let list: StoredNetworkList = serde_json::from_slice(&bytes)
            .with_context(|| format!("无法解析网络列表: {}", networks_file.display()))?;
        Ok(list.networks.into_keys().collect())
    }

    fn ensure_directories(&self) -> Result<()> {
        for path in [self.store_dir(), self.generations_dir(), self.history_dir()] {
            std::fs::create_dir_all(&path)
                .with_context(|| format!("无法创建配置事务目录: {}", path.display()))?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<LockedStore> {
        self.ensure_directories()?;
        let path = self.store_dir().join(LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("无法打开配置事务锁: {}", path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("无法获取配置事务锁: {}", path.display()))?;
        Ok(LockedStore { _lock: lock })
    }

    fn store_dir(&self) -> PathBuf {
        self.root.join(STORE_DIR)
    }

    fn generations_dir(&self) -> PathBuf {
        self.store_dir().join(GENERATIONS_DIR)
    }

    fn history_dir(&self) -> PathBuf {
        self.store_dir().join(HISTORY_DIR)
    }

    fn state_path(&self) -> PathBuf {
        self.store_dir().join(STATE_FILE)
    }

    fn legacy_network_path(&self, network_name: &str) -> PathBuf {
        self.root.join(network_name).join("config.toml")
    }

    fn network_path_in_generation(&self, generation: &str, network_name: &str) -> PathBuf {
        self.generations_dir()
            .join(generation)
            .join(network_name)
            .join("config.toml")
    }
}

pub(crate) fn redacted_value(config: &ClientConfig) -> Result<Value> {
    let mut value = serde_json::to_value(config).context("无法序列化客户端配置")?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "private_key_path".into(),
            Value::String(REDACTED_PATH.into()),
        );
        redact_url_field(object, "coordinator_url");
        if let Some(log) = object.get_mut("log").and_then(Value::as_object_mut) {
            if log.get("file").is_some_and(|value| !value.is_null()) {
                log.insert("file".into(), Value::String(REDACTED_PATH.into()));
            }
        }
    }
    Ok(value)
}

fn redact_raw_value(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "private_key_path".into(),
            Value::String(REDACTED_PATH.into()),
        );
        redact_url_field(object, "coordinator_url");
        if let Some(log) = object.get_mut("log").and_then(Value::as_object_mut) {
            if log.get("file").is_some_and(|value| !value.is_null()) {
                log.insert("file".into(), Value::String(REDACTED_PATH.into()));
            }
        }
    }
    value
}

fn redact_url_field(object: &mut Map<String, Value>, key: &str) {
    if let Some(value) = object.get_mut(key) {
        if let Some(url) = value.as_str() {
            if let Some((scheme, rest)) = url.split_once("://") {
                if let Some(at) = rest.find('@') {
                    *value =
                        Value::String(format!("{scheme}://{REDACTED_URL}@{}", &rest[at + 1..]));
                }
            }
        }
    }
}

fn restore_redacted_values(candidate: &Value, current: &ClientConfig) -> Value {
    let mut candidate = candidate.clone();
    let Some(object) = candidate.as_object_mut() else {
        return candidate;
    };
    let preserve_private_key = object
        .get("private_key_path")
        .is_none_or(|value| value.as_str() == Some(REDACTED_PATH));
    if preserve_private_key {
        object.insert(
            "private_key_path".into(),
            Value::String(current.private_key_path.to_string_lossy().into_owned()),
        );
    }
    if let Some(coordinator_url) = object.get_mut("coordinator_url") {
        if coordinator_url
            .as_str()
            .is_some_and(|url| url.contains(REDACTED_URL))
        {
            *coordinator_url = Value::String(current.coordinator_url.clone());
        }
    }
    if let Some(log) = object.get_mut("log").and_then(Value::as_object_mut) {
        if log.get("file").and_then(Value::as_str) == Some(REDACTED_PATH) {
            match &current.log.file {
                Some(path) => {
                    log.insert(
                        "file".into(),
                        Value::String(path.to_string_lossy().into_owned()),
                    );
                }
                None => {
                    log.remove("file");
                }
            }
        }
    }
    candidate
}

fn validate_configuration(
    prefix: &str,
    config: &ClientConfig,
) -> Vec<ConfigurationValidationIssue> {
    let mut issues = Vec::new();
    let mut push = |path: &str, code: ConfigurationValidationCode, message: String| {
        issues.push(ConfigurationValidationIssue {
            path: format!("{prefix}/{path}"),
            code,
            message,
        });
    };

    if config.coordinator_url.trim().is_empty() {
        push(
            "coordinator_url",
            ConfigurationValidationCode::InvalidValue,
            "协调服务器地址不能为空".into(),
        );
    } else if !config.coordinator_url.starts_with("ws://")
        && !config.coordinator_url.starts_with("wss://")
    {
        push(
            "coordinator_url",
            ConfigurationValidationCode::InvalidValue,
            "协调服务器地址必须以 ws:// 或 wss:// 开头".into(),
        );
    }
    if config.private_key_path.as_os_str().is_empty() {
        push(
            "private_key_path",
            ConfigurationValidationCode::InvalidValue,
            "私钥路径不能为空".into(),
        );
    }
    if let Some(network) = &config.network.virtual_network {
        if network.parse::<ipnet::Ipv6Net>().is_err() {
            push(
                "network/virtual_network",
                ConfigurationValidationCode::InvalidValue,
                "虚拟网络必须是 IPv6 CIDR".into(),
            );
        }
    }
    if config.tun.name.trim().is_empty() {
        push(
            "tun/name",
            ConfigurationValidationCode::InvalidValue,
            "TUN 名称不能为空".into(),
        );
    }
    if config.tun.mtu < 1280 {
        push(
            "tun/mtu",
            ConfigurationValidationCode::InvalidValue,
            "TUN MTU 不能小于 1280".into(),
        );
    }
    if config.p2p.inbound_queue_capacity == 0 || config.p2p.inbound_queue_capacity > 65_536 {
        push(
            "p2p/inbound_queue_capacity",
            ConfigurationValidationCode::InvalidValue,
            "入站队列容量必须在 1 到 65536 之间".into(),
        );
    }
    if config.p2p.udp_batch_size == 0 || config.p2p.udp_batch_size > 128 {
        push(
            "p2p/udp_batch_size",
            ConfigurationValidationCode::InvalidValue,
            "UDP 批大小必须在 1 到 128 之间".into(),
        );
    }
    if config.p2p.crypto_workers > 256 {
        push(
            "p2p/crypto_workers",
            ConfigurationValidationCode::InvalidValue,
            "加密 worker 数必须在 0 到 256 之间".into(),
        );
    }
    if config.p2p.connect_timeout == 0 {
        push(
            "p2p/connect_timeout",
            ConfigurationValidationCode::InvalidValue,
            "连接超时必须大于 0".into(),
        );
    }
    if config.p2p.heartbeat_interval == 0 {
        push(
            "p2p/heartbeat_interval",
            ConfigurationValidationCode::InvalidValue,
            "心跳间隔必须大于 0".into(),
        );
    }
    if config.p2p.fallback_threshold.max_rtt_ms == 0 {
        push(
            "p2p/fallback_threshold/max_rtt_ms",
            ConfigurationValidationCode::InvalidValue,
            "最大 RTT 必须大于 0".into(),
        );
    }
    if !(0.0..=1.0).contains(&config.p2p.fallback_threshold.max_loss_rate) {
        push(
            "p2p/fallback_threshold/max_loss_rate",
            ConfigurationValidationCode::InvalidValue,
            "最大丢包率必须在 0.0 到 1.0 之间".into(),
        );
    }
    if config.p2p.fallback_threshold.timeout_secs == 0 {
        push(
            "p2p/fallback_threshold/timeout_secs",
            ConfigurationValidationCode::InvalidValue,
            "连接超时必须大于 0".into(),
        );
    }
    if config.p2p.nat_detection.timeout_ms == 0 {
        push(
            "p2p/nat_detection/timeout_ms",
            ConfigurationValidationCode::InvalidValue,
            "NAT 检测超时必须大于 0".into(),
        );
    }
    if config.p2p.nat_detection.enabled && config.p2p.nat_detection.stun_servers.is_empty() {
        push(
            "p2p/nat_detection/stun_servers",
            ConfigurationValidationCode::InvalidValue,
            "启用 NAT 检测时至少需要一个 STUN 地址".into(),
        );
    }
    for (index, server) in config.p2p.nat_detection.stun_servers.iter().enumerate() {
        if server.trim().is_empty() {
            push(
                &format!("p2p/nat_detection/stun_servers/{index}"),
                ConfigurationValidationCode::InvalidValue,
                "STUN 地址不能为空".into(),
            );
        }
    }
    let level = config.log.level.trim().to_ascii_lowercase();
    if !matches!(
        level.as_str(),
        "trace" | "debug" | "info" | "warn" | "warning" | "error"
    ) {
        push(
            "log/level",
            ConfigurationValidationCode::InvalidValue,
            "日志级别必须是 trace、debug、info、warn 或 error".into(),
        );
    }
    for (field, values) in [
        ("routing/direct_cidrs", &config.routing.direct_cidrs),
        ("routing/proxy_cidrs", &config.routing.proxy_cidrs),
    ] {
        for (index, cidr) in values.iter().enumerate() {
            if cidr.parse::<IpNet>().is_err() {
                push(
                    &format!("{field}/{index}"),
                    ConfigurationValidationCode::InvalidValue,
                    format!("CIDR 无效: {cidr}"),
                );
            }
        }
    }
    for (index, rule) in config.acl.rules.iter().enumerate() {
        for (field, cidr) in [
            ("src_cidr", rule.src_cidr.as_deref()),
            ("dst_cidr", rule.dst_cidr.as_deref()),
        ] {
            if let Some(cidr) = cidr {
                if cidr.parse::<IpNet>().is_err() {
                    push(
                        &format!("acl/rules/{index}/{field}"),
                        ConfigurationValidationCode::InvalidValue,
                        format!("CIDR 无效: {cidr}"),
                    );
                }
            }
        }
        if let Some((start, end)) = rule.dst_port_range {
            if start > end {
                push(
                    &format!("acl/rules/{index}/dst_port_range"),
                    ConfigurationValidationCode::InvalidValue,
                    "端口范围起点不能大于终点".into(),
                );
            }
        }
    }

    if let Err(error) = config.validate() {
        let message = format!("{error:#}");
        let path = validation_path(&message, prefix);
        if !issues.iter().any(|issue| issue.path == path) {
            issues.push(ConfigurationValidationIssue {
                path,
                code: ConfigurationValidationCode::InvalidValue,
                message,
            });
        }
    }
    issues
}

fn validation_path(message: &str, prefix: &str) -> String {
    const PATHS: &[&str] = &[
        "private_key_path",
        "coordinator_url",
        "network.virtual_network",
        "tun.name",
        "tun.mtu",
        "p2p.inbound_queue_capacity",
        "p2p.udp_batch_size",
        "p2p.crypto_workers",
        "p2p.connect_timeout",
        "p2p.heartbeat_interval",
        "p2p.fallback_threshold.max_rtt_ms",
        "p2p.fallback_threshold.max_loss_rate",
        "p2p.fallback_threshold.timeout_secs",
        "p2p.nat_detection.timeout_ms",
        "p2p.nat_detection.stun_servers",
        "log.level",
    ];
    for path in PATHS {
        if message.starts_with(path) || message.contains(&format!("{path} ")) {
            return format!("{prefix}/{}", path.replace('.', "/"));
        }
    }
    prefix.to_string()
}

fn diff_documents(
    current: &ConfigurationDocument,
    candidate: &ConfigurationDocument,
) -> Vec<ConfigurationDiffEntry> {
    let mut diffs = Vec::new();
    diff_values(
        "/networks",
        &Value::Object(
            current
                .networks
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ),
        &Value::Object(
            candidate
                .networks
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ),
        &mut diffs,
    );
    diffs
}

fn diff_values(path: &str, before: &Value, after: &Value, diffs: &mut Vec<ConfigurationDiffEntry>) {
    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            let keys = before
                .keys()
                .chain(after.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for key in keys {
                let child_path = format!("{path}/{}", escape_pointer(&key));
                match (before.get(&key), after.get(&key)) {
                    (Some(before), Some(after)) => diff_values(&child_path, before, after, diffs),
                    (Some(before), None) => diffs.push(ConfigurationDiffEntry {
                        path: child_path,
                        before: before.clone(),
                        after: Value::Null,
                        impact: impact_for_path(path),
                    }),
                    (None, Some(after)) => diffs.push(ConfigurationDiffEntry {
                        path: child_path,
                        before: Value::Null,
                        after: after.clone(),
                        impact: impact_for_path(path),
                    }),
                    (None, None) => {}
                }
            }
        }
        _ if before != after => diffs.push(ConfigurationDiffEntry {
            path: path.to_string(),
            before: before.clone(),
            after: after.clone(),
            impact: impact_for_path(path),
        }),
        _ => {}
    }
}

fn impact_for_path(path: &str) -> ConfigurationImpact {
    if path.contains("/p2p/fallback_threshold/") {
        ConfigurationImpact::Immediate
    } else {
        ConfigurationImpact::RestartRequired
    }
}

fn normalized_serde_path(path: &str) -> String {
    path.trim_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(escape_pointer)
        .collect::<Vec<_>>()
        .join("/")
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn generation_name(revision: u64) -> String {
    format!("revision-{revision:020}")
}

fn parse_generation_name(name: &str) -> Option<u64> {
    name.strip_prefix("revision-")?.parse().ok()
}

fn validate_stored_network_name(network_name: &str) -> Result<()> {
    if network_name.is_empty() || network_name == "." || network_name == ".." {
        anyhow::bail!("网络名称无效: {network_name}");
    }
    if network_name.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    }) {
        anyhow::bail!("网络名称包含不支持的字符: {network_name}");
    }
    Ok(())
}

fn transaction_id(revision: u64, changed_paths: &[String], occurred_at: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&revision.to_le_bytes());
    hasher.update(occurred_at.as_bytes());
    for path in changed_paths {
        hasher.update(path.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn persistence_error(error: impl std::fmt::Display) -> ConfigurationTransactionError {
    ConfigurationTransactionError::Persistence(format!("{error:#}"))
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("无法序列化配置事务数据")?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("目标文件没有父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("无法创建目标目录: {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("目标文件名无效")?;
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let write = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .with_context(|| format!("无法创建临时文件: {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("无法写入临时文件: {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("无法同步临时文件: {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("无法原子替换文件: {}", path.display()))?;
        sync_dir(parent)?;
        Ok(())
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write
}

fn sync_dir(path: &Path) -> Result<()> {
    let directory =
        File::open(path).with_context(|| format!("无法打开目录: {}", path.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("无法同步目录: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn fixture(networks: &[&str]) -> (PathBuf, Vec<String>, ConfigurationTransactionStore) {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "fuckipv6-config-transaction-{}-{suffix}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        for name in networks {
            let config = ClientConfig {
                client_id: None,
                coordinator_url: format!("ws://{name}.example:6666"),
                private_key_path: root.join("node_identity.pem"),
                network: Default::default(),
                tun: Default::default(),
                p2p: Default::default(),
                log: Default::default(),
                acl: Default::default(),
                routing: Default::default(),
                exit_node: Default::default(),
            };
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).expect("network dir");
            config
                .save_to_file(dir.join("config.toml"))
                .expect("legacy config");
        }
        let names = networks.iter().map(|name| (*name).to_string()).collect();
        (
            root.clone(),
            names,
            ConfigurationTransactionStore::new(root),
        )
    }

    fn document_map(
        store: &ConfigurationTransactionStore,
        names: &[String],
    ) -> BTreeMap<String, Value> {
        store
            .current_document(names)
            .expect("current document")
            .networks
    }

    #[test]
    fn preview_redacts_complete_document_and_reports_empty_diff() {
        let (root, names, store) = fixture(&["home", "office"]);
        let document = store.current_document(&names).expect("document");
        assert_eq!(document.revision, 1);
        assert_eq!(document.networks.len(), 2);
        for value in document.networks.values() {
            assert_eq!(value["private_key_path"], REDACTED_PATH);
            assert!(value.get("tun").is_some());
            assert!(value.get("p2p").is_some());
            assert!(value.get("routing").is_some());
            assert!(value.get("exit_node").is_some());
        }
        let preview = store
            .preview(1, &names, document.networks)
            .expect("preview");
        assert!(!preview.revision_conflict);
        assert!(preview.validation_errors.is_empty());
        assert!(!preview.has_changes);
        assert!(!preview.can_apply);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn type_errors_are_located_with_json_pointer_paths() {
        let (root, names, store) = fixture(&["home"]);
        let mut candidate = document_map(&store, &names);
        candidate
            .get_mut("home")
            .expect("home")
            .as_object_mut()
            .expect("object")
            .insert(
                "tun".into(),
                serde_json::json!({"name":"fk0","mtu":"not-a-number"}),
            );
        let preview = store.preview(1, &names, candidate).expect("preview");
        assert!(preview.validation_errors.iter().any(|issue| {
            issue.path == "/networks/home/tun/mtu"
                && issue.code == ConfigurationValidationCode::InvalidType
        }));
        assert!(!preview.can_apply);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn validation_collects_multiple_field_paths_and_does_not_mutate() {
        let (root, names, store) = fixture(&["home"]);
        let before = store.current_document(&names).expect("before");
        let mut candidate = before.networks.clone();
        let home = candidate.get_mut("home").expect("home");
        home["tun"]["mtu"] = serde_json::json!(900);
        home["p2p"]["connect_timeout"] = serde_json::json!(0);
        home["log"]["level"] = serde_json::json!("verbose");
        let error = store
            .apply(1, &names, candidate, "owner", "test")
            .expect_err("invalid candidate");
        let ConfigurationTransactionError::InvalidCandidate(preview) = error else {
            panic!("expected validation error");
        };
        assert!(preview
            .validation_errors
            .iter()
            .any(|issue| issue.path == "/networks/home/tun/mtu"));
        assert!(preview
            .validation_errors
            .iter()
            .any(|issue| issue.path == "/networks/home/p2p/connect_timeout"));
        assert!(preview
            .validation_errors
            .iter()
            .any(|issue| issue.path == "/networks/home/log/level"));
        assert_eq!(
            store.current_document(&names).expect("after"),
            before,
            "failed validation must not mutate the active generation"
        );
        assert_eq!(store.history(&names).expect("history").len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revision_conflict_is_rejected_before_writes() {
        let (root, names, store) = fixture(&["home"]);
        let mut candidate = document_map(&store, &names);
        candidate
            .get_mut("home")
            .expect("home")
            .as_object_mut()
            .expect("object")
            .insert(
                "coordinator_url".into(),
                serde_json::json!("wss://new.example"),
            );
        let error = store
            .apply(2, &names, candidate, "owner", "test")
            .expect_err("revision conflict");
        assert!(matches!(
            error,
            ConfigurationTransactionError::RevisionConflict {
                expected: 2,
                current: 1
            }
        ));
        assert_eq!(store.current_document(&names).unwrap().revision, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apply_commits_every_network_atomically_and_creates_audit_history() {
        let (root, names, store) = fixture(&["home", "office"]);
        let mut candidate = document_map(&store, &names);
        candidate
            .get_mut("home")
            .expect("home")
            .as_object_mut()
            .expect("object")
            .insert(
                "coordinator_url".into(),
                serde_json::json!("wss://home-new.example"),
            );
        candidate
            .get_mut("office")
            .expect("office")
            .as_object_mut()
            .expect("object")
            .get_mut("p2p")
            .and_then(Value::as_object_mut)
            .expect("p2p")
            .insert("connect_timeout".into(), serde_json::json!(25));
        let result = store
            .apply(1, &names, candidate, "node-owner", "webui")
            .expect("apply");
        assert_eq!(result.previous_revision, 1);
        assert_eq!(result.revision, 2);
        assert_eq!(result.document.revision, 2);
        assert!(result.diffs.iter().any(|diff| {
            diff.path == "/networks/home/coordinator_url"
                && diff.impact == ConfigurationImpact::RestartRequired
        }));
        assert!(result.diffs.iter().any(|diff| {
            diff.path == "/networks/office/p2p/connect_timeout"
                && diff.impact == ConfigurationImpact::RestartRequired
        }));
        let history = store.history(&names).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].revision, 2);
        assert_eq!(history[0].previous_revision, 1);
        assert_eq!(history[0].action, ConfigurationChangeAction::Apply);
        assert_eq!(history[0].actor, "node-owner");
        assert_eq!(history[0].source, "webui");
        assert!(history[0]
            .changed_paths
            .contains(&"/networks/home/coordinator_url".to_string()));
        let current = store.current_document(&names).expect("current");
        assert_eq!(current.revision, 2);
        assert_eq!(
            current.networks["home"]["coordinator_url"],
            "wss://home-new.example"
        );
        assert_eq!(current.networks["home"]["private_key_path"], REDACTED_PATH);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_creates_a_new_revision_instead_of_rewinding_history() {
        let (root, names, store) = fixture(&["home"]);
        let mut first = document_map(&store, &names);
        first
            .get_mut("home")
            .and_then(Value::as_object_mut)
            .and_then(|home| home.get_mut("p2p"))
            .and_then(Value::as_object_mut)
            .expect("p2p")
            .insert("connect_timeout".into(), serde_json::json!(20));
        store
            .apply(1, &names, first, "owner", "gpui")
            .expect("first apply");
        let mut second = document_map(&store, &names);
        second
            .get_mut("home")
            .and_then(Value::as_object_mut)
            .and_then(|home| home.get_mut("p2p"))
            .and_then(Value::as_object_mut)
            .expect("p2p")
            .insert("connect_timeout".into(), serde_json::json!(30));
        store
            .apply(2, &names, second, "owner", "gpui")
            .expect("second apply");

        let result = store
            .rollback(3, 2, &names, "owner", "webui")
            .expect("rollback");
        assert_eq!(result.previous_revision, 3);
        assert_eq!(result.revision, 4);
        assert_eq!(result.document.revision, 4);
        assert_eq!(
            result.document.networks["home"]["p2p"]["connect_timeout"],
            20
        );
        let history = store.history(&names).expect("history");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].revision, 4);
        assert_eq!(history[0].action, ConfigurationChangeAction::Rollback);
        assert_eq!(history[0].target_revision, Some(2));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn redacted_import_preserves_private_key_and_log_paths() {
        let (root, names, store) = fixture(&["home"]);
        let path = store.current_network_path(&names, "home").expect("path");
        let mut config = ClientConfig::from_file(&path).expect("config");
        config.log.file = Some(root.join("private/activity.log"));
        config.save_to_file(&path).expect("save secret path");

        let before = store.current_document(&names).expect("before");
        assert_eq!(before.networks["home"]["log"]["file"], REDACTED_PATH);
        let mut candidate = before.networks;
        candidate
            .get_mut("home")
            .and_then(Value::as_object_mut)
            .and_then(|home| home.get_mut("log"))
            .and_then(Value::as_object_mut)
            .expect("log")
            .insert("level".into(), serde_json::json!("debug"));
        let result = store
            .apply(1, &names, candidate, "owner", "webui")
            .expect("apply");
        assert_eq!(result.revision, 2);
        let committed_path = store
            .current_network_path(&names, "home")
            .expect("committed path");
        let committed = ClientConfig::from_file(committed_path).expect("committed");
        assert_eq!(committed.private_key_path, root.join("node_identity.pem"));
        assert_eq!(committed.log.file, Some(root.join("private/activity.log")));
        assert_eq!(committed.log.level, "debug");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_unknown_network_is_a_field_level_error() {
        let (root, names, store) = fixture(&["home"]);
        let mut candidate = document_map(&store, &names);
        candidate.insert("intruder".into(), candidate["home"].clone());
        candidate.remove("home");
        let preview = store.preview(1, &names, candidate).expect("preview");
        assert!(preview.validation_errors.iter().any(|issue| {
            issue.code == ConfigurationValidationCode::MissingNetwork
                && issue.path == "/networks/home"
        }));
        assert!(preview.validation_errors.iter().any(|issue| {
            issue.code == ConfigurationValidationCode::UnknownNetwork
                && issue.path == "/networks/intruder"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_removes_unpublished_generations_histories_and_staging_directories() {
        let (root, names, store) = fixture(&["home"]);
        let before = store.current_document(&names).expect("before");

        let orphan_generation = store.generations_dir().join(generation_name(2));
        std::fs::create_dir_all(orphan_generation.join("home")).expect("orphan generation");
        std::fs::write(orphan_generation.join("home/config.toml"), "orphan")
            .expect("orphan config");
        std::fs::write(
            store.history_dir().join(format!("{:020}.json", 2)),
            "{\"orphan\":true}",
        )
        .expect("orphan history");
        let staging = store.generations_dir().join(".staging-2-orphan");
        std::fs::create_dir_all(&staging).expect("orphan staging");

        let after = store.current_document(&names).expect("recovered document");
        assert_eq!(after, before);
        assert!(!orphan_generation.exists());
        assert!(!staging.exists());
        assert!(!store.history_dir().join(format!("{:020}.json", 2)).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
