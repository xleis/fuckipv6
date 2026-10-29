//! State machine for online v2 epoch rekeying.
//!
//! This module coordinates `REKEY_INIT`, `REKEY_ACCEPT`, and `REKEY_CONFIRM`
//! after the caller has authenticated and cryptographically validated their
//! contents. It deliberately does not perform X-Wing, signature, KDF, or key
//! confirmation operations itself. Those values are carried as opaque fields
//! so the state machine can be tested independently and cannot accidentally
//! establish an epoch from unverified placeholders.
//!
//! A state machine is scoped to one authenticated session. It tracks the
//! current epoch, reserves every attempted new session identifier, permits at
//! most one half-open rekey, and treats duplicate messages idempotently during
//! a bounded retransmission window.

use crate::SESSION_ID_LEN;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Maximum lifetime of one half-open rekey transaction.
pub const REKEY_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum number of simultaneous half-open rekeys per authenticated session.
pub const MAX_HALF_OPEN_REKEYS: usize = 1;
/// Length of each initiator or responder freshness nonce.
pub const REKEY_NONCE_LEN: usize = 32;
/// Length of the transcript hash and key-confirmation values.
pub const REKEY_DIGEST_LEN: usize = 32;

/// A 128-bit v2 session identifier.
pub type SessionId = [u8; SESSION_ID_LEN];
/// A 256-bit freshness nonce.
pub type RekeyNonce = [u8; REKEY_NONCE_LEN];
/// A 256-bit transcript digest or key-confirmation value.
pub type RekeyDigest = [u8; REKEY_DIGEST_LEN];

/// Initiator-to-responder rekey proposal metadata.
///
/// `initiator_nonce` must be generated from a CSPRNG for every attempt.
/// `new_session_id` must identify the independent, freshly authenticated
/// X-Wing handshake that produces the next epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyInit {
    pub base_epoch: u16,
    pub new_epoch: u16,
    pub new_session_id: SessionId,
    pub initiator_nonce: RekeyNonce,
}

/// Responder-to-initiator acceptance metadata.
///
/// The digest and responder confirmation must come from a verified handshake
/// transcript. This type only binds the fields; it is not proof of authenticity
/// on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyAccept {
    pub init: RekeyInit,
    pub responder_nonce: RekeyNonce,
    pub transcript_hash: RekeyDigest,
    pub responder_confirmation: RekeyDigest,
}

/// Initiator-to-responder confirmation of a previously accepted rekey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyConfirm {
    pub accept: RekeyAccept,
    pub initiator_confirmation: RekeyDigest,
}

/// Observable phase of a per-session rekey state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyPhase {
    /// No rekey transaction is in progress.
    Idle,
    /// This endpoint initiated a rekey and is waiting for `REKEY_ACCEPT`.
    AwaitingAccept,
    /// This endpoint accepted a peer proposal and is waiting for `REKEY_CONFIRM`.
    AwaitingConfirm,
    /// This endpoint sent `REKEY_CONFIRM` and retains it for idempotent retries.
    ConfirmSent,
}

/// Result of processing `REKEY_CONFIRM` on the responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyCompletion {
    /// The responder advanced to the confirmed epoch.
    Activated {
        previous_epoch: u16,
        new_epoch: u16,
        new_session_id: SessionId,
    },
    /// The exact same confirmation was already processed within the retry window.
    Duplicate,
}

/// Errors returned by the rekey state machine.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RekeyError {
    #[error("another rekey transaction is already in progress")]
    Busy,
    #[error("rekey message does not match the pending transaction")]
    TransactionMismatch,
    #[error("rekey message is invalid in the current state")]
    UnexpectedMessage,
    #[error("rekey base epoch does not match the active epoch")]
    EpochMismatch,
    #[error("new rekey epoch must be greater than the active epoch")]
    EpochRollback,
    #[error("the 16-bit epoch space is exhausted; establish a new session")]
    EpochExhausted,
    #[error("new session identifier was already used by this session")]
    SessionIdReuse,
    #[error("rekey nonce must not be zero")]
    InvalidNonce,
    #[error("rekey transcript hash must not be zero")]
    InvalidTranscriptHash,
    #[error("rekey key-confirmation value must not be zero")]
    InvalidConfirmation,
}

