use fuckipv6_secure_session::confirmation::{
    compute_confirmation, ConfirmationError, ConfirmationRole, ConfirmationTag,
};
use fuckipv6_secure_session::driver::{DriverError, SessionDriver, SessionInstallConfig};
use fuckipv6_secure_session::handshake::{ClassicInitiator, ClassicResponder, Ed25519Identity};
use fuckipv6_secure_session::handshake_epoch::HandshakeEpochError;
use fuckipv6_secure_session::key_schedule::{derive_session_keys, DerivedSession};
use fuckipv6_secure_session::rekey::{RekeyCompletion, RekeyPhase};
use fuckipv6_secure_session::rekey_session::{RekeyAcceptOffer, RekeySessionError};
use fuckipv6_secure_session::transcript::{ENDPOINT_CONTEXT_HASH_LEN, SUITE_CHACHA20_POLY1305};
use fuckipv6_secure_session::{
    DataCipherSuite, Direction, ReceiveCipher, SendCipher, SessionError, AEAD_TAG_LEN,
    DATA_HEADER_LEN, MAX_EPOCH_LIFETIME, RETIRED_EPOCH_GRACE, SESSION_ID_LEN,
};
use std::time::{Duration, Instant};

const UNIX_NOW: u64 = 1_700_000_000;
const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];
const INITIAL_EPOCH: u16 = 1;
const NEW_EPOCH: u16 = 2;
const INITIATOR_INDEX: u32 = 0xa1a2_a3a4;
const RESPONDER_INDEX: u32 = 0xb1b2_c3c4;
const NEW_SID: [u8; SESSION_ID_LEN] = [0x22; SESSION_ID_LEN];

fn derived_pair() -> (DerivedSession, DerivedSession) {
    let initiator_identity = Ed25519Identity::from_seed([0x11; 32]);
    let responder_identity = Ed25519Identity::from_seed([0x22; 32]);

    let initiator = ClassicInitiator::start(
        &initiator_identity,
        responder_identity.verifying_key(),
        SUITE_CHACHA20_POLY1305,
        ENDPOINT,
        INITIAL_EPOCH,
        UNIX_NOW,
    )
    .expect("start initiator");
    let hello = initiator.hello().clone();
    let mut responder =
        ClassicResponder::new(responder_identity, initiator_identity.verifying_key())
            .expect("create responder");
    let accepted = responder
        .accept(&hello, ENDPOINT, INITIAL_EPOCH, UNIX_NOW)
        .expect("accept hello");

    let initiator_derived = derive_session_keys(
        initiator
            .finish(&accepted.accept, UNIX_NOW)
            .expect("finish initiator"),
        None,
    )
    .expect("derive initiator keys");
    let responder_derived =
        derive_session_keys(accepted.established, None).expect("derive responder keys");
    (initiator_derived, responder_derived)
}

fn confirmation_for(derived: &DerivedSession, role: ConfirmationRole) -> ConfirmationTag {
    compute_confirmation(&derived.keys, role, &derived.identity.transcript_hash)
}

fn install_pair(now: Instant) -> (SessionDriver, SessionDriver, [u8; SESSION_ID_LEN]) {
    let (initiator_derived, responder_derived) = derived_pair();
    let sid = initiator_derived.identity.session_id;
    assert_eq!(responder_derived.identity.session_id, sid);
    assert_ne!(sid, [0; SESSION_ID_LEN]);

    let initiator_confirmation = confirmation_for(&initiator_derived, ConfirmationRole::Initiator);
    let responder_confirmation = confirmation_for(&responder_derived, ConfirmationRole::Responder);

    let initiator = SessionDriver::from_handshake(
        initiator_derived,
        ConfirmationRole::Initiator,
        SessionInstallConfig {
            local_receiver_index: INITIATOR_INDEX,
            peer_receiver_index: RESPONDER_INDEX,
        },
        &responder_confirmation,
        now,
    )
    .expect("install initiator driver");
    let responder = SessionDriver::from_handshake(
        responder_derived,
        ConfirmationRole::Responder,
        SessionInstallConfig {
            local_receiver_index: RESPONDER_INDEX,
            peer_receiver_index: INITIATOR_INDEX,
        },
        &initiator_confirmation,
        now,
    )
    .expect("install responder driver");

    assert_eq!(initiator.session_id(), Some(sid));
    assert_eq!(responder.session_id(), Some(sid));
    assert_eq!(initiator.epoch(), Some(INITIAL_EPOCH));
    assert_eq!(responder.epoch(), Some(INITIAL_EPOCH));
    assert_eq!(initiator.suite(), Some(DataCipherSuite::ChaCha20Poly1305));
    assert_eq!(responder.suite(), Some(DataCipherSuite::ChaCha20Poly1305));
    (initiator, responder, sid)
}

