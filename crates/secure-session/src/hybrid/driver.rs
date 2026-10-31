//! Experimental bridge from the hybrid handshake to [`SessionDriver`].
//!
//! This module composes the X-Wing + composite-identity handshake with the
//! transport-independent data-plane driver. It deliberately remains independent
//! of the production control channel: the encoded `HELLO` and `ACCEPT` frames
//! are several kilobytes and must be carried by an authenticated, bounded
//! control channel before this adapter can be deployed.
//!
//! The responder verifies the complete hybrid `CONFIRM` binding (version,
//! profile, suite, session identifier, transcript hash, and directional HMAC)
//! before installing any data key. A known-but-different hybrid suite is
//! rejected by the strict control-frame decoder rather than negotiated down.

use super::handshake::{
    HybridAccept, HybridAccepted, HybridHandshakeError, HybridHello, HybridInitiator,
    HybridResponder, HYBRID_COOKIE_LEN,
};
use super::identity::{CompositeIdentity, CompositePublicKey};
use super::profile::NegotiatedHybridProfile;
use super::wire::{HybridConfirm, HybridConfirmError, HybridControlCodecError};
use crate::confirmation::{
    compute_confirmation, ConfirmationError, ConfirmationRole, ConfirmationTag,
};
use crate::driver::{SessionDriver, SessionInstallConfig};
use crate::handshake_epoch::HandshakeEpochError;
use crate::transcript::{TranscriptHash, ENDPOINT_CONTEXT_HASH_LEN};
use crate::SESSION_ID_LEN;
use std::time::Instant;
use thiserror::Error;
use zeroize::Zeroizing;

