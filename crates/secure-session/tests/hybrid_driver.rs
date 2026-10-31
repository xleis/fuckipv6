#![cfg(feature = "pq")]

use fuckipv6_secure_session::confirmation::{ConfirmationError, ConfirmationRole, ConfirmationTag};
use fuckipv6_secure_session::driver::{DriverError, SessionDriver, SessionInstallConfig};
use fuckipv6_secure_session::hybrid::{
    CompositePublicKey, HybridAccept, HybridConfirm, HybridConfirmError, HybridControlCodecError,
    HybridDriverError, HybridHandshakeError, HybridHello, HybridInitiatorHandshake,
    HybridProfilePolicy, HybridResponder, HybridResponderHandshake, HybridSuite,
    NegotiatedHybridProfile, ProfileNegotiationError,
};
use fuckipv6_secure_session::transcript::{
    ENDPOINT_CONTEXT_HASH_LEN, SUITE_AES_256_GCM, SUITE_CHACHA20_POLY1305,
};
use fuckipv6_secure_session::{DataCipherSuite, SessionError};
use std::time::Instant;

const UNIX_NOW: u64 = 1_700_000_000;
const EPOCH: u16 = 7;
const ENDPOINT: [u8; ENDPOINT_CONTEXT_HASH_LEN] = [0x5a; ENDPOINT_CONTEXT_HASH_LEN];
const COOKIE: [u8; 41] = [0u8; 41];
const INITIATOR_INDEX: u32 = 0xa1a2_a3a4;
const RESPONDER_INDEX: u32 = 0xb1b2_c3c4;
const CHACHA_PROFILE: NegotiatedHybridProfile =
    NegotiatedHybridProfile::required(HybridSuite::ChaCha20Poly1305);

struct Pending {
    initiator: HybridInitiatorHandshake,
    responder: HybridResponderHandshake,
    accept: HybridAccept,
}

fn begin_with(profile: NegotiatedHybridProfile) -> Pending {
    let (initiator_identity, initiator_public) =
        CompositePublicKey::from_seeds(&[0x11; 32], &[0x22; 32]).expect("initiator identity");
    let (responder_identity, responder_public) =
        CompositePublicKey::from_seeds(&[0x33; 32], &[0x44; 32]).expect("responder identity");

    let mut responder = HybridResponder::new(
        responder_identity,
        initiator_public,
        None,
        HybridProfilePolicy::require(profile.suite()),
    )
    .expect("create hybrid responder");

    let (initiator, hello) = HybridInitiatorHandshake::start(
        &initiator_identity,
        responder_public,
        profile,
        ENDPOINT,
        EPOCH,
        0,
        COOKIE,
        UNIX_NOW,
        SessionInstallConfig {
            local_receiver_index: INITIATOR_INDEX,
            peer_receiver_index: RESPONDER_INDEX,
        },
        None,
    )
    .expect("start hybrid initiator");

    let hello_frame = hello
        .encode_control_for_profile(profile)
        .expect("encode HELLO");
    let hello = HybridHello::decode_control_for_profile(&hello_frame, profile)
        .expect("decode exact-profile HELLO");

    let (responder, accept) = HybridResponderHandshake::accept(
        &mut responder,
        &hello,
        ENDPOINT,
        EPOCH,
        UNIX_NOW,
        SessionInstallConfig {
            local_receiver_index: RESPONDER_INDEX,
            peer_receiver_index: INITIATOR_INDEX,
        },
    )
    .expect("accept hybrid HELLO");

    let accept_frame = accept
        .encode_control_for_profile(profile)
        .expect("encode ACCEPT");
    let accept = HybridAccept::decode_control_for_profile(&accept_frame, profile)
        .expect("decode exact-profile ACCEPT");
    Pending {
        initiator,
        responder,
        accept,
    }
}

fn begin() -> Pending {
    begin_with(CHACHA_PROFILE)
}

fn activate_initiator(
    pending: Pending,
    now: Instant,
) -> (
    SessionDriver,
    HybridResponderHandshake,
    HybridConfirm,
    Vec<u8>,
) {
    let profile = pending.initiator.profile();
    let accept_frame = pending
        .accept
        .encode_control_for_profile(profile)
        .expect("re-encode ACCEPT");
    let (driver, confirm_frame) = pending
        .initiator
        .complete_wire(&accept_frame, UNIX_NOW, now)
        .expect("complete hybrid initiator");
    let confirm = HybridConfirm::decode_control_for_profile(&confirm_frame, profile)
        .expect("decode exact-profile CONFIRM");
    (driver, pending.responder, confirm, confirm_frame)
}

