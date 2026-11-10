//! Versioned Exit readiness control frames carried inside an authenticated
//! Secure Session data frame.
//!
//! These frames are deliberately narrow: only the two product reachability
//! targets are accepted, both directions are bounded, duplicate targets are
//! rejected, and a response must match the request nonce and target set. The
//! payload is encrypted and peer-authenticated by Secure Session v2; this
//! module adds no parallel authentication scheme.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::recovery::{
    RecoveryProbeResult, RecoveryProbeStage, RecoveryProbeState, RecoveryProbeTarget,
};

const FRAME_MAGIC: &[u8; 8] = b"FKEXRDY1";
pub(crate) const MAX_EXIT_READINESS_FRAME_LEN: usize = 4 * 1024;
pub(crate) const MAX_EXIT_READINESS_TARGETS: usize = 2;
const MAX_ERROR_CODE_LEN: usize = 96;
const MAX_PROBE_DURATION_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum ExitReadinessError {
    #[error("Exit readiness frame is too large")]
    FrameTooLarge,
    #[error("Exit readiness frame has an unknown prefix")]
    UnknownPrefix,
    #[error("Exit readiness frame is malformed")]
    Malformed,
    #[error("Exit readiness request has an invalid target set")]
    InvalidTargets,
    #[error("Exit readiness response does not match the request")]
    ResponseMismatch,
    #[error("Exit readiness response contains an invalid probe result")]
    InvalidProbeResult,
    #[error("Exit readiness frame could not be encoded")]
    EncodeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ExitReadinessFrame {
    Request {
        nonce: [u8; 16],
        targets: Vec<RecoveryProbeTarget>,
    },
    Response {
        nonce: [u8; 16],
        results: Vec<RecoveryProbeResult>,
    },
}

impl ExitReadinessFrame {
    pub(crate) fn request(
        nonce: [u8; 16],
        targets: Vec<RecoveryProbeTarget>,
    ) -> Result<Self, ExitReadinessError> {
        validate_targets(&targets)?;
        Ok(Self::Request { nonce, targets })
    }

    pub(crate) fn response(
        nonce: [u8; 16],
        results: Vec<RecoveryProbeResult>,
    ) -> Result<Self, ExitReadinessError> {
        validate_probe_results(&results)?;
        Ok(Self::Response { nonce, results })
    }

    pub(crate) fn validate_response(
        &self,
        expected_nonce: [u8; 16],
        expected_targets: &[RecoveryProbeTarget],
    ) -> Result<(), ExitReadinessError> {
        let Self::Response { nonce, results } = self else {
            return Err(ExitReadinessError::ResponseMismatch);
        };
        if *nonce != expected_nonce {
            return Err(ExitReadinessError::ResponseMismatch);
        }
        validate_targets(expected_targets)?;
        validate_probe_results(results)?;
        if results.len() != expected_targets.len() {
            return Err(ExitReadinessError::ResponseMismatch);
        }
        for target in expected_targets {
            if results
                .iter()
                .filter(|result| &result.target == target)
                .count()
                != 1
            {
                return Err(ExitReadinessError::ResponseMismatch);
            }
        }
        Ok(())
    }

    pub(crate) fn all_probes_passed(&self) -> bool {
        matches!(
            self,
            Self::Response { results, .. }
                if !results.is_empty()
                    && results
                        .iter()
                        .all(|result| result.state == RecoveryProbeState::Passed)
        )
    }
}

pub(crate) fn is_exit_readiness_frame(payload: &[u8]) -> bool {
    payload.starts_with(FRAME_MAGIC)
}

pub(crate) fn encode_exit_readiness_frame(
    frame: &ExitReadinessFrame,
) -> Result<Vec<u8>, ExitReadinessError> {
    let body = serde_json::to_vec(frame).map_err(|_| ExitReadinessError::EncodeFailed)?;
    let total = FRAME_MAGIC
        .len()
        .checked_add(body.len())
        .ok_or(ExitReadinessError::FrameTooLarge)?;
    if total > MAX_EXIT_READINESS_FRAME_LEN {
        return Err(ExitReadinessError::FrameTooLarge);
    }
    let mut encoded = Vec::with_capacity(total);
    encoded.extend_from_slice(FRAME_MAGIC);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

pub(crate) fn decode_exit_readiness_frame(
    payload: &[u8],
) -> Result<ExitReadinessFrame, ExitReadinessError> {
    if payload.len() > MAX_EXIT_READINESS_FRAME_LEN {
        return Err(ExitReadinessError::FrameTooLarge);
    }
    let body = payload
        .strip_prefix(FRAME_MAGIC)
        .ok_or(ExitReadinessError::UnknownPrefix)?;
    if body.is_empty() {
        return Err(ExitReadinessError::Malformed);
    }
    let frame: ExitReadinessFrame =
        serde_json::from_slice(body).map_err(|_| ExitReadinessError::Malformed)?;
    match &frame {
        ExitReadinessFrame::Request { targets, .. } => validate_targets(targets)?,
        ExitReadinessFrame::Response { results, .. } => validate_probe_results(results)?,
    }
    Ok(frame)
}

fn validate_targets(targets: &[RecoveryProbeTarget]) -> Result<(), ExitReadinessError> {
    if targets.is_empty() || targets.len() > MAX_EXIT_READINESS_TARGETS {
        return Err(ExitReadinessError::InvalidTargets);
    }
    let mut unique = BTreeSet::new();
    for target in targets {
        if !matches!(
            target,
            RecoveryProbeTarget::Google | RecoveryProbeTarget::YouTube
        ) {
            return Err(ExitReadinessError::InvalidTargets);
        }
        if !unique.insert(target.key()) {
            return Err(ExitReadinessError::InvalidTargets);
        }
    }
    Ok(())
}

fn validate_probe_results(results: &[RecoveryProbeResult]) -> Result<(), ExitReadinessError> {
    if results.len() > MAX_EXIT_READINESS_TARGETS {
        return Err(ExitReadinessError::InvalidProbeResult);
    }
    let mut unique = BTreeSet::new();
    for result in results {
        if !matches!(
            result.target,
            RecoveryProbeTarget::Google | RecoveryProbeTarget::YouTube
        ) || !unique.insert(result.target.key())
            || result.duration_ms > MAX_PROBE_DURATION_MS
        {
            return Err(ExitReadinessError::InvalidProbeResult);
        }
        match result.state {
            RecoveryProbeState::Passed => {
                if result.failed_stage.is_some() || result.error_code.is_some() {
                    return Err(ExitReadinessError::InvalidProbeResult);
                }
            }
            RecoveryProbeState::Failed | RecoveryProbeState::Unsupported => {
                if result.failed_stage == Some(RecoveryProbeStage::Exit)
                    || result.failed_stage == Some(RecoveryProbeStage::Relay)
                    || !result.error_code.as_deref().is_some_and(valid_error_code)
                {
                    return Err(ExitReadinessError::InvalidProbeResult);
                }
            }
        }
    }
    Ok(())
}

fn valid_error_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_ERROR_CODE_LEN
        && code.starts_with("recovery.probe_")
        && code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonce(byte: u8) -> [u8; 16] {
        [byte; 16]
    }

    fn passed(target: RecoveryProbeTarget) -> RecoveryProbeResult {
        RecoveryProbeResult {
            target,
            state: RecoveryProbeState::Passed,
            failed_stage: None,
            duration_ms: 123,
            error_code: None,
        }
    }

    fn failed(target: RecoveryProbeTarget) -> RecoveryProbeResult {
        RecoveryProbeResult {
            target,
            state: RecoveryProbeState::Failed,
            failed_stage: Some(RecoveryProbeStage::Tls),
            duration_ms: 456,
            error_code: Some("recovery.probe_tls_failed".into()),
        }
    }

    fn defaults() -> Vec<RecoveryProbeTarget> {
        vec![RecoveryProbeTarget::Google, RecoveryProbeTarget::YouTube]
    }

    #[test]
    fn readiness_frames_round_trip_with_strict_prefix() {
        let request =
            ExitReadinessFrame::request(nonce(7), defaults()).expect("valid readiness request");
        let encoded = encode_exit_readiness_frame(&request).expect("encode request");
        assert!(is_exit_readiness_frame(&encoded));
        assert_eq!(
            decode_exit_readiness_frame(&encoded).expect("decode request"),
            request
        );

        let response = ExitReadinessFrame::response(
            nonce(7),
            vec![
                passed(RecoveryProbeTarget::Google),
                failed(RecoveryProbeTarget::YouTube),
            ],
        )
        .expect("valid readiness response");
        let encoded = encode_exit_readiness_frame(&response).expect("encode response");
        decode_exit_readiness_frame(&encoded)
            .expect("decode response")
            .validate_response(nonce(7), &defaults())
            .expect("matching response");
    }

    #[test]
    fn request_targets_are_fixed_unique_and_bounded() {
        assert!(ExitReadinessFrame::request(nonce(1), Vec::new()).is_err());
        assert!(ExitReadinessFrame::request(
            nonce(1),
            vec![RecoveryProbeTarget::Google, RecoveryProbeTarget::Google]
        )
        .is_err());
        assert!(ExitReadinessFrame::request(
            nonce(1),
            vec![RecoveryProbeTarget::Controlled("deployment".into())]
        )
        .is_err());
        assert!(ExitReadinessFrame::request(
            nonce(1),
            vec![
                RecoveryProbeTarget::Google,
                RecoveryProbeTarget::YouTube,
                RecoveryProbeTarget::Google,
            ]
        )
        .is_err());
    }

    #[test]
    fn response_must_match_nonce_and_exact_target_set() {
        let response = ExitReadinessFrame::response(
            nonce(9),
            vec![
                passed(RecoveryProbeTarget::Google),
                passed(RecoveryProbeTarget::YouTube),
            ],
        )
        .expect("valid response");
        assert!(response.validate_response(nonce(8), &defaults()).is_err());
        assert!(response
            .validate_response(nonce(9), &[RecoveryProbeTarget::Google])
            .is_err());
        assert!(response.validate_response(nonce(9), &defaults()).is_ok());
        assert!(response.all_probes_passed());

        let failed = ExitReadinessFrame::response(
            nonce(9),
            vec![
                passed(RecoveryProbeTarget::Google),
                failed(RecoveryProbeTarget::YouTube),
            ],
        )
        .expect("valid failed response");
        assert!(!failed.all_probes_passed());
    }

    #[test]
    fn malformed_and_unbounded_frames_are_rejected() {
        assert_eq!(
            decode_exit_readiness_frame(b"not-a-readiness-frame"),
            Err(ExitReadinessError::UnknownPrefix)
        );
        assert_eq!(
            decode_exit_readiness_frame(b"FKEXRDY1{"),
            Err(ExitReadinessError::Malformed)
        );
        let oversized = vec![b'x'; MAX_EXIT_READINESS_FRAME_LEN + 1];
        assert_eq!(
            decode_exit_readiness_frame(&oversized),
            Err(ExitReadinessError::FrameTooLarge)
        );
    }

    #[test]
    fn response_rejects_combined_pass_and_error_evidence() {
        let mut result = passed(RecoveryProbeTarget::Google);
        result.error_code = Some("recovery.probe_tls_failed".into());
        assert!(ExitReadinessFrame::response(nonce(1), vec![result]).is_err());

        let mut result = failed(RecoveryProbeTarget::YouTube);
        result.error_code = Some("arbitrary free-form failure".into());
        assert!(ExitReadinessFrame::response(nonce(1), vec![result]).is_err());

        let mut result = failed(RecoveryProbeTarget::Google);
        result.failed_stage = Some(RecoveryProbeStage::Exit);
        assert!(ExitReadinessFrame::response(nonce(1), vec![result]).is_err());
    }
}