fn initiator_ciphers() -> (SendCipher, ReceiveCipher) {
    (
        SendCipher::new(
            [0x41; 32],
            NEW_SID,
            Direction::InitiatorToResponder,
            NEW_EPOCH,
            RESPONDER_INDEX,
        ),
        ReceiveCipher::new(
            [0x42; 32],
            NEW_SID,
            Direction::ResponderToInitiator,
            NEW_EPOCH,
            INITIATOR_INDEX,
        ),
    )
}

fn responder_ciphers() -> (SendCipher, ReceiveCipher) {
    (
        SendCipher::new(
            [0x42; 32],
            NEW_SID,
            Direction::ResponderToInitiator,
            NEW_EPOCH,
            INITIATOR_INDEX,
        ),
        ReceiveCipher::new(
            [0x41; 32],
            NEW_SID,
            Direction::InitiatorToResponder,
            NEW_EPOCH,
            RESPONDER_INDEX,
        ),
    )
}

#[test]
fn confirmed_handshake_drives_bidirectional_data() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);

    let mut request = [0u8; 256];
    let request_len = initiator
        .seal(b"authenticated request", &mut request, now)
        .expect("seal request");
    assert_eq!(
        responder
            .open_in_place(&mut request[..request_len], now)
            .expect("open request"),
        b"authenticated request"
    );

    let mut response = [0u8; 256];
    let response_len = responder
        .seal(b"authenticated response", &mut response, now)
        .expect("seal response");
    assert_eq!(
        initiator
            .open_in_place(&mut response[..response_len], now)
            .expect("open response"),
        b"authenticated response"
    );

    let initiator_stats = initiator.stats();
    assert_eq!(initiator_stats.packets_sent, 1);
    assert_eq!(initiator_stats.packets_received, 1);
    assert_eq!(
        initiator_stats.plaintext_bytes_sent,
        b"authenticated request".len() as u64
    );
    assert_eq!(
        initiator_stats.plaintext_bytes_received,
        b"authenticated response".len() as u64
    );
}

#[test]
fn driver_seals_a_reserved_packet_buffer_in_place() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);
    let plaintext = b"reserved packet payload";
    let packet_len = DATA_HEADER_LEN + plaintext.len() + AEAD_TAG_LEN;
    let mut packet = [0u8; 256];
    packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext.len()].copy_from_slice(plaintext);

    let encoded_len = initiator
        .seal_in_place(&mut packet[..packet_len], plaintext.len(), now)
        .expect("seal reserved packet in place");
    assert_eq!(encoded_len, packet_len);
    assert_eq!(
        responder
            .open_in_place(&mut packet[..encoded_len], now)
            .expect("open in-place packet"),
        plaintext
    );
    assert_eq!(initiator.stats().packets_sent, 1);
    assert_eq!(
        initiator.stats().plaintext_bytes_sent,
        plaintext.len() as u64
    );
}

#[test]
fn driver_rejects_short_reserved_buffer_without_charging_stats_or_counter() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);
    let mut short = [0u8; DATA_HEADER_LEN + AEAD_TAG_LEN];

    assert_eq!(
        initiator.seal_in_place(&mut short, 1, now),
        Err(DriverError::Data(SessionError::BufferTooSmall))
    );
    assert_eq!(initiator.stats().packets_sent, 0);
    assert_eq!(initiator.stats().plaintext_bytes_sent, 0);

    let mut packet = [0u8; DATA_HEADER_LEN + 1 + AEAD_TAG_LEN];
    packet[DATA_HEADER_LEN] = b'x';
    let encoded_len = initiator
        .seal_in_place(&mut packet, 1, now)
        .expect("the failed attempt must not consume the first counter");
    assert_eq!(
        responder
            .open_in_place(&mut packet[..encoded_len], now)
            .expect("open packet after rejected short buffer"),
        b"x"
    );
}

