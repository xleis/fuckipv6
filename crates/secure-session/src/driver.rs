//! Transport-independent data-plane driver for one authenticated v2 session.
//!
//! The types in this module join the experimental classic handshake, derived
//! keys, key confirmation and online rekeying into one embeddable client API.
//! They do not encode or authenticate control messages: the transport must do
//! that before calling the rekey methods. Data packets, however, stay entirely
//! inside the driver, so callers cannot bypass epoch health checks, replay
//! protection or the retired-epoch grace policy.
//!
//! This is an adapter only. The production client data path still uses its
//! existing static-key implementation until it is migrated in a separate
//! change.

use crate::confirmation::{compute_confirmation, ConfirmationRole, ConfirmationTag};
use crate::handshake::{Accept, ClassicInitiator, ClassicResponder, Ed25519Identity, Hello};
use crate::handshake_epoch::HandshakeEpochError;
use crate::key_schedule::{derive_session_keys, DerivedSession, KeyScheduleError};
use crate::rekey::{RekeyAccept, RekeyCompletion, RekeyConfirm, RekeyInit, RekeyNonce, RekeyPhase};
use crate::rekey_session::{RekeyAcceptOffer, RekeySession, RekeySessionError};
use crate::transcript::ENDPOINT_CONTEXT_HASH_LEN;
use crate::{DataCipherSuite, EpochSession, SessionError, SESSION_ID_LEN};
use std::time::Instant;
use thiserror::Error;
use zeroize::Zeroizing;

/// Local packet-routing labels installed into a completed data epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInstallConfig {
    /// Receiver index written by the peer into packets for this endpoint.
    pub local_receiver_index: u32,
    /// Receiver index this endpoint writes into outgoing packets.
    pub peer_receiver_index: u32,
}