enum Pending {
    AwaitAccept {
        init: RekeyInit,
        deadline: Instant,
    },
    AwaitConfirm {
        accept: RekeyAccept,
        expected_initiator_confirmation: RekeyDigest,
        deadline: Instant,
    },
    ConfirmSent {
        confirm: RekeyConfirm,
        deadline: Instant,
    },
}

struct RecentConfirmation {
    confirm: RekeyConfirm,
    expires_at: Instant,
}

/// Per-session online rekey coordinator.
///
/// The caller must authenticate each incoming message and verify all
/// cryptographic fields before passing it to this state machine. Invalid or
/// mismatched online messages reject the transaction without rolling back the
/// already established data epoch. Once the active data epoch reaches its hard
/// time/byte/counter limit, the caller must stop sending until a new epoch is
/// installed.
pub struct RekeyStateMachine {
    current_epoch: u16,
    current_session_id: SessionId,
    used_session_ids: Vec<SessionId>,
    pending: Option<Pending>,
    recent_confirmation: Option<RecentConfirmation>,
}

impl RekeyStateMachine {
    /// Create an idle state machine for an authenticated active epoch.
    pub fn new(current_epoch: u16, current_session_id: SessionId) -> Self {
        Self {
            current_epoch,
            current_session_id,
            used_session_ids: vec![current_session_id],
            pending: None,
            recent_confirmation: None,
        }
    }

    /// Return the currently active epoch.
    pub const fn current_epoch(&self) -> u16 {
        self.current_epoch
    }

    /// Return the currently active session identifier.
    pub const fn current_session_id(&self) -> SessionId {
        self.current_session_id
    }

    /// Return the observable handshake phase.
    pub const fn phase(&self) -> RekeyPhase {
        match self.pending {
            None => RekeyPhase::Idle,
            Some(Pending::AwaitAccept { .. }) => RekeyPhase::AwaitingAccept,
            Some(Pending::AwaitConfirm { .. }) => RekeyPhase::AwaitingConfirm,
            Some(Pending::ConfirmSent { .. }) => RekeyPhase::ConfirmSent,
        }
    }

    /// Whether a half-open rekey currently consumes the per-session budget.
    pub const fn is_half_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Expire a timed-out transaction or duplicate-confirmation retention entry.
    ///
    /// Returns `true` when a half-open transaction was expired. Expired session
    /// identifiers remain reserved and cannot be reused, even after timeout.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self
            .recent_confirmation
            .as_ref()
            .is_some_and(|recent| now >= recent.expires_at)
        {
            self.recent_confirmation = None;
        }

