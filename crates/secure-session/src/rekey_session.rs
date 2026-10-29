//! Atomic bridge between the online rekey state machine and data ciphers.
//!
//! [`RekeySession`] owns both the active [`EpochSession`] and its
//! [`RekeyStateMachine`], so callers cannot advance one without the other. New
//! ciphers are validated against the negotiated SID, epoch, direction pair and
//! active epoch before they can be installed. The responder stages keys during
//! `REKEY_ACCEPT` and installs them only after a valid `REKEY_CONFIRM`; the
//! initiator installs them only after validating the responder's ACCEPT
//! confirmation. Installation uses [`EpochSession::rotate`], retaining the old
//! receive epoch for the bounded in-flight grace period.

use crate::rekey::{
    RekeyAccept, RekeyCompletion, RekeyConfirm, RekeyError, RekeyInit, RekeyNonce, RekeyPhase,
    RekeyStateMachine,
};
use crate::{EpochSession, ReceiveCipher, SendCipher, SessionError, SESSION_ID_LEN};
use std::time::Instant;
use thiserror::Error;

struct PreparedEpoch {
    new_epoch: u16,
    new_session_id: [u8; SESSION_ID_LEN],
    send: SendCipher,
    receive: ReceiveCipher,
}

impl PreparedEpoch {
    fn new(
        session: &EpochSession,
        new_epoch: u16,
        new_session_id: [u8; SESSION_ID_LEN],
        send: SendCipher,
        receive: ReceiveCipher,
    ) -> Result<Self, RekeySessionError> {
        if send.session_id() != new_session_id
            || receive.session_id() != new_session_id
            || send.epoch() != new_epoch
            || receive.epoch() != new_epoch
        {
            return Err(RekeySessionError::MaterialMismatch);
        }
        session.validate_rotation(&send, &receive)?;
        Ok(Self {
            new_epoch,
            new_session_id,
            send,
            receive,
        })
    }

    fn matches(&self, new_epoch: u16, new_session_id: [u8; SESSION_ID_LEN]) -> bool {
        self.new_epoch == new_epoch && self.new_session_id == new_session_id
    }
}