/// Failures returned while building or operating a [`SessionDriver`].
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    /// The driver was explicitly closed or failed closed and no longer owns keys.
    #[error("secure session driver is closed")]
    Closed,
    /// The classic handshake could not be completed.
    #[error("secure handshake failed: {0}")]
    Handshake(#[from] crate::handshake::HandshakeError),
    /// HKDF key derivation failed.
    #[error("secure session key derivation failed: {0}")]
    KeySchedule(#[from] KeyScheduleError),
    /// Peer confirmation or data-epoch installation failed.
    #[error("secure session epoch installation failed: {0}")]
    EpochInstall(#[from] HandshakeEpochError),
    /// A data packet operation failed.
    #[error("secure session data operation failed: {0}")]
    Data(#[from] SessionError),
    /// A rekey operation failed.
    #[error("secure session rekey operation failed: {0}")]
    Rekey(#[from] RekeySessionError),
}

/// Observable packet and rekey counters for one installed driver.
///
/// Counters describe events handled by this driver instance. Rejected packets
/// are counted separately and never contribute to the authenticated packet or
/// plaintext byte totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub plaintext_bytes_sent: u64,
    pub plaintext_bytes_received: u64,
    pub authentication_failures: u64,
    pub replay_rejections: u64,
    pub rekeys_started: u64,
    pub rekeys_accepted: u64,
    pub rekeys_completed: u64,
}

/// A transport-independent client data-plane driver.
///
/// The driver owns all active and retired key material. [`close`](Self::close)
/// and fatal rekey validation failures drop those keys immediately, after
/// which all packet and rekey operations return [`DriverError::Closed`].
pub struct SessionDriver {
    session: Option<RekeySession>,
    role: ConfirmationRole,
    stats: DriverStats,
}

impl SessionDriver {
    /// Verify the peer confirmation and install a handshake-derived epoch.
    ///
    /// Normal integrations should use [`InitiatorHandshake::complete`] or
    /// [`ResponderHandshake::complete`], which also derive the keys and local
    /// confirmation before reaching this constructor.
    pub fn from_handshake(
        derived: DerivedSession,
        role: ConfirmationRole,
        config: SessionInstallConfig,
        peer_confirmation: &ConfirmationTag,
        now: Instant,
    ) -> Result<Self, HandshakeEpochError> {
        let session = EpochSession::from_handshake(
            derived,
            role,
            config.local_receiver_index,
            config.peer_receiver_index,
            peer_confirmation,
            now,
        )?;
        Ok(Self {
            session: Some(RekeySession::new(session)),
            role,
            stats: DriverStats::default(),
        })
    }

    /// Local endpoint role bound into the handshake transcript.
    pub const fn role(&self) -> ConfirmationRole {
        self.role
    }

    /// Active session identifier, or `None` after the driver closes.
    pub fn session_id(&self) -> Option<[u8; SESSION_ID_LEN]> {
        self.session.as_ref().map(RekeySession::session_id)
    }

    /// Active data epoch, or `None` after the driver closes.
    pub fn epoch(&self) -> Option<u16> {
        self.session.as_ref().map(RekeySession::epoch)
    }

    /// Negotiated data-cipher suite, or `None` after the driver closes.
    pub fn suite(&self) -> Option<DataCipherSuite> {
        self.session.as_ref().map(RekeySession::suite)
    }

    /// Observable rekey phase, or `None` after the driver closes.
    pub fn rekey_phase(&self) -> Option<RekeyPhase> {
        self.session.as_ref().map(RekeySession::phase)
    }

    /// Whether a rekey transaction currently consumes the half-open budget.
    pub fn is_rekey_half_open(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(RekeySession::is_half_open)
    }

    /// Whether this driver is closed and owns no session keys.
    pub fn is_closed(&self) -> bool {
        self.session.is_none()
    }

    /// Whether the active epoch must be replaced before more data is sent.
    ///
    /// A closed driver always reports `true`.
    pub fn rekey_required(&self, now: Instant) -> bool {
        self.session
            .as_ref()
            .is_none_or(|session| session.rekey_required(now))
    }

    /// Return a snapshot of packet and rekey counters.
    pub const fn stats(&self) -> DriverStats {
        self.stats
    }

    /// Drop all active, staged and retired keys and permanently close the driver.
    pub fn close(&mut self) {
        self.session = None;
    }

    /// Encrypt one plaintext into a caller-owned packet buffer without allocating.
    pub fn seal(
        &mut self,
        plaintext: &[u8],
        packet: &mut [u8],
        now: Instant,
    ) -> Result<usize, DriverError> {
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        let packet_len = session
            .seal(plaintext, packet, now)
            .map_err(DriverError::Data)?;
        self.stats.packets_sent += 1;
        self.stats.plaintext_bytes_sent += plaintext.len() as u64;
        Ok(packet_len)
    }

    /// Authenticate and decrypt one packet under the active or retired epoch.
    pub fn open_in_place<'a>(
        &mut self,
        packet: &'a mut [u8],
        now: Instant,
    ) -> Result<&'a mut [u8], DriverError> {
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        match session.open_in_place(packet, now) {
            Ok(plaintext) => {
                self.stats.packets_received += 1;
                self.stats.plaintext_bytes_received += plaintext.len() as u64;
                Ok(plaintext)
            }
            Err(error) => {
                match error {
                    SessionError::AuthenticationFailed => {
                        self.stats.authentication_failures += 1;
                    }
                    SessionError::ReplayDetected | SessionError::CounterTooOld => {
                        self.stats.replay_rejections += 1;
                    }
                    _ => {}
                }
                Err(DriverError::Data(error))
            }
        }
    }

    /// Start a locally initiated rekey transaction.
    pub fn initiate_rekey(
        &mut self,
        new_epoch: u16,
        new_session_id: [u8; SESSION_ID_LEN],
        initiator_nonce: RekeyNonce,
        now: Instant,
    ) -> Result<RekeyInit, DriverError> {
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        match session.initiate(new_epoch, new_session_id, initiator_nonce, now) {
            Ok(init) => {
                self.stats.rekeys_started += 1;
                Ok(init)
            }
            Err(error) => Err(self.map_rekey_error(error)),
        }
    }

    /// Accept an authenticated peer proposal and stage its new epoch ciphers.
    pub fn accept_rekey(
        &mut self,
        offer: RekeyAcceptOffer,
        now: Instant,
    ) -> Result<RekeyAccept, DriverError> {
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        let starts_new_transaction = session.phase() == RekeyPhase::Idle;
        let result = session.accept_peer_init(offer, now);
        match result {
            Ok(accept) => {
                if starts_new_transaction {
                    self.stats.rekeys_accepted += 1;
                }
                Ok(accept)
            }
            Err(error) => Err(self.map_rekey_error(error)),
        }
    }

    /// Complete the initiator side of a rekey after validating the responder's
    /// authenticated acceptance and confirmation material.
    pub fn complete_rekey_initiator(
        &mut self,
        accept: &RekeyAccept,
        initiator_confirmation: [u8; 32],
        new_send: crate::SendCipher,
        new_receive: crate::ReceiveCipher,
        now: Instant,
    ) -> Result<RekeyConfirm, DriverError> {
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        let activates_epoch = session.phase() == RekeyPhase::AwaitingAccept;
        let result =
            session.handle_accept(accept, initiator_confirmation, new_send, new_receive, now);
        match result {
            Ok(confirm) => {
                if activates_epoch {
                    self.stats.rekeys_completed += 1;
                }
                Ok(confirm)
            }
            Err(error) => Err(self.map_rekey_error(error)),
        }
    }

    /// Complete the responder side of a rekey after validating the initiator's
    /// authenticated confirmation.
    pub fn complete_rekey_responder(
        &mut self,
        confirm: &RekeyConfirm,
        now: Instant,
    ) -> Result<RekeyCompletion, DriverError> {
        if self.session.is_none() {
            return Err(DriverError::Closed);
        }
        let result = self
            .session
            .as_mut()
            .expect("checked above")
            .handle_confirm(confirm, now);
        match result {
            Ok(completion) => {
                if matches!(completion, RekeyCompletion::Activated { .. }) {
                    self.stats.rekeys_completed += 1;
                }
                Ok(completion)
            }
            Err(error) => Err(self.map_rekey_error(error)),
        }
    }

    /// Expire a half-open rekey transaction and discard any staged key material.
    pub fn expire_rekey(&mut self, now: Instant) -> Result<bool, DriverError> {
        self.session
            .as_mut()
            .ok_or(DriverError::Closed)
            .map(|session| session.expire(now))
    }

    fn map_rekey_error(&mut self, error: RekeySessionError) -> DriverError {
        if matches!(
            error,
            RekeySessionError::MaterialMismatch
                | RekeySessionError::MissingMaterial
                | RekeySessionError::Session(_)
                | RekeySessionError::Desynchronized
        ) {
            self.session = None;
        }
        DriverError::Rekey(error)
    }
}

/// Initiator-side handshake and data-driver builder.
pub struct InitiatorHandshake {
    handshake: ClassicInitiator,
    config: SessionInstallConfig,
    psk: Option<Zeroizing<[u8; 32]>>,
}

impl InitiatorHandshake {
    /// Create a fresh authenticated handshake and return its `HELLO` message.
    ///
    /// `psk` is optional 256-bit pre-shared material mixed into the HKDF key
    /// schedule. The input is copied into zeroizing storage immediately.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        local_identity: &Ed25519Identity,
        expected_responder: ed25519_dalek::VerifyingKey,
        suite_id: u16,
        endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        issued_at: u64,
        config: SessionInstallConfig,
        psk: Option<&[u8; 32]>,
    ) -> Result<(Self, Hello), DriverError> {
        let handshake = ClassicInitiator::start(
            local_identity,
            expected_responder,
            suite_id,
            endpoint_context_hash,
            epoch,
            issued_at,
        )?;
        let hello = handshake.hello().clone();
        Ok((
            Self {
                handshake,
                config,
                psk: psk.copied().map(Zeroizing::new),
            },
            hello,
        ))
    }

    /// Finish the initiator handshake, verify the responder confirmation, and
    /// install a ready data-plane driver.
    ///
    /// Returns the driver and this endpoint's confirmation tag, which the
    /// caller must deliver to the responder.
    pub fn complete(
        self,
        accept: &Accept,
        peer_confirmation: &ConfirmationTag,
        issued_at: u64,
        now: Instant,
    ) -> Result<(SessionDriver, ConfirmationTag), DriverError> {
        let established = self.handshake.finish(accept, issued_at)?;
        let derived = derive_session_keys(established, self.psk.as_deref())?;
        let local_confirmation = compute_confirmation(
            &derived.keys,
            ConfirmationRole::Initiator,
            &derived.identity.transcript_hash,
        );
        let driver = SessionDriver::from_handshake(
            derived,
            ConfirmationRole::Initiator,
            self.config,
            peer_confirmation,
            now,
        )?;
        Ok((driver, local_confirmation))
    }
}

