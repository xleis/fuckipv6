#![no_main]

use fuckipv6_secure_session::handshake::{
    identity_id, Accept, ClassicInitiator, ClassicResponder, Ed25519Identity, Hello,
    CLASSIC_PROFILE_ID, ED25519_SIGNATURE_LEN,
};
use fuckipv6_secure_session::transcript::{HandshakeTranscript, SUITE_CHACHA20_POLY1305};
use fuckipv6_secure_session::PROTOCOL_VERSION;
use libfuzzer_sys::fuzz_target;

const NOW: u64 = 1_700_000_000;
const CONTEXT: [u8; 32] = [0x5a; 32];

fn byte<const N: usize>(data: &[u8], offset: usize) -> [u8; N] {
    let mut output = [0u8; N];
    for (index, value) in output.iter_mut().enumerate() {
        *value = data[(offset + index) % data.len()];
    }
    output
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let initiator_seed = byte(data, 0);
    let mut responder_seed = byte(data, 32);
    if initiator_seed == responder_seed {
        responder_seed[0] ^= 1;
    }
    let initiator = Ed25519Identity::from_seed(initiator_seed);
    let responder = Ed25519Identity::from_seed(responder_seed);
    let initiator_verifying = initiator.verifying_key();
    let responder_verifying = responder.verifying_key();

    let mut hello = Hello {
        version: PROTOCOL_VERSION,
        profile: CLASSIC_PROFILE_ID,
        suite_id: SUITE_CHACHA20_POLY1305,
        initiator_identity: identity_id(&initiator_verifying),
        responder_identity: identity_id(&responder_verifying),
        initiator_ephemeral_public: byte(data, 64),
        initiator_nonce: byte(data, 96),
        issued_at: NOW,
        endpoint_context_hash: CONTEXT,
        signature: byte(data, 128),
    };
    if hello
        .initiator_ephemeral_public
        .iter()
        .all(|byte| *byte == 0)
    {
        hello.initiator_ephemeral_public[0] = 1;
    }
    if hello.initiator_nonce.iter().all(|byte| *byte == 0) {
        hello.initiator_nonce[0] = 1;
    }
    let hello_hash = hello.signing_hash();

    if let Ok(mut responder_state) = ClassicResponder::new(responder.clone(), initiator_verifying) {
        let _ = responder_state.accept(&hello, CONTEXT, 7, NOW);
    }

    let mut accept = Accept {
        version: PROTOCOL_VERSION,
        profile: CLASSIC_PROFILE_ID,
        suite_id: hello.suite_id,
        session_id: byte(data, 128),
        epoch: 7,
        initiator_identity: hello.initiator_identity,
        responder_identity: hello.responder_identity,
        initiator_ephemeral_public: hello.initiator_ephemeral_public,
        responder_ephemeral_public: byte(data, 144),
        initiator_nonce: hello.initiator_nonce,
        responder_nonce: byte(data, 176),
        hello_issued_at: NOW,
        issued_at: NOW,
        endpoint_context_hash: CONTEXT,
        signature: byte(data, 208),
    };
    if accept
        .responder_ephemeral_public
        .iter()
        .all(|byte| *byte == 0)
    {
        accept.responder_ephemeral_public[0] = 1;
    }
    if accept.responder_nonce.iter().all(|byte| *byte == 0) {
        accept.responder_nonce[0] = 1;
    }
    if accept.session_id.iter().all(|byte| *byte == 0) {
        accept.session_id[0] = 1;
    }
    let accept_hash = accept.signing_hash(&hello_hash, &hello.signature);
    let _ = accept.signing_hash(&accept_hash, &[0u8; ED25519_SIGNATURE_LEN]);

    let transcript = HandshakeTranscript {
        suite_id: hello.suite_id,
        initiator_identity: hello.initiator_identity,
        responder_identity: hello.responder_identity,
        initiator_ephemeral_public: &hello.initiator_ephemeral_public,
        responder_ephemeral_public: &accept.responder_ephemeral_public,
        session_id: accept.session_id,
        epoch: accept.epoch,
        endpoint_context_hash: accept.endpoint_context_hash,
    };
    let _ = transcript.canonical_bytes();
    let _ = transcript.hash();

    // Exercise the initiator's post-signature validation path with arbitrary
    // ACCEPT fields while keeping its own HELLO internally consistent.
    if let Ok(initiator_state) = ClassicInitiator::start(
        &initiator,
        responder_verifying,
        SUITE_CHACHA20_POLY1305,
        CONTEXT,
        7,
        NOW,
    ) {
        let _ = initiator_state.finish(&accept, NOW);
    }
});
