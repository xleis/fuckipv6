//! Extensible capability registry and fail-closed capability presentation.

use super::foundation::ContractIssue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

/// Stable capability key. Built-ins use reserved names; extensions must use a
/// lowercase namespaced key such as `extension.example.watch`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityIdError(pub String);

impl fmt::Display for CapabilityIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CapabilityIdError {}

impl CapabilityId {
    pub fn new(value: impl Into<String>) -> Result<Self, CapabilityIdError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 {
            return Err(CapabilityIdError(
                "capability id must contain 1-128 characters".into(),
            ));
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err(CapabilityIdError(
                "capability id must use lowercase ASCII letters, digits, '.', '_' or '-'".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn coordinator() -> Self {
        Self("coordinator".into())
    }

    pub fn relay() -> Self {
        Self("relay".into())
    }

    pub fn exit() -> Self {
        Self("exit".into())
    }

    pub fn file() -> Self {
        Self("file".into())
    }
}

impl FromStr for CapabilityId {
    type Err = CapabilityIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    Coordinator,
    Relay,
    Exit,
    File,
    Extension(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityIcon {
    Topology,
    Route,
    ExitDoor,
    Files,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityColor {
    Sky,
    Violet,
    Amber,
    Teal,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityVisual {
    pub icon: CapabilityIcon,
    pub color: CapabilityColor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub id: CapabilityId,
    pub kind: CapabilityKind,
    pub label: String,
    pub visual: CapabilityVisual,
    pub extension: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityHealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityHealth {
    pub status: CapabilityHealthStatus,
    pub reason: Option<String>,
}

impl Default for CapabilityHealth {
    fn default() -> Self {
        Self {
            status: CapabilityHealthStatus::Unknown,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityLimits {
    pub max_bandwidth_kbps: Option<u64>,
    pub max_connections: Option<u32>,
    pub max_storage_bytes: Option<u64>,
    pub allow_lan_access: bool,
    pub allow_ipv4: bool,
    pub allow_ipv6: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Disabled,
    Enabled,
    Degraded,
    Failed,
    Unknown,
}

/// `Neutral` is the normal state for a member who does not contribute this
/// capability. It is not a failure and must not produce a warning badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityContribution {
    Neutral,
    Offered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub contribution: CapabilityContribution,
    pub authorized: bool,
    pub state: CapabilityState,
    pub health: CapabilityHealth,
    pub limits: CapabilityLimits,
    pub failure_closed: bool,
}

impl CapabilityGrant {
    pub fn neutral() -> Self {
        Self {
            contribution: CapabilityContribution::Neutral,
            authorized: false,
            state: CapabilityState::Disabled,
            health: CapabilityHealth::default(),
            limits: CapabilityLimits::default(),
            failure_closed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDecision {
    pub visible: bool,
    pub authorized: bool,
    pub state: CapabilityState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRingSegment {
    pub capability_id: CapabilityId,
    pub kind: CapabilityKind,
    pub visual: CapabilityVisual,
    pub state: CapabilityState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRegistry {
    descriptors: BTreeMap<CapabilityId, CapabilityDescriptor>,
}

impl CapabilityRegistry {
    pub fn builtin() -> Self {
        let definitions = [
            (
                CapabilityId::coordinator(),
                CapabilityKind::Coordinator,
                "Coordinator",
                CapabilityVisual {
                    icon: CapabilityIcon::Topology,
                    color: CapabilityColor::Sky,
                },
            ),
            (
                CapabilityId::relay(),
                CapabilityKind::Relay,
                "Relay",
                CapabilityVisual {
                    icon: CapabilityIcon::Route,
                    color: CapabilityColor::Violet,
                },
            ),
            (
                CapabilityId::exit(),
                CapabilityKind::Exit,
                "Exit",
                CapabilityVisual {
                    icon: CapabilityIcon::ExitDoor,
                    color: CapabilityColor::Amber,
                },
            ),
            (
                CapabilityId::file(),
                CapabilityKind::File,
                "File",
                CapabilityVisual {
                    icon: CapabilityIcon::Files,
                    color: CapabilityColor::Teal,
                },
            ),
        ];
        let descriptors = definitions
            .into_iter()
            .map(|(id, kind, label, visual)| {
                (
                    id.clone(),
                    CapabilityDescriptor {
                        id,
                        kind,
                        label: label.into(),
                        visual,
                        extension: false,
                    },
                )
            })
            .collect();
        Self { descriptors }
    }

    pub fn register_extension(
        &mut self,
        descriptor: CapabilityDescriptor,
    ) -> Result<(), Vec<ContractIssue>> {
        let mut issues = Vec::new();
        if !descriptor.extension {
            issues.push(ContractIssue::new(
                "extension_flag_required",
                "extension",
                "extension capabilities must be marked as extensions",
            ));
        }
        if self.descriptors.contains_key(&descriptor.id) {
            issues.push(ContractIssue::new(
                "capability_already_registered",
                "id",
                "capability id is already registered",
            ));
        }
        if !matches!(descriptor.kind, CapabilityKind::Extension(_)) {
            issues.push(ContractIssue::new(
                "extension_kind_required",
                "kind",
                "extension capabilities must use extension kind",
            ));
        }
        if descriptor.label.trim().is_empty() {
            issues.push(ContractIssue::new(
                "value_required",
                "label",
                "capability label must not be empty",
            ));
        }
        if !issues.is_empty() {
            return Err(issues);
        }
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }

    pub fn descriptor(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors.get(id)
    }

    /// Unknown capabilities fail closed: they are neither authorized nor shown.
    pub fn resolve(
        &self,
        id: &CapabilityId,
        grant: Option<&CapabilityGrant>,
    ) -> CapabilityDecision {
        let Some(_descriptor) = self.descriptor(id) else {
            return CapabilityDecision {
                visible: false,
                authorized: false,
                state: CapabilityState::Unknown,
                reason: Some("unknown_capability".into()),
            };
        };
        let Some(grant) = grant else {
            return CapabilityDecision {
                visible: false,
                authorized: false,
                state: CapabilityState::Disabled,
                reason: Some("capability_not_granted".into()),
            };
        };
        if grant.contribution == CapabilityContribution::Neutral {
            return CapabilityDecision {
                visible: false,
                authorized: false,
                state: CapabilityState::Disabled,
                reason: None,
            };
        }
        if !grant.authorized {
            return CapabilityDecision {
                visible: true,
                authorized: false,
                state: CapabilityState::Disabled,
                reason: Some("capability_not_authorized".into()),
            };
        }
        CapabilityDecision {
            visible: true,
            authorized: true,
            state: grant.state,
            reason: grant.health.reason.clone(),
        }
    }

    /// Produce deterministic ring segments. Known capabilities appear in stable
    /// built-in order, followed by registered extensions in lexical order.
    pub fn ring_segments(
        &self,
        grants: &BTreeMap<CapabilityId, CapabilityGrant>,
    ) -> Vec<CapabilityRingSegment> {
        let mut segments = self
            .descriptors
            .values()
            .filter_map(|descriptor| {
                let decision = self.resolve(&descriptor.id, grants.get(&descriptor.id));
                (decision.visible && decision.authorized).then_some(CapabilityRingSegment {
                    capability_id: descriptor.id.clone(),
                    kind: descriptor.kind.clone(),
                    visual: descriptor.visual,
                    state: decision.state,
                })
            })
            .collect::<Vec<_>>();
        segments.sort_by_key(|segment| capability_order(&segment.kind, &segment.capability_id));
        segments
    }

    pub fn ids(&self) -> BTreeSet<CapabilityId> {
        self.descriptors.keys().cloned().collect()
    }
}

fn capability_order(kind: &CapabilityKind, id: &CapabilityId) -> (u8, String) {
    let rank = match kind {
        CapabilityKind::Coordinator => 0,
        CapabilityKind::Relay => 1,
        CapabilityKind::Exit => 2,
        CapabilityKind::File => 3,
        CapabilityKind::Extension(_) => 4,
    };
    (rank, id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered() -> CapabilityGrant {
        CapabilityGrant {
            contribution: CapabilityContribution::Offered,
            authorized: true,
            state: CapabilityState::Enabled,
            health: CapabilityHealth {
                status: CapabilityHealthStatus::Healthy,
                reason: None,
            },
            limits: CapabilityLimits::default(),
            failure_closed: true,
        }
    }

    #[test]
    fn builtins_have_fixed_visual_semantics_and_ordering() {
        let registry = CapabilityRegistry::builtin();
        let grants = BTreeMap::from([
            (CapabilityId::file(), offered()),
            (CapabilityId::relay(), offered()),
            (CapabilityId::coordinator(), offered()),
            (CapabilityId::exit(), offered()),
        ]);
        let segments = registry.ring_segments(&grants);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.capability_id.to_string())
                .collect::<Vec<_>>(),
            ["coordinator", "relay", "exit", "file"]
        );
        assert_eq!(segments[0].visual.icon, CapabilityIcon::Topology);
        assert_eq!(segments[2].visual.color, CapabilityColor::Amber);
    }

    #[test]
    fn neutral_member_capability_is_hidden_and_not_a_failure() {
        let registry = CapabilityRegistry::builtin();
        let decision = registry.resolve(&CapabilityId::relay(), Some(&CapabilityGrant::neutral()));
        assert!(!decision.visible);
        assert!(!decision.authorized);
        assert_eq!(decision.state, CapabilityState::Disabled);
        assert_eq!(decision.reason, None);
    }

    #[test]
    fn unknown_capabilities_default_to_hidden_and_denied() {
        let registry = CapabilityRegistry::builtin();
        let unknown = CapabilityId::new("extension.example.unknown").expect("id");
        let decision = registry.resolve(&unknown, Some(&offered()));
        assert!(!decision.visible);
        assert!(!decision.authorized);
        assert_eq!(decision.state, CapabilityState::Unknown);

        let grants = BTreeMap::from([(unknown, offered())]);
        assert!(registry.ring_segments(&grants).is_empty());
    }

    #[test]
    fn extension_registration_extends_registry_without_rewriting_builtins() {
        let mut registry = CapabilityRegistry::builtin();
        let id = CapabilityId::new("extension.example.watch").expect("id");
        registry
            .register_extension(CapabilityDescriptor {
                id: id.clone(),
                kind: CapabilityKind::Extension("example".into()),
                label: "Watch".into(),
                visual: CapabilityVisual {
                    icon: CapabilityIcon::Extension,
                    color: CapabilityColor::Neutral,
                },
                extension: true,
            })
            .expect("register");
        assert!(registry.descriptor(&id).is_some());
        assert_eq!(registry.ids().len(), 5);
    }
}