        let pending_expired = match &self.pending {
            Some(Pending::AwaitAccept { deadline, .. })
            | Some(Pending::AwaitConfirm { deadline, .. })
            | Some(Pending::ConfirmSent { deadline, .. }) => now >= *deadline,
            None => false,
        };
        if pending_expired {
            self.pending = None;
        }
        pending_expired
    }

    /// Start a locally initiated rekey transaction.
    pub fn initiate(
        &mut self,
        new_epoch: u16,
        new_session_id: SessionId,
        initiator_nonce: RekeyNonce,
        now: Instant,
    ) -> Result<RekeyInit, RekeyError> {
        self.expire(now);
        if self.pending.is_some() {
            return Err(RekeyError::Busy);
        }
        self.validate_new_epoch(new_epoch)?;
        self.ensure_session_unused(new_session_id)?;
        validate_nonce(initiator_nonce)?;

        let init = RekeyInit {
            base_epoch: self.current_epoch,
            new_epoch,
            new_session_id,
            initiator_nonce,
        };
        self.used_session_ids.push(new_session_id);
        self.pending = Some(Pending::AwaitAccept {
            init,
            deadline: now + REKEY_TIMEOUT,
        });
        Ok(init)
    }

    /// Accept a peer proposal after its cryptographic proof has been verified.
    ///
    /// Repeating the exact same `REKEY_INIT` during the retry window returns the
    /// original acceptance instead of allocating another half-open epoch.
    pub fn accept_init(
        &mut self,
        init: &RekeyInit,
        responder_nonce: RekeyNonce,
        transcript_hash: RekeyDigest,
        responder_confirmation: RekeyDigest,
        expected_initiator_confirmation: RekeyDigest,
        now: Instant,
    ) -> Result<RekeyAccept, RekeyError> {
        self.expire(now);

        if let Some(Pending::AwaitConfirm { accept, .. }) = &self.pending {
            return if accept.init == *init {
                Ok(*accept)
            } else {
                Err(RekeyError::Busy)
            };
        }
        if self.pending.is_some() {
            return Err(RekeyError::Busy);
        }
        if init.base_epoch != self.current_epoch {
            return Err(RekeyError::EpochMismatch);
        }
        self.validate_new_epoch(init.new_epoch)?;
        self.ensure_session_unused(init.new_session_id)?;
        validate_nonce(init.initiator_nonce)?;
        validate_nonce(responder_nonce)?;
        validate_transcript_hash(transcript_hash)?;
        validate_confirmation(responder_confirmation)?;
        validate_confirmation(expected_initiator_confirmation)?;

        let accept = RekeyAccept {
            init: *init,
            responder_nonce,
            transcript_hash,
            responder_confirmation,
        };
        self.used_session_ids.push(init.new_session_id);
        self.pending = Some(Pending::AwaitConfirm {
            accept,
            expected_initiator_confirmation,
            deadline: now + REKEY_TIMEOUT,
        });
        Ok(accept)
    }

    /// Process `REKEY_ACCEPT` and produce `REKEY_CONFIRM`.
    ///
    /// The initiator may start using the accepted epoch after this transition,
    /// but must retain the confirmation until the control channel confirms that
    /// the responder processed it. Repeating the exact acceptance returns the
    /// original confirmation.
    pub fn handle_accept(
        &mut self,
        accept: &RekeyAccept,
        initiator_confirmation: RekeyDigest,
        now: Instant,
    ) -> Result<RekeyConfirm, RekeyError> {
        self.expire(now);

        if let Some(Pending::ConfirmSent { confirm, .. }) = &self.pending {
            if confirm.accept == *accept && confirm.initiator_confirmation == initiator_confirmation
            {
                return Ok(*confirm);
            }
            return Err(RekeyError::TransactionMismatch);
        }

        let expected_init = match &self.pending {
            Some(Pending::AwaitAccept { init, .. }) => *init,
            _ => return Err(RekeyError::UnexpectedMessage),
        };
        if expected_init != accept.init {
            return Err(RekeyError::TransactionMismatch);
        }
        validate_nonce(accept.responder_nonce)?;
        validate_transcript_hash(accept.transcript_hash)?;
        validate_confirmation(accept.responder_confirmation)?;
        validate_confirmation(initiator_confirmation)?;

        let confirm = RekeyConfirm {
            accept: *accept,
            initiator_confirmation,
        };
        self.current_epoch = accept.init.new_epoch;
        self.current_session_id = accept.init.new_session_id;
        self.pending = Some(Pending::ConfirmSent {
            confirm,
            deadline: now + REKEY_TIMEOUT,
        });
        Ok(confirm)
    }

    /// Process `REKEY_CONFIRM` and activate the responder side of the epoch.
    pub fn handle_confirm(
        &mut self,
        confirm: &RekeyConfirm,
        now: Instant,
    ) -> Result<RekeyCompletion, RekeyError> {
        self.expire(now);

        if self
            .recent_confirmation
            .as_ref()
            .is_some_and(|recent| recent.confirm == *confirm)
        {
            return Ok(RekeyCompletion::Duplicate);
        }

        let (expected_accept, expected_confirmation) = match &self.pending {
            Some(Pending::AwaitConfirm {
                accept,
                expected_initiator_confirmation,
                ..
            }) => (*accept, *expected_initiator_confirmation),
            _ => return Err(RekeyError::UnexpectedMessage),
        };
        if expected_accept != confirm.accept
            || expected_confirmation != confirm.initiator_confirmation
        {
            return Err(RekeyError::TransactionMismatch);
        }

        let previous_epoch = self.current_epoch;
        self.current_epoch = confirm.accept.init.new_epoch;
        self.current_session_id = confirm.accept.init.new_session_id;
        self.pending = None;
        self.recent_confirmation = Some(RecentConfirmation {
            confirm: *confirm,
            expires_at: now + REKEY_TIMEOUT,
        });

        Ok(RekeyCompletion::Activated {
            previous_epoch,
            new_epoch: self.current_epoch,
            new_session_id: self.current_session_id,
        })
    }

    fn validate_new_epoch(&self, new_epoch: u16) -> Result<(), RekeyError> {
        if self.current_epoch == u16::MAX {
            return Err(RekeyError::EpochExhausted);
        }
        if new_epoch <= self.current_epoch {
            return Err(RekeyError::EpochRollback);
        }
        Ok(())
    }

    fn ensure_session_unused(&self, session_id: SessionId) -> Result<(), RekeyError> {
        if self.used_session_ids.contains(&session_id) {
            Err(RekeyError::SessionIdReuse)
        } else {
            Ok(())
        }
    }
}