#[test]
fn wire_encoded_handshake_installs_bidirectional_hybrid_session() {
    let now = Instant::now();
    let (mut initiator, responder, confirm, confirm_frame) = activate_initiator(begin(), now);

    assert_eq!(initiator.role(), ConfirmationRole::Initiator);
    assert_eq!(initiator.session_id(), Some(confirm.session_id));
    assert_eq!(initiator.epoch(), Some(EPOCH));
    assert_eq!(initiator.suite(), Some(DataCipherSuite::ChaCha20Poly1305));

    let (mut responder, _) = responder
        .complete_wire(&confirm_frame, now)
        .expect("complete hybrid responder");
    assert_eq!(responder.role(), ConfirmationRole::Responder);
    assert_eq!(responder.session_id(), initiator.session_id());
    assert_eq!(responder.epoch(), Some(EPOCH));
    assert_eq!(responder.suite(), Some(DataCipherSuite::ChaCha20Poly1305));

    let mut packet = [0u8; 256];
    let packet_len = initiator
        .seal(b"hybrid initiator request", &mut packet, now)
        .expect("seal initiator packet");
    assert_eq!(
        responder
            .open_in_place(&mut packet[..packet_len], now)
            .expect("open initiator packet"),
        b"hybrid initiator request"
    );

    let packet_len = responder
        .seal(b"hybrid responder response", &mut packet, now)
        .expect("seal responder packet");
    assert_eq!(
        initiator
            .open_in_place(&mut packet[..packet_len], now)
            .expect("open responder packet"),
        b"hybrid responder response"
    );

    assert_eq!(initiator.stats().packets_sent, 1);
    assert_eq!(initiator.stats().packets_received, 1);
    assert_eq!(responder.stats().packets_sent, 1);
    assert_eq!(responder.stats().packets_received, 1);
}

#[test]
fn hybrid_driver_rejects_tampered_and_replayed_data_packets() {
    let now = Instant::now();
    let (mut initiator, responder, confirm, _) = activate_initiator(begin(), now);
    let (mut responder, _) = responder
        .complete(&confirm, now)
        .expect("complete hybrid responder");

    let mut packet = [0u8; 128];
    let packet_len = initiator
        .seal(b"authenticated hybrid packet", &mut packet, now)
        .expect("seal packet");
    let authenticated = packet;
    let mut tampered = authenticated;
    tampered[packet_len - 1] ^= 1;
    assert_eq!(
        responder.open_in_place(&mut tampered[..packet_len], now),
        Err(DriverError::Data(SessionError::AuthenticationFailed))
    );

    responder
        .open_in_place(&mut packet[..packet_len], now)
        .expect("valid packet opens after forged packet");
    let mut replay = authenticated;
    assert_eq!(
        responder.open_in_place(&mut replay[..packet_len], now),
        Err(DriverError::Data(SessionError::ReplayDetected))
    );
    assert_eq!(responder.stats().authentication_failures, 1);
    assert_eq!(responder.stats().replay_rejections, 1);
}

#[test]
fn confirm_validation_checks_every_binding_before_installing_keys() {
    let now = Instant::now();
    let (mut initiator, responder, valid, _) = activate_initiator(begin(), now);

    let mut wrong_version = valid;
    wrong_version.version = 3;
    assert!(matches!(
        responder.validate_confirm(&wrong_version),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::VersionMismatch { .. }
        ))
    ));

    let mut wrong_profile = valid;
    wrong_profile.profile = 3;
    assert!(matches!(
        responder.validate_confirm(&wrong_profile),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::ProfileMismatch { .. }
        ))
    ));

    let mut wrong_suite = valid;
    wrong_suite.suite_id = SUITE_AES_256_GCM;
    assert!(matches!(
        responder.validate_confirm(&wrong_suite),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::SuiteMismatch { .. }
        ))
    ));

    let mut wrong_session = valid;
    wrong_session.session_id[0] ^= 1;
    assert!(matches!(
        responder.validate_confirm(&wrong_session),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::SessionIdMismatch
        ))
    ));

    let mut wrong_transcript = valid;
    wrong_transcript.transcript_hash[0] ^= 1;
    assert!(matches!(
        responder.validate_confirm(&wrong_transcript),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::TranscriptHashMismatch
        ))
    ));

    let mut wrong_tag = valid;
    let mut tag = *wrong_tag.initiator_confirmation.as_bytes();
    tag[0] ^= 1;
    wrong_tag.initiator_confirmation = ConfirmationTag::from_slice(&tag).expect("tag");
    assert!(matches!(
        responder.validate_confirm(&wrong_tag),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::Confirmation(ConfirmationError::Mismatch)
        ))
    ));

    // Failed validations above did not consume pending state.
    let (mut responder, _) = responder
        .complete(&valid, now)
        .expect("valid confirmation still activates after rejected attempts");
    let mut packet = [0u8; 128];
    let packet_len = initiator
        .seal(b"still usable", &mut packet, now)
        .expect("seal");
    assert_eq!(
        responder
            .open_in_place(&mut packet[..packet_len], now)
            .expect("open"),
        b"still usable"
    );
}