/// Failures raised while adapting a hybrid handshake into a data driver.
#[derive(Debug, Error)]
pub enum HybridDriverError {
    /// The hybrid handshake, KEM, signatures, or responder confirmation failed.
    #[error(transparent)]
    Handshake(#[from] HybridHandshakeError),
    /// The strict hybrid control-frame decoder rejected the message.
    #[error(transparent)]
    Control(#[from] HybridControlCodecError),
    /// The initiator `CONFIRM` did not bind the completed responder handshake.
    #[error(transparent)]
    Confirm(#[from] HybridConfirmError),
    /// A fixed-size confirmation tag could not be constructed.
    #[error(transparent)]
    Confirmation(#[from] ConfirmationError),
    /// The verified keys could not be installed as a data epoch.
    #[error(transparent)]
    Install(#[from] HandshakeEpochError),
}

/// Initiator-side hybrid handshake and data-driver builder.
pub struct HybridInitiatorHandshake {
    handshake: HybridInitiator,
    profile: NegotiatedHybridProfile,
    config: SessionInstallConfig,
    psk: Option<Zeroizing<[u8; 32]>>,
}

impl HybridInitiatorHandshake {
    /// Start a hybrid handshake with a fresh X-Wing key and nonce.
    ///
    /// The PSK mode and local PSK configuration must agree before a `HELLO` is
    /// generated. The PSK is copied into zeroizing storage immediately.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        local_identity: &CompositeIdentity,
        expected_responder: CompositePublicKey,
        profile: NegotiatedHybridProfile,
        endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        psk_mode: u8,
        cookie: [u8; HYBRID_COOKIE_LEN],
        issued_at: u64,
        config: SessionInstallConfig,
        psk: Option<&[u8; 32]>,
    ) -> Result<(Self, HybridHello), HybridDriverError> {
        if !matches!((psk_mode, psk), (0, None) | (1, Some(_))) {
            return Err(HybridHandshakeError::PskModeMismatch.into());
        }
        let handshake = HybridInitiator::start(
            local_identity,
            expected_responder,
            profile,
            endpoint_context_hash,
            epoch,
            psk_mode,
            cookie,
            issued_at,
        )?;
        let hello = handshake.hello().clone();
        Ok((
            Self {
                handshake,
                profile,
                config,
                psk: psk.copied().map(Zeroizing::new),
            },
            hello,
        ))
    }

    /// Exact hybrid profile selected for this handshake.
    pub const fn profile(&self) -> NegotiatedHybridProfile {
        self.profile
    }

    /// Finish the initiator handshake, install its data epoch, and build the
    /// canonical initiator `CONFIRM` required by the responder.
    ///
    /// The responder signature, X-Wing secret, key schedule, and responder
    /// confirmation are all verified before the driver is constructed.
    pub fn complete(
        self,
        accept: &HybridAccept,
        issued_at: u64,
        now: Instant,
    ) -> Result<(SessionDriver, HybridConfirm), HybridDriverError> {
        let finished = self
            .handshake
            .finish(accept, issued_at, self.psk.as_deref())?;
        let confirm = HybridConfirm::from_finished(&finished)?;
        let responder_confirmation =
            ConfirmationTag::from_slice(&finished.accept().responder_confirmation)?;
        let (_, derived) = finished.into_parts();
        let driver = SessionDriver::from_handshake(
            derived,
            ConfirmationRole::Initiator,
            self.config,
            &responder_confirmation,
            now,
        )?;
        Ok((driver, confirm))
    }

    /// Decode an exact-profile `ACCEPT`, install the driver, and return the
    /// canonical `CONFIRM` control frame.
    pub fn complete_wire(
        self,
        accept_frame: &[u8],
        issued_at: u64,
        now: Instant,
    ) -> Result<(SessionDriver, Vec<u8>), HybridDriverError> {
        let profile = self.profile;
        let accept = HybridAccept::decode_control_for_profile(accept_frame, profile)?;
        let (driver, confirm) = self.complete(&accept, issued_at, now)?;
        Ok((driver, confirm.encode_control_for_profile(profile)?))
    }
}

/// Responder-side hybrid handshake awaiting the initiator `CONFIRM`.
pub struct HybridResponderHandshake {
    accepted: HybridAccepted,
    profile: NegotiatedHybridProfile,
    config: SessionInstallConfig,
}

impl HybridResponderHandshake {
    /// Validate and accept an initiator `HELLO`, then retain the derived keys
    /// until the initiator proves possession in `CONFIRM`.
    #[allow(clippy::too_many_arguments)]
    pub fn accept(
        responder: &mut HybridResponder,
        hello: &HybridHello,
        expected_endpoint_context: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        issued_at: u64,
        config: SessionInstallConfig,
    ) -> Result<(Self, HybridAccept), HybridDriverError> {
        let profile = responder.required_profile();
        let accepted = responder.accept(hello, expected_endpoint_context, epoch, issued_at)?;
        let accept = accepted.accept().clone();
        Ok((
            Self {
                accepted,
                profile,
                config,
            },
            accept,
        ))
    }

    /// Exact hybrid profile selected by the responder policy.
    pub const fn profile(&self) -> NegotiatedHybridProfile {
        self.profile
    }

    /// Session identifier bound into the pending hybrid handshake.
    pub const fn session_id(&self) -> [u8; SESSION_ID_LEN] {
        self.accepted.derived().identity.session_id
    }

    /// Final transcript hash bound into the pending hybrid handshake.
    pub const fn transcript_hash(&self) -> TranscriptHash {
        self.accepted.derived().identity.transcript_hash
    }

    /// Validate all `CONFIRM` bindings without consuming the pending state.
    ///
    /// Version, profile, suite, session identifier, and transcript hash are
    /// checked before the constant-time directional HMAC verification. A
    /// transport should call this method before removing pending state so a
    /// forged confirmation cannot force a fresh hybrid handshake.
    pub fn validate_confirm(&self, confirm: &HybridConfirm) -> Result<(), HybridDriverError> {
        confirm.verify_accepted(&self.accepted)?;
        Ok(())
    }

    /// Verify `CONFIRM` and install the responder data epoch.
    ///
    /// The pending handshake is consumed so one initiator confirmation can
    /// activate at most one driver. The returned tag is the responder
    /// confirmation already carried by the signed `ACCEPT`.
    pub fn complete(
        self,
        confirm: &HybridConfirm,
        now: Instant,
    ) -> Result<(SessionDriver, ConfirmationTag), HybridDriverError> {
        self.validate_confirm(confirm)?;
        let local_confirmation = compute_confirmation(
            &self.accepted.derived().keys,
            ConfirmationRole::Responder,
            &self.accepted.derived().identity.transcript_hash,
        );
        let (_, derived) = self.accepted.into_parts();
        let driver = SessionDriver::from_handshake(
            derived,
            ConfirmationRole::Responder,
            self.config,
            &confirm.initiator_confirmation,
            now,
        )?;
        Ok((driver, local_confirmation))
    }

    /// Decode an exact-profile `CONFIRM`, verify it, and install the driver.
    pub fn complete_wire(
        self,
        confirm_frame: &[u8],
        now: Instant,
    ) -> Result<(SessionDriver, ConfirmationTag), HybridDriverError> {
        let confirm = HybridConfirm::decode_control_for_profile(confirm_frame, self.profile)?;
        self.complete(&confirm, now)
    }
}