fn validate_nonce(nonce: RekeyNonce) -> Result<(), RekeyError> {
    if nonce == [0; REKEY_NONCE_LEN] {
        Err(RekeyError::InvalidNonce)
    } else {
        Ok(())
    }
}

fn validate_transcript_hash(digest: RekeyDigest) -> Result<(), RekeyError> {
    if digest == [0; REKEY_DIGEST_LEN] {
        Err(RekeyError::InvalidTranscriptHash)
    } else {
        Ok(())
    }
}

fn validate_confirmation(confirmation: RekeyDigest) -> Result<(), RekeyError> {
    if confirmation == [0; REKEY_DIGEST_LEN] {
        Err(RekeyError::InvalidConfirmation)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(byte: u8) -> SessionId {
        [byte; SESSION_ID_LEN]
    }

    fn digest(byte: u8) -> RekeyDigest {
        [byte; REKEY_DIGEST_LEN]
    }

    fn machines() -> (RekeyStateMachine, RekeyStateMachine, Instant) {
        let now = Instant::now();
        (
            RekeyStateMachine::new(1, sid(0x11)),
            RekeyStateMachine::new(1, sid(0x11)),
            now,
        )
    }

    fn accepted_machines() -> (RekeyStateMachine, RekeyStateMachine, RekeyAccept, Instant) {
        let (mut initiator, mut responder, now) = machines();
        let init = initiator
            .initiate(2, sid(0x22), digest(0x31), now)
            .expect("initiate rekey");
        let accept = responder
            .accept_init(
                &init,
                digest(0x32),
                digest(0x33),
                digest(0x34),
                digest(0x35),
                now,
            )
            .expect("accept rekey");
        (initiator, responder, accept, now)
    }

    fn confirmed_machines() -> (
        RekeyStateMachine,
        RekeyStateMachine,
        RekeyConfirm,
        RekeyCompletion,
        Instant,
    ) {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("accept response");
        let completion = responder
            .handle_confirm(&confirm, now)
            .expect("confirm rekey");
        (initiator, responder, confirm, completion, now)
    }

    #[test]
    fn rekey_messages_advance_both_ends_to_the_bound_new_epoch() {
        let (initiator, responder, confirm, completion, _) = confirmed_machines();

        assert_eq!(initiator.current_epoch(), 2);
        assert_eq!(initiator.current_session_id(), sid(0x22));
        assert_eq!(responder.current_epoch(), 2);
        assert_eq!(responder.current_session_id(), sid(0x22));
        assert_eq!(initiator.phase(), RekeyPhase::ConfirmSent);
        assert_eq!(responder.phase(), RekeyPhase::Idle);
        assert_eq!(
            completion,
            RekeyCompletion::Activated {
                previous_epoch: 1,
                new_epoch: 2,
                new_session_id: sid(0x22),
            }
        );
        assert_eq!(confirm.accept.init.base_epoch, 1);
    }

    #[test]
    fn duplicate_messages_are_idempotent_within_retry_window() {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let original_accept = responder
            .accept_init(
                &accept.init,
                digest(0x91),
                digest(0x92),
                digest(0x93),
                digest(0x94),
                now,
            )
            .expect("duplicate init");
        assert_eq!(original_accept, accept);

        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("first accept");
        let duplicate_confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("duplicate accept");
        assert_eq!(duplicate_confirm, confirm);

        assert!(matches!(
            responder
                .handle_confirm(&confirm, now)
                .expect("first confirm"),
            RekeyCompletion::Activated { .. }
        ));
        assert_eq!(
            responder
                .handle_confirm(&confirm, now)
                .expect("duplicate confirm"),
            RekeyCompletion::Duplicate
        );
    }

    #[test]
    fn mismatched_accept_does_not_destroy_the_pending_transaction() {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let mut tampered = accept;
        tampered.init.new_epoch = 3;

        assert_eq!(
            initiator.handle_accept(&tampered, digest(0x35), now),
            Err(RekeyError::TransactionMismatch)
        );
        assert_eq!(initiator.phase(), RekeyPhase::AwaitingAccept);

        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("valid acceptance still succeeds");
        assert_eq!(
            responder
                .handle_confirm(&confirm, now)
                .expect("valid confirmation"),
            RekeyCompletion::Activated {
                previous_epoch: 1,
                new_epoch: 2,
                new_session_id: sid(0x22),
            }
        );
    }

    #[test]
    fn mismatched_confirm_does_not_activate_or_destroy_responder_state() {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("accept response");
        let mut tampered = confirm;
        tampered.initiator_confirmation = digest(0x99);

        assert_eq!(
            responder.handle_confirm(&tampered, now),
            Err(RekeyError::TransactionMismatch)
        );
        assert_eq!(responder.current_epoch(), 1);
        assert_eq!(responder.phase(), RekeyPhase::AwaitingConfirm);
        assert!(matches!(
            responder
                .handle_confirm(&confirm, now)
                .expect("original confirm still valid"),
            RekeyCompletion::Activated { .. }
        ));
    }

    #[test]
    fn timeout_clears_half_open_state_but_reserves_the_session_id() {
        let (mut initiator, _, _, now) = accepted_machines();
        let timeout = now + REKEY_TIMEOUT;

        assert!(initiator.expire(timeout));
        assert_eq!(initiator.phase(), RekeyPhase::Idle);
        assert!(!initiator.is_half_open());
        assert_eq!(
            initiator.initiate(3, sid(0x22), digest(0x41), timeout),
            Err(RekeyError::SessionIdReuse)
        );
        assert!(initiator
            .initiate(3, sid(0x23), digest(0x41), timeout)
            .is_ok());
    }

    #[test]
    fn responder_timeout_does_not_activate_the_epoch() {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("accept response");

        assert!(responder.expire(now + REKEY_TIMEOUT));
        assert_eq!(responder.current_epoch(), 1);
        assert_eq!(
            responder.handle_confirm(&confirm, now + REKEY_TIMEOUT),
            Err(RekeyError::UnexpectedMessage)
        );
        assert_eq!(
            responder.accept_init(
                &accept.init,
                digest(0x42),
                digest(0x43),
                digest(0x44),
                digest(0x45),
                now + REKEY_TIMEOUT
            ),
            Err(RekeyError::SessionIdReuse)
        );
    }

    #[test]
    fn second_transaction_is_rejected_until_the_first_completes_or_times_out() {
        let (mut initiator, _, _, now) = accepted_machines();
        assert_eq!(initiator.phase(), RekeyPhase::AwaitingAccept);
        assert_eq!(
            initiator.initiate(3, sid(0x23), digest(0x51), now),
            Err(RekeyError::Busy)
        );

        let mut responder = RekeyStateMachine::new(1, sid(0x11));
        let init = initiator
            .initiate(4, sid(0x24), digest(0x52), now + REKEY_TIMEOUT)
            .expect("first pending expired");
        responder
            .accept_init(
                &init,
                digest(0x53),
                digest(0x54),
                digest(0x55),
                digest(0x56),
                now,
            )
            .expect("remote transaction");
        let other = RekeyInit {
            base_epoch: 1,
            new_epoch: 2,
            new_session_id: sid(0x25),
            initiator_nonce: digest(0x56),
        };
        assert_eq!(
            responder.accept_init(
                &other,
                digest(0x57),
                digest(0x58),
                digest(0x59),
                digest(0x5a),
                now,
            ),
            Err(RekeyError::Busy)
        );
    }

    #[test]
    fn rollback_base_mismatch_and_epoch_exhaustion_are_rejected() {
        let now = Instant::now();
        let mut machine = RekeyStateMachine::new(5, sid(0x11));
        assert_eq!(
            machine.initiate(5, sid(0x12), digest(1), now),
            Err(RekeyError::EpochRollback)
        );
        assert_eq!(machine.phase(), RekeyPhase::Idle);

        let wrong_base = RekeyInit {
            base_epoch: 4,
            new_epoch: 6,
            new_session_id: sid(0x13),
            initiator_nonce: digest(2),
        };
        assert_eq!(
            machine.accept_init(&wrong_base, digest(3), digest(4), digest(5), digest(6), now),
            Err(RekeyError::EpochMismatch)
        );

        let mut exhausted = RekeyStateMachine::new(u16::MAX, sid(0x14));
        assert_eq!(
            exhausted.initiate(u16::MAX, sid(0x15), digest(6), now),
            Err(RekeyError::EpochExhausted)
        );
    }

    #[test]
    fn used_session_ids_cannot_be_recycled_after_completion() {
        let (mut initiator, _, _, _, now) = confirmed_machines();
        assert!(initiator.expire(now + REKEY_TIMEOUT));
        assert_eq!(
            initiator.initiate(3, sid(0x11), digest(0x61), now),
            Err(RekeyError::SessionIdReuse)
        );
        assert_eq!(
            initiator.initiate(3, sid(0x22), digest(0x61), now),
            Err(RekeyError::SessionIdReuse)
        );
    }

    #[test]
    fn zero_placeholder_security_fields_are_rejected_without_state_change() {
        let now = Instant::now();
        let mut initiator = RekeyStateMachine::new(1, sid(0x11));
        assert_eq!(
            initiator.initiate(2, sid(0x22), [0; 32], now),
            Err(RekeyError::InvalidNonce)
        );
        assert_eq!(initiator.phase(), RekeyPhase::Idle);

        let init = initiator
            .initiate(2, sid(0x22), digest(0x31), now)
            .expect("valid init");
        let mut responder = RekeyStateMachine::new(1, sid(0x11));
        assert_eq!(
            responder.accept_init(
                &init,
                [0; 32],
                digest(0x32),
                digest(0x33),
                digest(0x35),
                now,
            ),
            Err(RekeyError::InvalidNonce)
        );
        assert_eq!(
            responder.accept_init(
                &init,
                digest(0x32),
                [0; 32],
                digest(0x33),
                digest(0x35),
                now,
            ),
            Err(RekeyError::InvalidTranscriptHash)
        );
        assert_eq!(
            responder.accept_init(
                &init,
                digest(0x32),
                digest(0x33),
                [0; 32],
                digest(0x35),
                now,
            ),
            Err(RekeyError::InvalidConfirmation)
        );
        assert_eq!(
            responder.accept_init(
                &init,
                digest(0x32),
                digest(0x33),
                digest(0x34),
                [0; 32],
                now,
            ),
            Err(RekeyError::InvalidConfirmation)
        );
        assert_eq!(responder.phase(), RekeyPhase::Idle);

        let accept = responder
            .accept_init(
                &init,
                digest(0x32),
                digest(0x33),
                digest(0x34),
                digest(0x35),
                now,
            )
            .expect("valid accept");
        assert_eq!(
            initiator.handle_accept(&accept, [0; 32], now),
            Err(RekeyError::InvalidConfirmation)
        );
        assert_eq!(initiator.phase(), RekeyPhase::AwaitingAccept);
        assert_eq!(initiator.current_epoch(), 1);
    }

    #[test]
    fn duplicate_accept_and_confirm_are_rejected_after_timeout() {
        let (mut initiator, mut responder, accept, now) = accepted_machines();
        let confirm = initiator
            .handle_accept(&accept, digest(0x35), now)
            .expect("accept response");

        assert!(initiator.expire(now + REKEY_TIMEOUT));
        assert_eq!(
            initiator.handle_accept(&accept, digest(0x35), now + REKEY_TIMEOUT),
            Err(RekeyError::UnexpectedMessage)
        );

        responder
            .handle_confirm(&confirm, now)
            .expect("confirm response");
        assert!(!responder.expire(now + REKEY_TIMEOUT));
        assert_eq!(
            responder.handle_confirm(&confirm, now + REKEY_TIMEOUT),
            Err(RekeyError::UnexpectedMessage)
        );
    }
}