#[test]
fn rejected_tamper_and_replay_do_not_poison_the_session() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);

    let mut packet = [0u8; 256];
    let packet_len = initiator
        .seal(b"packet", &mut packet, now)
        .expect("seal packet");
    let authenticated_packet = packet;
    let mut tampered = authenticated_packet;
    tampered[packet_len - 1] ^= 1;
    assert_eq!(
        responder.open_in_place(&mut tampered[..packet_len], now),
        Err(DriverError::Data(SessionError::AuthenticationFailed))
    );

    responder
        .open_in_place(&mut packet[..packet_len], now)
        .expect("valid packet still opens after forged packet");
    let mut replay = authenticated_packet;
    assert_eq!(
        responder.open_in_place(&mut replay[..packet_len], now),
        Err(DriverError::Data(SessionError::ReplayDetected))
    );
    assert!(!responder.is_closed());
    let stats = responder.stats();
    assert_eq!(stats.authentication_failures, 1);
    assert_eq!(stats.replay_rejections, 1);
    assert_eq!(stats.packets_received, 1);
    assert_eq!(stats.plaintext_bytes_received, b"packet".len() as u64);
}

#[test]
fn wrong_peer_confirmation_is_rejected_before_data_keys_are_installed() {
    let now = Instant::now();
    let (initiator_derived, _) = derived_pair();
    let wrong = ConfirmationTag::from_slice(&[0x99; 32]).expect("32-byte confirmation");

    let error = match SessionDriver::from_handshake(
        initiator_derived,
        ConfirmationRole::Initiator,
        SessionInstallConfig {
            local_receiver_index: INITIATOR_INDEX,
            peer_receiver_index: RESPONDER_INDEX,
        },
        &wrong,
        now,
    ) {
        Ok(_) => panic!("foreign confirmation unexpectedly installed a driver"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        HandshakeEpochError::Confirmation(ConfirmationError::Mismatch)
    );
}

#[test]
fn rekey_installs_new_epoch_and_retains_old_receive_keys_only_for_grace() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);

    let mut old_in_grace = [0u8; 128];
    let old_in_grace_len = initiator
        .seal(b"old in grace", &mut old_in_grace, now)
        .expect("seal old packet");
    let mut old_after_grace = [0u8; 128];
    let old_after_grace_len = initiator
        .seal(b"old after grace", &mut old_after_grace, now)
        .expect("seal second old packet");

    let init = initiator
        .initiate_rekey(NEW_EPOCH, NEW_SID, [0x31; 32], now)
        .expect("initiate rekey");
    let (responder_send, responder_receive) = responder_ciphers();
    let accept = responder
        .accept_rekey(
            RekeyAcceptOffer {
                init,
                responder_nonce: [0x32; 32],
                transcript_hash: [0x33; 32],
                responder_confirmation: [0x34; 32],
                expected_initiator_confirmation: [0x35; 32],
                new_send: responder_send,
                new_receive: responder_receive,
            },
            now,
        )
        .expect("stage responder rekey");
    let (initiator_send, initiator_receive) = initiator_ciphers();
    let confirm = initiator
        .complete_rekey_initiator(
            &accept,
            accept.expected_initiator_confirmation,
            initiator_send,
            initiator_receive,
            now,
        )
        .expect("activate initiator rekey");
    assert!(matches!(
        responder
            .complete_rekey_responder(&confirm, now)
            .expect("activate responder rekey"),
        RekeyCompletion::Activated {
            previous_epoch: INITIAL_EPOCH,
            new_epoch: NEW_EPOCH,
            ..
        }
    ));
    assert_eq!(initiator.session_id(), Some(NEW_SID));
    assert_eq!(responder.session_id(), Some(NEW_SID));
    assert_eq!(initiator.epoch(), Some(NEW_EPOCH));
    assert_eq!(responder.epoch(), Some(NEW_EPOCH));

    assert_eq!(
        responder
            .open_in_place(&mut old_in_grace[..old_in_grace_len], now)
            .expect("old epoch packet inside grace"),
        b"old in grace"
    );
    assert_eq!(
        responder.open_in_place(
            &mut old_after_grace[..old_after_grace_len],
            now + RETIRED_EPOCH_GRACE,
        ),
        Err(DriverError::Data(SessionError::UnknownEpoch))
    );

    let mut current = [0u8; 128];
    let current_len = initiator
        .seal(b"new epoch", &mut current, now)
        .expect("seal new epoch packet");
    assert_eq!(
        responder
            .open_in_place(&mut current[..current_len], now)
            .expect("open new epoch packet"),
        b"new epoch"
    );
}