#[test]
fn exact_profile_wire_decode_rejects_known_suite_downgrade() {
    let now = Instant::now();
    let pending = begin();
    let mut accept_frame = pending
        .accept
        .encode_control_for_profile(CHACHA_PROFILE)
        .expect("encode ACCEPT");
    accept_frame[14..16].copy_from_slice(&SUITE_AES_256_GCM.to_be_bytes());
    assert!(matches!(
        pending
            .initiator
            .complete_wire(&accept_frame, UNIX_NOW, now),
        Err(HybridDriverError::Control(
            HybridControlCodecError::Profile(ProfileNegotiationError::SuiteMismatch {
                required: SUITE_CHACHA20_POLY1305,
                found: SUITE_AES_256_GCM,
            })
        ))
    ));

    let (_, responder, _, confirm_frame) = activate_initiator(begin(), now);
    let mut downgraded_confirm = confirm_frame;
    downgraded_confirm[14..16].copy_from_slice(&SUITE_AES_256_GCM.to_be_bytes());
    assert!(matches!(
        responder.complete_wire(&downgraded_confirm, now),
        Err(HybridDriverError::Control(
            HybridControlCodecError::Profile(ProfileNegotiationError::SuiteMismatch {
                required: SUITE_CHACHA20_POLY1305,
                found: SUITE_AES_256_GCM,
            })
        ))
    ));
}

#[test]
fn stale_confirm_cannot_activate_a_new_handshake() {
    let now = Instant::now();
    let (_, _, old_confirm, _) = activate_initiator(begin(), now);
    let (mut initiator, responder, new_confirm, _) = activate_initiator(begin(), now);

    assert!(matches!(
        responder.validate_confirm(&old_confirm),
        Err(HybridDriverError::Confirm(
            HybridConfirmError::SessionIdMismatch
        ))
    ));
    let (mut responder, _) = responder
        .complete(&new_confirm, now)
        .expect("new confirmation activates its own pending handshake");

    let mut packet = [0u8; 64];
    let packet_len = initiator
        .seal(b"new session", &mut packet, now)
        .expect("seal");
    assert_eq!(
        responder
            .open_in_place(&mut packet[..packet_len], now)
            .expect("open"),
        b"new session"
    );
}

#[test]
fn selected_hybrid_suite_is_installed_without_fallback() {
    let now = Instant::now();
    for (profile, expected_suite) in [
        (
            NegotiatedHybridProfile::required(HybridSuite::Aes256Gcm),
            DataCipherSuite::Aes256Gcm,
        ),
        (CHACHA_PROFILE, DataCipherSuite::ChaCha20Poly1305),
    ] {
        let (initiator, responder, confirm, _) = activate_initiator(begin_with(profile), now);
        let (responder, _) = responder
            .complete(&confirm, now)
            .expect("activate selected suite");
        assert_eq!(initiator.suite(), Some(expected_suite));
        assert_eq!(responder.suite(), Some(expected_suite));
        assert_eq!(initiator.session_id(), responder.session_id());
    }
}

#[test]
fn psk_mode_must_match_local_configuration_before_hello_generation() {
    let (initiator_identity, _) =
        CompositePublicKey::from_seeds(&[0x11; 32], &[0x22; 32]).expect("initiator identity");
    let (_, responder_public) =
        CompositePublicKey::from_seeds(&[0x33; 32], &[0x44; 32]).expect("responder identity");

    let result = HybridInitiatorHandshake::start(
        &initiator_identity,
        responder_public,
        CHACHA_PROFILE,
        ENDPOINT,
        EPOCH,
        0,
        COOKIE,
        UNIX_NOW,
        SessionInstallConfig {
            local_receiver_index: INITIATOR_INDEX,
            peer_receiver_index: RESPONDER_INDEX,
        },
        Some(&[0x55; 32]),
    );
    assert!(matches!(
        result,
        Err(HybridDriverError::Handshake(
            HybridHandshakeError::PskModeMismatch
        ))
    ));
}
