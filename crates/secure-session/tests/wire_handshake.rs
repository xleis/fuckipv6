use ed25519_dalek::VerifyingKey;
use fuckipv6_secure_session::confirmation::ConfirmationError;
use fuckipv6_secure_session::driver::{
    DriverError, InitiatorHandshake, ResponderHandshake, SessionDriver, SessionInstallConfig,
};
use fuckipv6_secure_session::handshake::{ClassicResponder, Ed25519Identity};
use fuckipv6_secure_session::handshake_epoch::HandshakeEpochError;
use fuckipv6_secure_session::wire::{
    classic_endpoint_context, decode_control, AcceptFrame, ClassicEndpointContextInput,
    ConfirmFrame, ControlMessage,
};
use fuckipv6_secure_session::{DataCipherSuite, SESSION_ID_LEN};
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

const UNIX_NOW: u64 = 1_700_000_000;
const EPOCH: u16 = 7;
const INITIATOR_INDEX: u32 = 0xa1a2_a3a4;
const RESPONDER_INDEX: u32 = 0xb1b2_c3c4;

struct PendingWireHandshake {
    initiator: InitiatorHandshake,
    responder: ResponderHandshake,
    accept: AcceptFrame,
}

fn endpoint_context() -> [u8; 32] {
    let initiator_ips: &[IpAddr] = &["10.8.0.2".parse().unwrap(), "fd00::2".parse().unwrap()];
    let responder_ips: &[IpAddr] = &["10.8.0.1".parse().unwrap(), "fd00::1".parse().unwrap()];
    let initiator_endpoints: &[SocketAddr] = &["198.51.100.2:6667".parse().unwrap()];
    let responder_endpoints: &[SocketAddr] = &["198.51.100.1:6667".parse().unwrap()];
    let initiator_key = Ed25519Identity::from_seed([0x11; 32])
        .verifying_key()
        .to_bytes();
    let responder_key = Ed25519Identity::from_seed([0x22; 32])
        .verifying_key()
        .to_bytes();

    classic_endpoint_context(&ClassicEndpointContextInput {
        network_id: b"wire-integration-network",
        initiator_public_key: &initiator_key,
        responder_public_key: &responder_key,
        initiator_virtual_ips: initiator_ips,
        responder_virtual_ips: responder_ips,
        initiator_endpoints,
        responder_endpoints,
    })
    .expect("endpoint context")
}

fn begin() -> PendingWireHandshake {
    let initiator_identity = Ed25519Identity::from_seed([0x11; 32]);
    let responder_identity = Ed25519Identity::from_seed([0x22; 32]);
    let expected_responder: VerifyingKey = responder_identity.verifying_key();
    let context = endpoint_context();

    let (initiator, hello) = InitiatorHandshake::start(
        &initiator_identity,
        expected_responder,
        DataCipherSuite::ChaCha20Poly1305.id(),
        context,
        EPOCH,
        UNIX_NOW,
        SessionInstallConfig {
            local_receiver_index: INITIATOR_INDEX,
            peer_receiver_index: RESPONDER_INDEX,
        },
        None,
    )
    .expect("start initiator");

    let hello = match decode_control(&hello.encode_control()).expect("decode HELLO") {
        ControlMessage::Hello(hello) => hello,
        _ => panic!("expected HELLO"),
    };
    let mut responder =
        ClassicResponder::new(responder_identity, initiator_identity.verifying_key())
            .expect("create responder");
    let (responder, accept) = ResponderHandshake::accept(
        &mut responder,
        &hello,
        context,
        EPOCH,
        UNIX_NOW,
        SessionInstallConfig {
            local_receiver_index: RESPONDER_INDEX,
            peer_receiver_index: INITIATOR_INDEX,
        },
        None,
    )
    .expect("accept HELLO");
    let accept_frame = AcceptFrame {
        accept,
        responder_confirmation: responder.local_confirmation(),
    };
    let accept = match decode_control(&accept_frame.encode()).expect("decode ACCEPT") {
        ControlMessage::Accept(accept) => accept,
        _ => panic!("expected ACCEPT"),
    };

    PendingWireHandshake {
        initiator,
        responder,
        accept,
    }
}

fn initiator_confirm(
    pending: PendingWireHandshake,
) -> (SessionDriver, ResponderHandshake, ConfirmFrame) {
    let (driver, confirm) = pending
        .initiator
        .complete_wire(&pending.accept, UNIX_NOW, Instant::now())
        .expect("complete initiator");
    let confirm = match decode_control(&confirm.encode()).expect("decode CONFIRM") {
        ControlMessage::Confirm(confirm) => confirm,
        _ => panic!("expected CONFIRM"),
    };
    (driver, pending.responder, confirm)
}

#[test]
fn wire_encoded_handshake_installs_bidirectional_session() {
    let (mut initiator, responder_handshake, confirm) = initiator_confirm(begin());
    let (mut responder, _) = responder_handshake
        .complete_wire(&confirm, Instant::now())
        .expect("complete responder");

    assert_eq!(initiator.session_id(), responder.session_id());
    assert_eq!(initiator.session_id(), Some(confirm.session_id));
    assert_eq!(initiator.epoch(), Some(EPOCH));
    assert_eq!(responder.epoch(), Some(EPOCH));
    assert_eq!(initiator.suite(), Some(DataCipherSuite::ChaCha20Poly1305));

    let mut packet = [0u8; 128];
    let len = initiator
        .seal(
            b"wire handshake initiator data",
            &mut packet,
            Instant::now(),
        )
        .expect("seal initiator packet");
    assert_eq!(
        responder
            .open_in_place(&mut packet[..len], Instant::now())
            .expect("open initiator packet"),
        b"wire handshake initiator data"
    );

    let len = responder
        .seal(
            b"wire handshake responder data",
            &mut packet,
            Instant::now(),
        )
        .expect("seal responder packet");
    assert_eq!(
        initiator
            .open_in_place(&mut packet[..len], Instant::now())
            .expect("open responder packet"),
        b"wire handshake responder data"
    );
    assert_eq!(initiator.stats().packets_sent, 1);
    assert_eq!(initiator.stats().packets_received, 1);
    assert_eq!(responder.stats().packets_sent, 1);
    assert_eq!(responder.stats().packets_received, 1);
}

#[test]
fn responder_rejects_confirm_session_and_transcript_mismatches() {
    let (_, responder, valid) = initiator_confirm(begin());
    let mut wrong_session = valid;
    wrong_session.session_id = [0x55; SESSION_ID_LEN];
    assert_eq!(
        responder
            .complete_wire(&wrong_session, Instant::now())
            .err(),
        Some(DriverError::ConfirmSessionMismatch)
    );

    let (_, responder, valid) = initiator_confirm(begin());
    let mut wrong_transcript = valid;
    wrong_transcript.transcript_hash[0] ^= 1;
    assert_eq!(
        responder
            .complete_wire(&wrong_transcript, Instant::now())
            .err(),
        Some(DriverError::ConfirmTranscriptMismatch)
    );

    let (_, responder, valid) = initiator_confirm(begin());
    let mut wrong_tag = valid;
    let mut tag = *wrong_tag.initiator_confirmation.as_bytes();
    tag[0] ^= 1;
    wrong_tag.initiator_confirmation =
        fuckipv6_secure_session::confirmation::ConfirmationTag::from_slice(&tag)
            .expect("confirmation tag");
    assert!(matches!(
        responder.complete_wire(&wrong_tag, Instant::now()).err(),
        Some(DriverError::EpochInstall(
            HandshakeEpochError::Confirmation(ConfirmationError::Mismatch)
        ))
    ));
}