#[test]
fn invalid_rekey_material_fails_closed_and_drops_access_to_data() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);
    let init = initiator
        .initiate_rekey(NEW_EPOCH, NEW_SID, [0x31; 32], now)
        .expect("initiate rekey");
    let (send, _) = responder_ciphers();
    let wrong_sid = [0x77; SESSION_ID_LEN];
    let bad_receive = ReceiveCipher::new(
        [0x41; 32],
        wrong_sid,
        Direction::InitiatorToResponder,
        NEW_EPOCH,
        RESPONDER_INDEX,
    );

    assert_eq!(
        responder.accept_rekey(
            RekeyAcceptOffer {
                init,
                responder_nonce: [0x32; 32],
                transcript_hash: [0x33; 32],
                responder_confirmation: [0x34; 32],
                expected_initiator_confirmation: [0x35; 32],
                new_send: send,
                new_receive: bad_receive,
            },
            now,
        ),
        Err(DriverError::Rekey(RekeySessionError::MaterialMismatch))
    );
    assert!(responder.is_closed());
    assert_eq!(responder.rekey_phase(), None);
    assert!(responder.rekey_required(now));

    let mut packet = [0u8; 128];
    assert_eq!(
        responder.seal(b"closed", &mut packet, now),
        Err(DriverError::Closed)
    );
    assert_eq!(
        responder.open_in_place(&mut packet, now),
        Err(DriverError::Closed)
    );
}

#[test]
fn rekey_required_blocks_send_after_epoch_lifetime_without_closing_session() {
    let now = Instant::now();
    let (mut initiator, _, _) = install_pair(now);
    let mut packet = [0u8; 128];
    assert_eq!(
        initiator.seal(b"late", &mut packet, now + MAX_EPOCH_LIFETIME),
        Err(DriverError::Data(SessionError::RekeyRequired))
    );
    assert!(!initiator.is_closed());
    assert!(initiator.rekey_required(now + MAX_EPOCH_LIFETIME));
}

#[test]
fn responder_can_expire_an_abandoned_rekey_without_advancing_epoch() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);
    let init = initiator
        .initiate_rekey(NEW_EPOCH, NEW_SID, [0x31; 32], now)
        .expect("initiate rekey");
    let (send, receive) = responder_ciphers();
    responder
        .accept_rekey(
            RekeyAcceptOffer {
                init,
                responder_nonce: [0x32; 32],
                transcript_hash: [0x33; 32],
                responder_confirmation: [0x34; 32],
                expected_initiator_confirmation: [0x35; 32],
                new_send: send,
                new_receive: receive,
            },
            now,
        )
        .expect("stage responder rekey");
    assert!(responder
        .expire_rekey(now + Duration::from_secs(6))
        .expect("expire rekey"));
    assert_eq!(responder.epoch(), Some(INITIAL_EPOCH));
    assert_eq!(responder.rekey_phase(), Some(RekeyPhase::Idle));
}

#[test]
fn idempotent_rekey_retries_do_not_inflate_completion_counters() {
    let now = Instant::now();
    let (mut initiator, mut responder, _) = install_pair(now);
    let init = initiator
        .initiate_rekey(NEW_EPOCH, NEW_SID, [0x31; 32], now)
        .expect("initiate rekey");

    let offer = || {
        let (send, receive) = responder_ciphers();
        RekeyAcceptOffer {
            init,
            responder_nonce: [0x32; 32],
            transcript_hash: [0x33; 32],
            responder_confirmation: [0x34; 32],
            expected_initiator_confirmation: [0x35; 32],
            new_send: send,
            new_receive: receive,
        }
    };
    let accept = responder
        .accept_rekey(offer(), now)
        .expect("stage responder rekey");
    responder
        .accept_rekey(offer(), now)
        .expect("idempotent responder retry");
    assert_eq!(responder.stats().rekeys_accepted, 1);

    let initiator_offer = || {
        let (send, receive) = initiator_ciphers();
        (accept.expected_initiator_confirmation, send, receive)
    };
    let (confirmation, send, receive) = initiator_offer();
    let confirm = initiator
        .complete_rekey_initiator(&accept, confirmation, send, receive, now)
        .expect("activate initiator rekey");
    let (confirmation, send, receive) = initiator_offer();
    initiator
        .complete_rekey_initiator(&accept, confirmation, send, receive, now)
        .expect("idempotent initiator retry");
    assert_eq!(initiator.stats().rekeys_completed, 1);

    assert!(matches!(
        responder
            .complete_rekey_responder(&confirm, now)
            .expect("activate responder rekey"),
        RekeyCompletion::Activated { .. }
    ));
    assert_eq!(
        responder
            .complete_rekey_responder(&confirm, now)
            .expect("idempotent responder confirmation"),
        RekeyCompletion::Duplicate
    );
    assert_eq!(responder.stats().rekeys_completed, 1);
}
