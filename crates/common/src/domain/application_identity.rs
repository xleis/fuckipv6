use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainResult};

/// A concrete application identity used by traffic rules. A display name is
/// deliberately not part of the identity: it is presentation-only and cannot
/// authorize or select a route.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApplicationIdentity {
    ExecutablePath(String),
    PackageId(String),
    Unknown,
}

impl ApplicationIdentity {
    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::ExecutablePath(path) => {
                if path.is_empty()
                    || path.len() > 4096
                    || path.trim() != path
                    || path.contains('\0')
                    || path.contains('\n')
                    || path.contains('\r')
                {
                    return Err(DomainError::invalid_value(
                        "application executable path is invalid",
                    ));
                }
            }
            Self::PackageId(package) => {
                if package.is_empty()
                    || package.len() > 255
                    || !package.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    return Err(DomainError::invalid_value(
                        "application package id is invalid",
                    ));
                }
            }
            Self::Unknown => {}
        }
        Ok(())
    }

    pub fn format(&self) -> Option<ApplicationIdentityFormat> {
        match self {
            Self::ExecutablePath(_) => Some(ApplicationIdentityFormat::ExecutablePath),
            Self::PackageId(_) => Some(ApplicationIdentityFormat::PackageId),
            Self::Unknown => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// Unknown is never a positive matcher. An unknown observation can only
    /// fall through to the explicit default policy or another non-application
    /// condition; it can never equal another unknown and accidentally match a
    /// concrete application rule.
    pub(crate) fn matches(&self, observed: &Self) -> bool {
        match (self, observed) {
            (Self::ExecutablePath(expected), Self::ExecutablePath(actual)) => expected == actual,
            (Self::PackageId(expected), Self::PackageId(actual)) => expected == actual,
            _ => false,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationIdentityFormat {
    ExecutablePath,
    PackageId,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationIdentityPlatform {
    Linux,
    MacOs,
    Windows,
    Android,
}

impl ApplicationIdentityPlatform {
    /// Contract-level formats each platform adapter is allowed to report.
    /// Runtime availability is separately represented by
    /// `ApplicationIdentityCapability`; declaring a format here does not prove
    /// that the current host has permission or a working adapter.
    pub const fn declared_formats(self) -> &'static [ApplicationIdentityFormat] {
        match self {
            Self::Linux => &[ApplicationIdentityFormat::ExecutablePath],
            Self::MacOs | Self::Windows => &[
                ApplicationIdentityFormat::PackageId,
                ApplicationIdentityFormat::ExecutablePath,
            ],
            Self::Android => &[ApplicationIdentityFormat::PackageId],
        }
    }

    pub fn supports_format(self, format: ApplicationIdentityFormat) -> bool {
        self.declared_formats().contains(&format)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApplicationIdentityCapabilityState {
    Available,
    Unavailable(String),
    Unsupported(String),
}

/// Runtime capability for application matching. UI must disable the
/// application condition whenever `is_enabled` is false and display
/// `disabled_explanation`; policy application performs the same check on the
/// server so a stale UI cannot submit an unsupported condition.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ApplicationIdentityCapability {
    pub platform: ApplicationIdentityPlatform,
    pub formats: Vec<ApplicationIdentityFormat>,
    pub state: ApplicationIdentityCapabilityState,
}

impl ApplicationIdentityCapability {
    pub fn available(
        platform: ApplicationIdentityPlatform,
        formats: impl IntoIterator<Item = ApplicationIdentityFormat>,
    ) -> DomainResult<Self> {
        let capability = Self {
            platform,
            formats: formats.into_iter().collect(),
            state: ApplicationIdentityCapabilityState::Available,
        };
        capability.validate()?;
        Ok(capability)
    }

    pub fn unavailable(
        platform: ApplicationIdentityPlatform,
        explanation: impl Into<String>,
    ) -> DomainResult<Self> {
        Self::disabled(
            platform,
            ApplicationIdentityCapabilityState::Unavailable(explanation.into()),
        )
    }

    pub fn unsupported(
        platform: ApplicationIdentityPlatform,
        explanation: impl Into<String>,
    ) -> DomainResult<Self> {
        Self::disabled(
            platform,
            ApplicationIdentityCapabilityState::Unsupported(explanation.into()),
        )
    }

    fn disabled(
        platform: ApplicationIdentityPlatform,
        state: ApplicationIdentityCapabilityState,
    ) -> DomainResult<Self> {
        let capability = Self {
            platform,
            formats: Vec::new(),
            state,
        };
        capability.validate()?;
        Ok(capability)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if has_duplicates(&self.formats) {
            return Err(DomainError::invariant(
                "application identity capability contains duplicate formats",
            ));
        }
        match &self.state {
            ApplicationIdentityCapabilityState::Available => {
                if self.formats.is_empty() {
                    return Err(DomainError::invalid_value(
                        "available application identity capability needs a format",
                    ));
                }
                if self
                    .formats
                    .iter()
                    .any(|format| !self.platform.supports_format(*format))
                {
                    return Err(DomainError::invalid_value(
                        "application identity format is not declared for the platform",
                    ));
                }
            }
            ApplicationIdentityCapabilityState::Unavailable(explanation)
            | ApplicationIdentityCapabilityState::Unsupported(explanation) => {
                if explanation.is_empty()
                    || explanation.trim() != explanation
                    || explanation.contains('\0')
                    || explanation.len() > 512
                {
                    return Err(DomainError::invalid_value(
                        "disabled application identity capability needs a clear explanation",
                    ));
                }
                if !self.formats.is_empty() {
                    return Err(DomainError::invariant(
                        "disabled application identity capability cannot advertise formats",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.state, ApplicationIdentityCapabilityState::Available)
    }

    pub fn disabled_explanation(&self) -> Option<&str> {
        match &self.state {
            ApplicationIdentityCapabilityState::Available => None,
            ApplicationIdentityCapabilityState::Unavailable(explanation)
            | ApplicationIdentityCapabilityState::Unsupported(explanation) => Some(explanation),
        }
    }
}

/// PID alone is reusable. The start-time token must come from the operating
/// system's process creation metadata rather than wall-clock time first seen by
/// this product. A key is never inferred from a PID alone.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct ProcessInstanceKey {
    pub pid: u32,
    /// Opaque platform process-creation token. Its only required property is
    /// stability for one process instance and difference across PID reuse.
    pub start_time_token: u64,
}

impl ProcessInstanceKey {
    pub fn new(pid: u32, start_time_token: u64) -> DomainResult<Self> {
        let value = Self {
            pid,
            start_time_token,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.pid == 0 || self.start_time_token == 0 {
            return Err(DomainError::invalid_value(
                "process instance identity is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationProcessClass {
    Application,
    /// A helper owned by its parent application. The helper must inherit the
    /// parent identity even when it has its own executable path.
    OwnedChild,
    /// Script hosts such as Python, Node.js or Java. Command-line script paths
    /// are not used as identity; a known parent wins, otherwise the interpreter
    /// executable itself is the explicit fallback.
    Interpreter,
    /// A container process is unrelated to its supervisor for routing
    /// purposes. It is known only through an explicit container mapping.
    Container,
    /// A service is known only through its service registration. It does not
    /// inherit the identity of a service manager or supervisor.
    Service,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationProcessLifetime {
    Persistent,
    ShortLived,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationIdentityUnavailableReason {
    ProcessObservationUnavailable,
    DirectIdentityUnavailable,
    ParentIdentityUnavailable,
    InheritanceCycle,
    InheritanceDepthExceeded,
    ContainerMappingUnavailable,
    ServiceRegistrationUnavailable,
}

/// How a concrete identity was obtained. This enum intentionally has no
/// command-line, arguments or environment field.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApplicationIdentitySource {
    ExecutablePath,
    PlatformPackage,
    ServiceRegistration,
    ContainerMapping,
    Unavailable(ApplicationIdentityUnavailableReason),
}

/// Adapter-provided facts for one process instance. The adapter must not put
/// command lines or environment variables in this contract.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessApplicationObservation {
    pub process: ProcessInstanceKey,
    pub parent: Option<ProcessInstanceKey>,
    pub class: ApplicationProcessClass,
    pub lifetime: ApplicationProcessLifetime,
    pub identity: ApplicationIdentity,
    pub source: ApplicationIdentitySource,
}

impl ProcessApplicationObservation {
    pub fn validate(&self) -> DomainResult<()> {
        self.process.validate()?;
        if let Some(parent) = &self.parent {
            parent.validate()?;
            if parent == &self.process {
                return Err(DomainError::invariant(
                    "process cannot be its own application-identity parent",
                ));
            }
        }
        self.identity.validate()?;
        if self.class == ApplicationProcessClass::OwnedChild && self.parent.is_none() {
            return Err(DomainError::invalid_value(
                "an owned child process must record its parent",
            ));
        }
        if !source_matches_identity(&self.source, &self.identity) {
            return Err(DomainError::invariant(
                "application identity source does not match its identity",
            ));
        }
        match self.class {
            ApplicationProcessClass::Application
            | ApplicationProcessClass::OwnedChild
            | ApplicationProcessClass::Interpreter => {
                if !matches!(
                    self.source,
                    ApplicationIdentitySource::ExecutablePath
                        | ApplicationIdentitySource::PlatformPackage
                        | ApplicationIdentitySource::Unavailable(_)
                ) {
                    return Err(DomainError::invariant(
                        "process class uses an invalid application-identity source",
                    ));
                }
            }
            ApplicationProcessClass::Container => {
                if !matches!(
                    self.source,
                    ApplicationIdentitySource::ContainerMapping
                        | ApplicationIdentitySource::Unavailable(_)
                ) {
                    return Err(DomainError::invalid_value(
                        "container process requires an explicit container mapping",
                    ));
                }
            }
            ApplicationProcessClass::Service => {
                if !matches!(
                    self.source,
                    ApplicationIdentitySource::ServiceRegistration
                        | ApplicationIdentitySource::Unavailable(_)
                ) {
                    return Err(DomainError::invalid_value(
                        "service process requires an explicit service registration",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationIdentityResolution {
    pub process: ProcessInstanceKey,
    pub identity: ApplicationIdentity,
    pub source: ApplicationIdentitySource,
    /// Immediate parent first, then grandparents. Empty for a direct identity.
    #[serde(default)]
    pub inherited_through: Vec<ProcessInstanceKey>,
}

impl ApplicationIdentityResolution {
    fn direct(
        process: ProcessInstanceKey,
        identity: ApplicationIdentity,
        source: ApplicationIdentitySource,
    ) -> Self {
        Self {
            process,
            identity,
            source,
            inherited_through: Vec::new(),
        }
    }

    fn unknown(process: ProcessInstanceKey, reason: ApplicationIdentityUnavailableReason) -> Self {
        Self {
            process,
            identity: ApplicationIdentity::Unknown,
            source: ApplicationIdentitySource::Unavailable(reason),
            inherited_through: Vec::new(),
        }
    }

    fn inherited(process: ProcessInstanceKey, parent: Self) -> Self {
        let inherited_through = std::iter::once(parent.process.clone())
            .chain(parent.inherited_through)
            .collect();
        Self {
            process,
            identity: parent.identity,
            source: parent.source,
            inherited_through,
        }
    }
}

/// Deterministic resolver over adapter-provided observations. The OS adapter is
/// responsible for process discovery, permission handling and mapping package
/// metadata into these facts. The resolver never reads a command line.
#[derive(Debug, Clone, Default)]
pub struct ApplicationIdentityResolver {
    observations: BTreeMap<ProcessInstanceKey, ProcessApplicationObservation>,
}

impl ApplicationIdentityResolver {
    /// Insert or refresh one observation. Refreshing the same process instance
    /// replaces stale metadata; a new process instance must use a new
    /// start-time token even when the PID is reused.
    pub fn upsert(&mut self, observation: ProcessApplicationObservation) -> DomainResult<()> {
        observation.validate()?;
        self.observations
            .insert(observation.process.clone(), observation);
        Ok(())
    }

    pub fn remove(&mut self, process: &ProcessInstanceKey) -> bool {
        self.observations.remove(process).is_some()
    }

    pub fn observation(
        &self,
        process: &ProcessInstanceKey,
    ) -> Option<&ProcessApplicationObservation> {
        self.observations.get(process)
    }

    pub fn len(&self) -> usize {
        self.observations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    /// Resolve one exact process instance. Missing metadata, PID reuse with a
    /// different start token, evicted short-lived processes and unsupported
    /// mappings all return `Unknown` rather than borrowing another process's
    /// identity.
    pub fn resolve(&self, process: &ProcessInstanceKey) -> ApplicationIdentityResolution {
        self.resolve_inner(process, &mut BTreeSet::new(), 0)
    }

    fn resolve_inner(
        &self,
        process: &ProcessInstanceKey,
        visiting: &mut BTreeSet<ProcessInstanceKey>,
        depth: usize,
    ) -> ApplicationIdentityResolution {
        if depth >= 64 {
            return ApplicationIdentityResolution::unknown(
                process.clone(),
                ApplicationIdentityUnavailableReason::InheritanceDepthExceeded,
            );
        }
        let Some(observation) = self.observations.get(process) else {
            return ApplicationIdentityResolution::unknown(
                process.clone(),
                ApplicationIdentityUnavailableReason::ProcessObservationUnavailable,
            );
        };
        if !visiting.insert(process.clone()) {
            return ApplicationIdentityResolution::unknown(
                process.clone(),
                ApplicationIdentityUnavailableReason::InheritanceCycle,
            );
        }

        let resolution = match observation.class {
            ApplicationProcessClass::Application => direct_identity(observation),
            ApplicationProcessClass::OwnedChild => {
                let Some(parent) = observation.parent.as_ref() else {
                    visiting.remove(process);
                    return ApplicationIdentityResolution::unknown(
                        process.clone(),
                        ApplicationIdentityUnavailableReason::ParentIdentityUnavailable,
                    );
                };
                let parent_resolution = self.resolve_inner(parent, visiting, depth + 1);
                if parent_resolution.identity.is_unknown() {
                    let reason = match parent_resolution.source {
                        ApplicationIdentitySource::Unavailable(
                            ApplicationIdentityUnavailableReason::InheritanceCycle,
                        ) => ApplicationIdentityUnavailableReason::InheritanceCycle,
                        ApplicationIdentitySource::Unavailable(
                            ApplicationIdentityUnavailableReason::InheritanceDepthExceeded,
                        ) => ApplicationIdentityUnavailableReason::InheritanceDepthExceeded,
                        _ => ApplicationIdentityUnavailableReason::ParentIdentityUnavailable,
                    };
                    let mut unknown =
                        ApplicationIdentityResolution::unknown(process.clone(), reason);
                    unknown.inherited_through.push(parent.clone());
                    unknown
                } else {
                    ApplicationIdentityResolution::inherited(process.clone(), parent_resolution)
                }
            }
            ApplicationProcessClass::Interpreter => {
                if let Some(parent) = observation.parent.as_ref() {
                    let parent_resolution = self.resolve_inner(parent, visiting, depth + 1);
                    if !parent_resolution.identity.is_unknown() {
                        ApplicationIdentityResolution::inherited(process.clone(), parent_resolution)
                    } else {
                        direct_identity(observation)
                    }
                } else {
                    direct_identity(observation)
                }
            }
            ApplicationProcessClass::Container => {
                if observation.identity.is_unknown() {
                    ApplicationIdentityResolution::unknown(
                        process.clone(),
                        ApplicationIdentityUnavailableReason::ContainerMappingUnavailable,
                    )
                } else {
                    direct_identity(observation)
                }
            }
            ApplicationProcessClass::Service => {
                if observation.identity.is_unknown() {
                    ApplicationIdentityResolution::unknown(
                        process.clone(),
                        ApplicationIdentityUnavailableReason::ServiceRegistrationUnavailable,
                    )
                } else {
                    direct_identity(observation)
                }
            }
        };

        visiting.remove(process);
        resolution
    }
}

fn direct_identity(observation: &ProcessApplicationObservation) -> ApplicationIdentityResolution {
    if observation.identity.is_unknown() {
        ApplicationIdentityResolution::unknown(
            observation.process.clone(),
            ApplicationIdentityUnavailableReason::DirectIdentityUnavailable,
        )
    } else {
        ApplicationIdentityResolution::direct(
            observation.process.clone(),
            observation.identity.clone(),
            observation.source.clone(),
        )
    }
}

fn source_matches_identity(
    source: &ApplicationIdentitySource,
    identity: &ApplicationIdentity,
) -> bool {
    matches!(
        (source, identity),
        (
            ApplicationIdentitySource::Unavailable(_),
            ApplicationIdentity::Unknown
        ) | (
            ApplicationIdentitySource::ExecutablePath
                | ApplicationIdentitySource::ServiceRegistration
                | ApplicationIdentitySource::ContainerMapping,
            ApplicationIdentity::ExecutablePath(_)
        ) | (
            ApplicationIdentitySource::PlatformPackage
                | ApplicationIdentitySource::ServiceRegistration
                | ApplicationIdentitySource::ContainerMapping,
            ApplicationIdentity::PackageId(_)
        )
    )
}

fn has_duplicates<T: Ord>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, start_time_token: u64) -> ProcessInstanceKey {
        ProcessInstanceKey::new(pid, start_time_token).unwrap()
    }

    fn observation(
        process: ProcessInstanceKey,
        parent: Option<ProcessInstanceKey>,
        class: ApplicationProcessClass,
        identity: ApplicationIdentity,
        source: ApplicationIdentitySource,
    ) -> ProcessApplicationObservation {
        ProcessApplicationObservation {
            process,
            parent,
            class,
            lifetime: ApplicationProcessLifetime::Persistent,
            identity,
            source,
        }
    }

    fn executable(path: &str) -> ApplicationIdentity {
        ApplicationIdentity::ExecutablePath(path.into())
    }

    #[test]
    fn platform_matrix_limits_identity_formats() {
        assert_eq!(
            ApplicationIdentityPlatform::Linux.declared_formats(),
            &[ApplicationIdentityFormat::ExecutablePath]
        );
        assert_eq!(
            ApplicationIdentityPlatform::Android.declared_formats(),
            &[ApplicationIdentityFormat::PackageId]
        );
        assert!(ApplicationIdentityPlatform::MacOs
            .supports_format(ApplicationIdentityFormat::PackageId));
        assert!(ApplicationIdentityPlatform::Windows
            .supports_format(ApplicationIdentityFormat::ExecutablePath));
        assert!(!ApplicationIdentityPlatform::Linux
            .supports_format(ApplicationIdentityFormat::PackageId));
    }

    #[test]
    fn disabled_capabilities_explain_why_application_conditions_are_unavailable() {
        let capability = ApplicationIdentityCapability::unsupported(
            ApplicationIdentityPlatform::Android,
            "the platform adapter cannot map this process namespace",
        )
        .unwrap();
        assert!(!capability.is_enabled());
        assert_eq!(
            capability.disabled_explanation(),
            Some("the platform adapter cannot map this process namespace")
        );
        assert!(capability.formats.is_empty());

        assert!(
            ApplicationIdentityCapability::unavailable(ApplicationIdentityPlatform::Linux, "",)
                .is_err()
        );
        assert!(ApplicationIdentityCapability::available(
            ApplicationIdentityPlatform::Linux,
            [ApplicationIdentityFormat::PackageId],
        )
        .is_err());
    }

    #[test]
    fn direct_identity_records_executable_source() {
        let process = process(100, 1);
        let mut resolver = ApplicationIdentityResolver::default();
        resolver
            .upsert(observation(
                process.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/usr/bin/browser"),
                ApplicationIdentitySource::ExecutablePath,
            ))
            .unwrap();

        assert_eq!(
            resolver.resolve(&process),
            ApplicationIdentityResolution {
                process,
                identity: executable("/usr/bin/browser"),
                source: ApplicationIdentitySource::ExecutablePath,
                inherited_through: Vec::new(),
            }
        );
    }

    #[test]
    fn owned_children_inherit_even_when_they_have_their_own_executable() {
        let parent = process(200, 10);
        let child = process(201, 11);
        let mut resolver = ApplicationIdentityResolver::default();
        resolver
            .upsert(observation(
                parent.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/opt/browser/browser"),
                ApplicationIdentitySource::ExecutablePath,
            ))
            .unwrap();
        resolver
            .upsert(observation(
                child.clone(),
                Some(parent.clone()),
                ApplicationProcessClass::OwnedChild,
                executable("/opt/browser/renderer"),
                ApplicationIdentitySource::ExecutablePath,
            ))
            .unwrap();

        let resolved = resolver.resolve(&child);
        assert_eq!(resolved.identity, executable("/opt/browser/browser"));
        assert_eq!(resolved.source, ApplicationIdentitySource::ExecutablePath);
        assert_eq!(resolved.inherited_through, vec![parent]);
    }

    #[test]
    fn interpreter_inherits_known_parent_but_falls_back_to_its_own_executable() {
        let parent = process(300, 20);
        let child = process(301, 21);
        let standalone = process(302, 22);
        let mut resolver = ApplicationIdentityResolver::default();
        for item in [
            observation(
                parent.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/usr/bin/editor"),
                ApplicationIdentitySource::ExecutablePath,
            ),
            observation(
                child.clone(),
                Some(parent),
                ApplicationProcessClass::Interpreter,
                executable("/usr/bin/python3"),
                ApplicationIdentitySource::ExecutablePath,
            ),
            observation(
                standalone.clone(),
                None,
                ApplicationProcessClass::Interpreter,
                executable("/usr/bin/python3"),
                ApplicationIdentitySource::ExecutablePath,
            ),
        ] {
            resolver.upsert(item).unwrap();
        }

        assert_eq!(
            resolver.resolve(&child).identity,
            executable("/usr/bin/editor")
        );
        assert_eq!(
            resolver.resolve(&standalone).identity,
            executable("/usr/bin/python3")
        );
    }

    #[test]
    fn containers_and_services_never_inherit_a_supervisor_identity() {
        let supervisor = process(400, 30);
        let container = process(401, 31);
        let service = process(402, 32);
        let mapped_container = process(403, 33);
        let registered_service = process(404, 34);
        let mut resolver = ApplicationIdentityResolver::default();
        for item in [
            observation(
                supervisor.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/usr/bin/terminal"),
                ApplicationIdentitySource::ExecutablePath,
            ),
            observation(
                container.clone(),
                Some(supervisor.clone()),
                ApplicationProcessClass::Container,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::ContainerMappingUnavailable,
                ),
            ),
            observation(
                service.clone(),
                Some(supervisor),
                ApplicationProcessClass::Service,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::ServiceRegistrationUnavailable,
                ),
            ),
            observation(
                mapped_container.clone(),
                None,
                ApplicationProcessClass::Container,
                ApplicationIdentity::PackageId("com.example.workload".into()),
                ApplicationIdentitySource::ContainerMapping,
            ),
            observation(
                registered_service.clone(),
                None,
                ApplicationProcessClass::Service,
                ApplicationIdentity::PackageId("com.example.service".into()),
                ApplicationIdentitySource::ServiceRegistration,
            ),
        ] {
            resolver.upsert(item).unwrap();
        }

        let container_resolution = resolver.resolve(&container);
        assert!(container_resolution.identity.is_unknown());
        assert_eq!(
            container_resolution.source,
            ApplicationIdentitySource::Unavailable(
                ApplicationIdentityUnavailableReason::ContainerMappingUnavailable
            )
        );
        assert!(resolver.resolve(&service).identity.is_unknown());
        assert_eq!(
            resolver.resolve(&mapped_container).identity,
            ApplicationIdentity::PackageId("com.example.workload".into())
        );
        assert_eq!(
            resolver.resolve(&registered_service).source,
            ApplicationIdentitySource::ServiceRegistration
        );
    }

    #[test]
    fn short_lived_process_that_vanished_is_unknown() {
        let process = process(500, 40);
        let mut resolver = ApplicationIdentityResolver::default();
        let mut short_lived = observation(
            process.clone(),
            None,
            ApplicationProcessClass::Application,
            executable("/usr/bin/curl"),
            ApplicationIdentitySource::ExecutablePath,
        );
        short_lived.lifetime = ApplicationProcessLifetime::ShortLived;
        resolver.upsert(short_lived).unwrap();
        assert_eq!(
            resolver.resolve(&process).identity,
            executable("/usr/bin/curl")
        );

        assert!(resolver.remove(&process));
        let resolved = resolver.resolve(&process);
        assert!(resolved.identity.is_unknown());
        assert_eq!(
            resolved.source,
            ApplicationIdentitySource::Unavailable(
                ApplicationIdentityUnavailableReason::ProcessObservationUnavailable
            )
        );
    }

    #[test]
    fn pid_reuse_uses_the_start_time_token() {
        let old_parent = process(600, 50);
        let new_parent = process(600, 51);
        let old_child = process(601, 52);
        let mut resolver = ApplicationIdentityResolver::default();
        for item in [
            observation(
                old_parent.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/usr/bin/old-app"),
                ApplicationIdentitySource::ExecutablePath,
            ),
            observation(
                new_parent.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/usr/bin/new-app"),
                ApplicationIdentitySource::ExecutablePath,
            ),
            observation(
                old_child.clone(),
                Some(old_parent),
                ApplicationProcessClass::OwnedChild,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::DirectIdentityUnavailable,
                ),
            ),
        ] {
            resolver.upsert(item).unwrap();
        }

        assert_eq!(
            resolver.resolve(&old_child).identity,
            executable("/usr/bin/old-app")
        );
        assert_eq!(
            resolver.resolve(&new_parent).identity,
            executable("/usr/bin/new-app")
        );
    }

    #[test]
    fn refreshing_same_process_instance_replaces_stale_app_metadata() {
        let process = process(700, 60);
        let mut resolver = ApplicationIdentityResolver::default();
        resolver
            .upsert(observation(
                process.clone(),
                None,
                ApplicationProcessClass::Application,
                executable("/opt/example/app-v1"),
                ApplicationIdentitySource::ExecutablePath,
            ))
            .unwrap();
        resolver
            .upsert(observation(
                process.clone(),
                None,
                ApplicationProcessClass::Application,
                ApplicationIdentity::PackageId("com.example.app.v2".into()),
                ApplicationIdentitySource::PlatformPackage,
            ))
            .unwrap();

        assert_eq!(
            resolver.resolve(&process).identity,
            ApplicationIdentity::PackageId("com.example.app.v2".into())
        );
    }

    #[test]
    fn inheritance_cycles_and_missing_parents_fail_closed() {
        let first = process(800, 70);
        let second = process(801, 71);
        let missing_parent = process(802, 72);
        let orphan = process(803, 73);
        let mut resolver = ApplicationIdentityResolver::default();
        for item in [
            observation(
                first.clone(),
                Some(second.clone()),
                ApplicationProcessClass::OwnedChild,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::DirectIdentityUnavailable,
                ),
            ),
            observation(
                second,
                Some(first.clone()),
                ApplicationProcessClass::OwnedChild,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::DirectIdentityUnavailable,
                ),
            ),
            observation(
                orphan.clone(),
                Some(missing_parent),
                ApplicationProcessClass::OwnedChild,
                ApplicationIdentity::Unknown,
                ApplicationIdentitySource::Unavailable(
                    ApplicationIdentityUnavailableReason::DirectIdentityUnavailable,
                ),
            ),
        ] {
            resolver.upsert(item).unwrap();
        }

        assert_eq!(
            resolver.resolve(&first).source,
            ApplicationIdentitySource::Unavailable(
                ApplicationIdentityUnavailableReason::InheritanceCycle
            )
        );
        assert_eq!(
            resolver.resolve(&orphan).source,
            ApplicationIdentitySource::Unavailable(
                ApplicationIdentityUnavailableReason::ParentIdentityUnavailable
            )
        );
    }

    #[test]
    fn observation_contract_rejects_command_line_data() {
        let value = serde_json::json!({
            "process": {"pid": 900, "start_time_token": 80},
            "parent": null,
            "class": "application",
            "lifetime": "persistent",
            "identity": {"kind": "executable_path", "value": "/usr/bin/app"},
            "source": {"kind": "executable_path"},
            "command_line": ["/usr/bin/app", "--secret"]
        });
        assert!(serde_json::from_value::<ProcessApplicationObservation>(value).is_err());

        let observation = observation(
            process(901, 81),
            None,
            ApplicationProcessClass::Application,
            executable("/usr/bin/app"),
            ApplicationIdentitySource::ExecutablePath,
        );
        let serialized = serde_json::to_value(observation).unwrap();
        assert!(serialized.get("command_line").is_none());
        assert!(serialized.get("arguments").is_none());
        assert!(serialized.get("environment").is_none());
    }
}
