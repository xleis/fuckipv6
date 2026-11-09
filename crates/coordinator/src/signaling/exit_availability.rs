//! Coordinator-owned, short-lived Exit availability observations.
//!
//! A report is authenticated as one member's self-observation. It is never
//! merged into a network-wide capability declaration, and absence or expiry
//! remains `unknown` rather than being treated as healthy.

use anyhow::{bail, Result};
use fuckipv6_common::domain::{
    ExitAvailabilitySource, ExitContributionAvailability, NetworkId, NodeId,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// An observation older than this cannot be used as current health evidence.
pub(crate) const EXIT_AVAILABILITY_OBSERVATION_MAX_AGE_MS: u64 = 90_000;

#[derive(Debug, Clone)]
struct ExitAvailabilityObservation {
    available: bool,
    reason_code: Option<String>,
    observed_at_unix_ms: u64,
}

#[derive(Default)]
pub(crate) struct ExitAvailabilityObservationRegistry {
    observations: Mutex<BTreeMap<(NetworkId, NodeId), ExitAvailabilityObservation>>,
}

impl ExitAvailabilityObservationRegistry {
    pub(crate) fn observe(
        &self,
        network_id: NetworkId,
        node_id: NodeId,
        available: bool,
        reason_code: Option<String>,
        observed_at_unix_ms: u64,
    ) -> Result<u64> {
        validate_reason_code(reason_code.as_deref())?;
        let mut observations = self
            .observations
            .lock()
            .map_err(|_| anyhow::anyhow!("exit availability observation store lock is poisoned"))?;
        observations.insert(
            (network_id, node_id),
            ExitAvailabilityObservation {
                available,
                reason_code,
                observed_at_unix_ms,
            },
        );
        Ok(observed_at_unix_ms)
    }

    pub(crate) fn project(
        &self,
        network_id: &NetworkId,
        node_id: &NodeId,
        declared_available: Option<bool>,
        now_unix_ms: u64,
    ) -> ExitContributionAvailability {
        let Ok(observations) = self.observations.lock() else {
            return ExitContributionAvailability::declared_unknown(declared_available);
        };
        let Some(observation) = observations.get(&(network_id.clone(), node_id.clone())) else {
            return ExitContributionAvailability::declared_unknown(declared_available);
        };
        if now_unix_ms
            > observation
                .observed_at_unix_ms
                .saturating_add(EXIT_AVAILABILITY_OBSERVATION_MAX_AGE_MS)
        {
            return ExitContributionAvailability::stale(
                declared_available.unwrap_or(observation.available),
                observation.observed_at_unix_ms,
            );
        }

        let mut availability = ExitContributionAvailability::observed_with_declaration(
            declared_available,
            observation.available,
            ExitAvailabilitySource::RemotePeerReport,
            observation.observed_at_unix_ms,
        );
        if availability.state != fuckipv6_common::domain::ExitAvailabilityState::Degraded {
            if let Some(reason_code) = &observation.reason_code {
                availability.reason_code = Some(reason_code.clone());
            }
        }
        availability
    }
}

fn validate_reason_code(reason_code: Option<&str>) -> Result<()> {
    let Some(reason_code) = reason_code else {
        return Ok(());
    };
    if reason_code.is_empty()
        || reason_code.len() > 96
        || !reason_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("exit_availability.error.invalid_reason_code");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"exit-availability-network")
    }

    fn node() -> NodeId {
        NodeId::from_public_key(b"exit-availability-node")
    }

    #[test]
    fn missing_observation_stays_unknown() {
        let registry = ExitAvailabilityObservationRegistry::default();
        let availability = registry.project(&network(), &node(), Some(true), 10);
        assert_eq!(
            availability.state,
            fuckipv6_common::domain::ExitAvailabilityState::Unknown
        );
        assert_eq!(availability.declared_available, Some(true));
        assert_eq!(
            availability.reason_code.as_deref(),
            Some("exit.availability.not_observed")
        );
    }

    #[test]
    fn fresh_observation_marks_declaration_mismatch_as_degraded() {
        let registry = ExitAvailabilityObservationRegistry::default();
        assert_eq!(
            registry
                .observe(
                    network(),
                    node(),
                    false,
                    Some("exit.forwarder.port_unavailable".into()),
                    1_000,
                )
                .unwrap(),
            1_000
        );
        let availability = registry.project(&network(), &node(), Some(true), 2_000);
        assert_eq!(
            availability.state,
            fuckipv6_common::domain::ExitAvailabilityState::Degraded
        );
        assert_eq!(availability.declared_available, Some(true));
        assert_eq!(availability.observed_at_unix_ms, Some(1_000));
        assert_eq!(
            availability.reason_code.as_deref(),
            Some("exit.availability.declared_but_unavailable")
        );
    }

    #[test]
    fn expired_observation_is_unknown_and_never_healthy() {
        let registry = ExitAvailabilityObservationRegistry::default();
        registry
            .observe(network(), node(), true, None, 1_000)
            .unwrap();
        let availability = registry.project(
            &network(),
            &node(),
            Some(true),
            1_000 + EXIT_AVAILABILITY_OBSERVATION_MAX_AGE_MS + 1,
        );
        assert_eq!(
            availability.state,
            fuckipv6_common::domain::ExitAvailabilityState::Unknown
        );
        assert_eq!(
            availability.reason_code.as_deref(),
            Some("exit.availability.stale")
        );
        assert_eq!(availability.declared_available, Some(true));
    }

    #[test]
    fn malformed_reason_codes_are_rejected() {
        let registry = ExitAvailabilityObservationRegistry::default();
        assert!(registry
            .observe(
                network(),
                node(),
                false,
                Some("contains spaces and /private/path".into()),
                1,
            )
            .is_err());
    }
}