/// Responder-side handshake and data-driver builder.
pub struct ResponderHandshake {
    derived: DerivedSession,
    local_confirmation: ConfirmationTag,
    config: SessionInstallConfig,
}

impl ResponderHandshake {
    /// Validate and accept a peer `HELLO`, then prepare this endpoint's keys.
    ///
    /// Returns the pending responder and the signed `ACCEPT` message. The
    /// caller must deliver `ACCEPT` and then pass the initiator's confirmation
    /// tag to [`complete`](Self::complete).
    #[allow(clippy::too_many_arguments)]
    pub fn accept(
        responder: &mut ClassicResponder,
        hello: &Hello,
        endpoint_context_hash: [u8; ENDPOINT_CONTEXT_HASH_LEN],
        epoch: u16,
        issued_at: u64,
        config: SessionInstallConfig,
        psk: Option<&[u8; 32]>,
    ) -> Result<(Self, Accept), DriverError> {
        let accepted = responder.accept(hello, endpoint_context_hash, epoch, issued_at)?;
        let derived = derive_session_keys(accepted.established, psk)?;
        let local_confirmation = compute_confirmation(
            &derived.keys,
            ConfirmationRole::Responder,
            &derived.identity.transcript_hash,
        );
        Ok((
            Self {
                derived,
                local_confirmation,
                config,
            },
            accepted.accept,
        ))
    }

    /// This endpoint's confirmation tag, which must be sent to the initiator.
    pub const fn local_confirmation(&self) -> ConfirmationTag {
        self.local_confirmation
    }

    /// Verify the initiator confirmation and install a ready data-plane driver.
    ///
    /// Returns the driver and this endpoint's local confirmation tag.
    pub fn complete(
        self,
        peer_confirmation: &ConfirmationTag,
        now: Instant,
    ) -> Result<(SessionDriver, ConfirmationTag), DriverError> {
        let driver = SessionDriver::from_handshake(
            self.derived,
            ConfirmationRole::Responder,
            self.config,
            peer_confirmation,
            now,
        )?;
        Ok((driver, self.local_confirmation))
    }
}