/// Errors returned by the combined rekey and data-epoch state machine.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RekeySessionError {
    /// The authenticated rekey state machine rejected the transition.
    #[error("rekey state machine rejected the transition: {0}")]
    State(#[from] RekeyError),
    /// The proposed data ciphers are internally inconsistent or roll back the epoch.
    #[error("rekey data epoch is invalid: {0}")]
    Session(#[from] SessionError),
    /// Ciphers do not match the SID or epoch bound by the rekey message.
    #[error("rekey ciphers do not match the negotiated SID and epoch")]
    MaterialMismatch,
    /// The state machine expects staged epoch material that is not available.
    #[error("rekey epoch material is unavailable")]
    MissingMaterial,
    /// An internal transition failed after the state machine advanced.
    #[error("rekey state and data epoch diverged; session is unusable")]
    Desynchronized,
}

/// Authenticated responder material used to accept a peer rekey proposal.
///
/// The digest and confirmation fields must already be verified by the caller.
/// `send` and `receive` are staged until the matching `REKEY_CONFIRM` arrives.
pub struct RekeyAcceptOffer {
    pub init: RekeyInit,
    pub responder_nonce: RekeyNonce,
    pub transcript_hash: crate::rekey::RekeyDigest,
    pub responder_confirmation: crate::rekey::RekeyDigest,
    pub expected_initiator_confirmation: crate::rekey::RekeyDigest,
    pub new_send: SendCipher,
    pub new_receive: ReceiveCipher,
}

/// An authenticated data session with atomic online rekeying.
///
/// The caller remains responsible for authenticating control messages and
/// deriving the directionally correct data ciphers before passing them here.
/// Rekey metadata alone never changes the active data epoch.
pub struct RekeySession {
    session: EpochSession,
    rekey: RekeyStateMachine,
    prepared: Option<PreparedEpoch>,
    poisoned: bool,
}

impl RekeySession {
    /// Create an idle rekey controller for an authenticated active epoch.
    pub fn new(session: EpochSession) -> Self {
        let rekey = RekeyStateMachine::new(session.epoch(), session.session_id());
        Self {
            session,
            rekey,
            prepared: None,
            poisoned: false,
        }
    }

    /// Active session identifier.
    pub const fn session_id(&self) -> [u8; SESSION_ID_LEN] {
        self.session.session_id()
    }

    /// Active data epoch.
    pub const fn epoch(&self) -> u16 {
        self.session.epoch()
    }

    /// Observable rekey phase.
    pub const fn phase(&self) -> RekeyPhase {
        self.rekey.phase()
    }

    /// Whether a rekey transaction currently consumes the half-open budget.
    pub const fn is_half_open(&self) -> bool {
        self.rekey.is_half_open()
    }

    /// Whether the active epoch must be replaced before more data is sent.
    pub fn rekey_required(&self, now: Instant) -> bool {
        self.poisoned || self.session.rekey_required(now)
    }

    /// Encrypt a packet under the active epoch.
    pub fn seal(
        &mut self,
        plaintext: &[u8],
        packet: &mut [u8],
        now: Instant,
    ) -> Result<usize, SessionError> {
        if self.poisoned {
            return Err(SessionError::InvalidEpochState);
        }
        self.session.seal(plaintext, packet, now)
    }

    /// Authenticate and decrypt a packet under the active or retained epoch.
    pub fn open_in_place<'a>(
        &mut self,
        packet: &'a mut [u8],
        now: Instant,
    ) -> Result<&'a mut [u8], SessionError> {
        if self.poisoned {
            return Err(SessionError::InvalidEpochState);
        }
        self.session.open_in_place(packet, now)
    }

    /// Start a locally initiated rekey transaction.
    pub fn initiate(
        &mut self,
        new_epoch: u16,
        new_session_id: [u8; SESSION_ID_LEN],
        initiator_nonce: RekeyNonce,
        now: Instant,
    ) -> Result<RekeyInit, RekeySessionError> {
        self.ensure_healthy()?;
        self.expire(now);
        self.rekey
            .initiate(new_epoch, new_session_id, initiator_nonce, now)
            .map_err(Into::into)
    }

    /// Accept a peer proposal and stage, but do not install, the new epoch keys.
    pub fn accept_peer_init(
        &mut self,
        offer: RekeyAcceptOffer,
        now: Instant,
    ) -> Result<RekeyAccept, RekeySessionError> {
        self.ensure_healthy()?;
        self.expire(now);
        if self.rekey.phase() != RekeyPhase::Idle {
            return self
                .rekey
                .accept_init(
                    &offer.init,
                    offer.responder_nonce,
                    offer.transcript_hash,
                    offer.responder_confirmation,
                    offer.expected_initiator_confirmation,
                    now,
                )
                .map_err(Into::into);
        }

        let prepared = PreparedEpoch::new(
            &self.session,
            offer.init.new_epoch,
            offer.init.new_session_id,
            offer.new_send,
            offer.new_receive,
        )?;
        let accept = self.rekey.accept_init(
            &offer.init,
            offer.responder_nonce,
            offer.transcript_hash,
            offer.responder_confirmation,
            offer.expected_initiator_confirmation,
            now,
        )?;
        self.prepared = Some(prepared);
        Ok(accept)
    }

    /// Validate the responder confirmation and atomically install the initiator epoch.
    ///
    /// Repeating the exact ACCEPT after installation is idempotent; the newly
    /// supplied ciphers are discarded without replacing the active epoch.
    pub fn handle_accept(
        &mut self,
        accept: &RekeyAccept,
        initiator_confirmation: crate::rekey::RekeyDigest,
        new_send: SendCipher,
        new_receive: ReceiveCipher,
        now: Instant,
    ) -> Result<RekeyConfirm, RekeySessionError> {
        self.ensure_healthy()?;
        self.expire(now);
        if self.rekey.phase() != RekeyPhase::AwaitingAccept {
            return self
                .rekey
                .handle_accept(accept, initiator_confirmation, now)
                .map_err(Into::into);
        }

        let prepared = PreparedEpoch::new(
            &self.session,
            accept.init.new_epoch,
            accept.init.new_session_id,
            new_send,
            new_receive,
        )?;
        let confirm = self
            .rekey
            .handle_accept(accept, initiator_confirmation, now)?;
        self.install(prepared, now)?;
        Ok(confirm)
    }

    /// Activate the responder epoch after the initiator confirmation is verified.
    pub fn handle_confirm(
        &mut self,
        confirm: &RekeyConfirm,
        now: Instant,
    ) -> Result<RekeyCompletion, RekeySessionError> {
        self.ensure_healthy()?;
        self.expire(now);
        if self.rekey.phase() != RekeyPhase::AwaitingConfirm {
            return self.rekey.handle_confirm(confirm, now).map_err(Into::into);
        }

        let prepared = self
            .prepared
            .take()
            .ok_or(RekeySessionError::MissingMaterial)?;
        if !prepared.matches(
            confirm.accept.init.new_epoch,
            confirm.accept.init.new_session_id,
        ) {
            self.prepared = Some(prepared);
            return Err(RekeySessionError::MaterialMismatch);
        }
        if let Err(error) = self
            .session
            .validate_rotation(&prepared.send, &prepared.receive)
        {
            self.prepared = Some(prepared);
            return Err(error.into());
        }

        let completion = match self.rekey.handle_confirm(confirm, now) {
            Ok(completion) => completion,
            Err(error) => {
                self.prepared = Some(prepared);
                return Err(error.into());
            }
        };
        self.install(prepared, now)?;
        Ok(completion)
    }

    /// Expire a half-open transaction and discard any staged epoch keys.
    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self.rekey.expire(now);
        if expired {
            self.prepared = None;
        }
        expired
    }

    fn install(&mut self, prepared: PreparedEpoch, now: Instant) -> Result<(), RekeySessionError> {
        if let Err(error) = self.session.rotate(prepared.send, prepared.receive, now) {
            self.poisoned = true;
            return Err(error.into());
        }
        Ok(())
    }

    fn ensure_healthy(&self) -> Result<(), RekeySessionError> {
        if self.poisoned {
            Err(RekeySessionError::Desynchronized)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekey::{REKEY_DIGEST_LEN, REKEY_NONCE_LEN, REKEY_TIMEOUT};
    use crate::{Direction, RETIRED_EPOCH_GRACE};
    use std::time::{Duration, Instant};

    const OLD_SID: [u8; SESSION_ID_LEN] = [0x11; SESSION_ID_LEN];
    const NEW_SID: [u8; SESSION_ID_LEN] = [0x22; SESSION_ID_LEN];
    const INITIATOR_INDEX: u32 = 0xa1a2_a3a4;
    const RESPONDER_INDEX: u32 = 0xb1b2_b3c4;

    fn digest(byte: u8) -> [u8; REKEY_DIGEST_LEN] {
        [byte; REKEY_DIGEST_LEN]
    }

    fn nonce(byte: u8) -> [u8; REKEY_NONCE_LEN] {
        [byte; REKEY_NONCE_LEN]
    }

    fn initiator_ciphers(
        epoch: u16,
        sid: [u8; SESSION_ID_LEN],
        key: u8,
    ) -> (SendCipher, ReceiveCipher) {
        (
            SendCipher::new(
                [key; 32],
                sid,
                Direction::InitiatorToResponder,
                epoch,
                RESPONDER_INDEX,
            ),
            ReceiveCipher::new(
                [key.wrapping_add(1); 32],
                sid,
                Direction::ResponderToInitiator,
                epoch,
                INITIATOR_INDEX,
            ),
        )
    }

    fn responder_ciphers(
        epoch: u16,
        sid: [u8; SESSION_ID_LEN],
        key: u8,
    ) -> (SendCipher, ReceiveCipher) {
        (
            SendCipher::new(
                [key.wrapping_add(1); 32],
                sid,
                Direction::ResponderToInitiator,
                epoch,
                INITIATOR_INDEX,
            ),
            ReceiveCipher::new(
                [key; 32],
                sid,
                Direction::InitiatorToResponder,
                epoch,
                RESPONDER_INDEX,
            ),
        )
    }

    fn sessions(now: Instant) -> (RekeySession, RekeySession) {
        let (send, receive) = initiator_ciphers(1, OLD_SID, 0x10);
        let initiator =
            RekeySession::new(EpochSession::new(send, receive, now).expect("initiator epoch"));
        let (send, receive) = responder_ciphers(1, OLD_SID, 0x10);
        let responder =
            RekeySession::new(EpochSession::new(send, receive, now).expect("responder epoch"));
        (initiator, responder)
    }

    fn confirm_initiator(
        initiator: &mut RekeySession,
        init: &RekeyInit,
        accept: &RekeyAccept,
        now: Instant,
    ) -> RekeyConfirm {
        assert_eq!(accept.init, *init);
        let (send, receive) = initiator_ciphers(2, NEW_SID, 0x40);
        initiator
            .handle_accept(accept, digest(0x35), send, receive, now)
            .expect("install initiator epoch")
    }

    #[test]
    fn rekey_installs_both_endpoints_only_after_confirmation_and_graces_old_receive() {
        let now = Instant::now();
        let (mut initiator, mut responder) = sessions(now);
        let mut old_i1 = [0u8; 128];
        let mut old_i2 = [0u8; 128];
        let mut old_i3 = [0u8; 128];
        let old_i1_len = initiator
            .seal(b"old i1", &mut old_i1, now)
            .expect("seal old i1");
        let old_i2_len = initiator
            .seal(b"old i2", &mut old_i2, now)
            .expect("seal old i2");
        let old_i3_len = initiator
            .seal(b"old i3", &mut old_i3, now)
            .expect("seal old i3");
        let mut old_r = [0u8; 128];
        let old_r_len = responder
            .seal(b"old response", &mut old_r, now)
            .expect("seal old response");

        let init = initiator
            .initiate(2, NEW_SID, nonce(0x31), now)
            .expect("initiate rekey");
        let (send, receive) = responder_ciphers(2, NEW_SID, 0x40);
        let accept = responder
            .accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            )
            .expect("accept rekey");

        assert_eq!(initiator.epoch(), 1);
        assert_eq!(responder.epoch(), 1);
        let confirm =
            confirm_initiator(&mut initiator, &init, &accept, now + Duration::from_secs(1));
        assert_eq!(initiator.epoch(), 2);
        assert_eq!(responder.epoch(), 1);

        assert_eq!(
            initiator
                .open_in_place(&mut old_r[..old_r_len], now + Duration::from_secs(1))
                .expect("initiator opens old response in grace"),
            b"old response"
        );
        assert_eq!(
            responder
                .open_in_place(&mut old_i1[..old_i1_len], now + Duration::from_secs(1))
                .expect("responder still accepts old active epoch"),
            b"old i1"
        );

        assert_eq!(
            responder
                .handle_confirm(&confirm, now + Duration::from_secs(2))
                .expect("activate responder"),
            RekeyCompletion::Activated {
                previous_epoch: 1,
                new_epoch: 2,
                new_session_id: NEW_SID,
            }
        );
        assert_eq!(responder.epoch(), 2);
        assert_eq!(
            responder
                .open_in_place(&mut old_i2[..old_i2_len], now + Duration::from_secs(2))
                .expect("responder opens old packet in grace"),
            b"old i2"
        );
        assert_eq!(
            responder.open_in_place(
                &mut old_i3[..old_i3_len],
                now + Duration::from_secs(2) + RETIRED_EPOCH_GRACE
            ),
            Err(SessionError::UnknownEpoch)
        );

        let mut new_i = [0u8; 128];
        let new_i_len = initiator
            .seal(b"new request", &mut new_i, now + Duration::from_secs(3))
            .expect("seal new request");
        assert_eq!(
            responder
                .open_in_place(&mut new_i[..new_i_len], now + Duration::from_secs(3))
                .expect("open new request"),
            b"new request"
        );

        let mut new_r = [0u8; 128];
        let new_r_len = responder
            .seal(b"new response", &mut new_r, now + Duration::from_secs(3))
            .expect("seal new response");
        assert_eq!(
            initiator
                .open_in_place(&mut new_r[..new_r_len], now + Duration::from_secs(3))
                .expect("open new response"),
            b"new response"
        );

        let (send, receive) = initiator_ciphers(2, NEW_SID, 0x40);
        assert_eq!(
            initiator
                .handle_accept(
                    &accept,
                    digest(0x35),
                    send,
                    receive,
                    now + Duration::from_secs(3)
                )
                .expect("duplicate accept"),
            confirm
        );
        assert_eq!(initiator.epoch(), 2);

        assert_eq!(
            responder
                .handle_confirm(&confirm, now + Duration::from_secs(3))
                .expect("duplicate confirmation"),
            RekeyCompletion::Duplicate
        );
        assert_eq!(responder.epoch(), 2);
    }

    #[test]
    fn mismatched_material_and_bad_confirm_preserve_the_old_epoch() {
        let now = Instant::now();
        let (mut initiator, mut responder) = sessions(now);
        let init = initiator
            .initiate(2, NEW_SID, nonce(0x31), now)
            .expect("initiate rekey");

        let (send, receive) = responder_ciphers(2, OLD_SID, 0x40);
        assert_eq!(
            responder.accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            ),
            Err(RekeySessionError::MaterialMismatch)
        );
        assert_eq!(responder.phase(), RekeyPhase::Idle);
        assert_eq!(responder.epoch(), 1);

        let (send, receive) = responder_ciphers(3, NEW_SID, 0x40);
        assert_eq!(
            responder.accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            ),
            Err(RekeySessionError::MaterialMismatch)
        );
        assert_eq!(responder.phase(), RekeyPhase::Idle);
        assert_eq!(responder.epoch(), 1);

        let send = SendCipher::new(
            [0x40; 32],
            NEW_SID,
            Direction::InitiatorToResponder,
            2,
            RESPONDER_INDEX,
        );
        let receive = ReceiveCipher::new(
            [0x40; 32],
            NEW_SID,
            Direction::InitiatorToResponder,
            2,
            RESPONDER_INDEX,
        );
        assert_eq!(
            responder.accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            ),
            Err(RekeySessionError::Session(SessionError::InvalidEpochState))
        );
        assert_eq!(responder.phase(), RekeyPhase::Idle);
        assert_eq!(responder.epoch(), 1);

        let (send, receive) = responder_ciphers(2, NEW_SID, 0x40);
        let accept = responder
            .accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            )
            .expect("valid accept");
        assert_eq!(responder.epoch(), 1);
        assert_eq!(responder.phase(), RekeyPhase::AwaitingConfirm);

        let mut bad_confirm = RekeyConfirm {
            accept,
            initiator_confirmation: digest(0x36),
        };
        assert_eq!(
            responder.handle_confirm(&bad_confirm, now),
            Err(RekeySessionError::State(RekeyError::TransactionMismatch))
        );
        assert_eq!(responder.epoch(), 1);
        assert_eq!(responder.phase(), RekeyPhase::AwaitingConfirm);

        let (send, receive) = initiator_ciphers(2, OLD_SID, 0x40);
        assert_eq!(
            initiator.handle_accept(&accept, digest(0x35), send, receive, now),
            Err(RekeySessionError::MaterialMismatch)
        );
        assert_eq!(initiator.epoch(), 1);
        assert_eq!(initiator.phase(), RekeyPhase::AwaitingAccept);

        let (send, receive) = initiator_ciphers(2, NEW_SID, 0x40);
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), send, receive, now)
            .expect("valid initiator material");
        assert_eq!(initiator.epoch(), 2);
        bad_confirm.initiator_confirmation = digest(0x35);
        assert!(matches!(
            responder
                .handle_confirm(&bad_confirm, now)
                .expect("confirmed responder epoch"),
            RekeyCompletion::Activated { .. }
        ));
        assert_eq!(responder.epoch(), 2);
        assert_eq!(confirm, bad_confirm);
    }

    #[test]
    fn timeout_discards_staged_responder_keys_without_rotation() {
        let now = Instant::now();
        let (mut initiator, mut responder) = sessions(now);
        let init = initiator
            .initiate(2, NEW_SID, nonce(0x31), now)
            .expect("initiate rekey");
        let (send, receive) = responder_ciphers(2, NEW_SID, 0x40);
        responder
            .accept_peer_init(
                RekeyAcceptOffer {
                    init,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            )
            .expect("stage responder keys");

        assert!(responder.expire(now + REKEY_TIMEOUT));
        assert_eq!(responder.epoch(), 1);
        assert_eq!(responder.phase(), RekeyPhase::Idle);
        let accept = RekeyAccept {
            init,
            responder_nonce: nonce(0x32),
            transcript_hash: digest(0x33),
            responder_confirmation: digest(0x34),
            expected_initiator_confirmation: digest(0x35),
        };
        let (send, receive) = initiator_ciphers(2, NEW_SID, 0x40);
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), send, receive, now)
            .expect("initiator accepts");
        assert_eq!(
            responder.handle_confirm(&confirm, now + REKEY_TIMEOUT),
            Err(RekeySessionError::State(RekeyError::UnexpectedMessage))
        );
        assert_eq!(responder.epoch(), 1);
    }

    #[test]
    fn expired_transaction_does_not_leak_staged_keys_into_the_next_rekey() {
        let now = Instant::now();
        let (mut initiator, mut responder) = sessions(now);
        let first = initiator
            .initiate(2, NEW_SID, nonce(0x31), now)
            .expect("first rekey");
        let (send, receive) = responder_ciphers(2, NEW_SID, 0x40);
        responder
            .accept_peer_init(
                RekeyAcceptOffer {
                    init: first,
                    responder_nonce: nonce(0x32),
                    transcript_hash: digest(0x33),
                    responder_confirmation: digest(0x34),
                    expected_initiator_confirmation: digest(0x35),
                    new_send: send,
                    new_receive: receive,
                },
                now,
            )
            .expect("stage first responder epoch");

        let later = now + REKEY_TIMEOUT;
        let third_sid = [0x44; SESSION_ID_LEN];
        let second = initiator
            .initiate(3, third_sid, nonce(0x41), later)
            .expect("first initiator transaction should expire");
        let (send, receive) = responder_ciphers(3, third_sid, 0x50);
        let accept = responder
            .accept_peer_init(
                RekeyAcceptOffer {
                    init: second,
                    responder_nonce: nonce(0x42),
                    transcript_hash: digest(0x43),
                    responder_confirmation: digest(0x44),
                    expected_initiator_confirmation: digest(0x45),
                    new_send: send,
                    new_receive: receive,
                },
                later,
            )
            .expect("replace expired responder transaction");
        let (send, receive) = initiator_ciphers(3, third_sid, 0x50);
        let confirm = initiator
            .handle_accept(&accept, digest(0x45), send, receive, later)
            .expect("install second initiator epoch");
        assert!(matches!(
            responder
                .handle_confirm(&confirm, later)
                .expect("install second responder epoch"),
            RekeyCompletion::Activated {
                previous_epoch: 1,
                new_epoch: 3,
                new_session_id,
            } if new_session_id == third_sid
        ));
        assert_eq!(initiator.epoch(), 3);
        assert_eq!(responder.epoch(), 3);
    }
}
