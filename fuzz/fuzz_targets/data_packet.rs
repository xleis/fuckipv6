#![no_main]

use fuckipv6_secure_session::{DataCipherSuite, Direction, ReceiveCipher, SESSION_ID_LEN};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut receiver = ReceiveCipher::new_with_suite(
        if data.first().is_some_and(|value| value & 1 == 1) {
            DataCipherSuite::Aes256Gcm
        } else {
            DataCipherSuite::ChaCha20Poly1305
        },
        [0x24; 32],
        [0x42; SESSION_ID_LEN],
        Direction::InitiatorToResponder,
        7,
        0x0102_0304,
    );
    let mut packet = data.to_vec();
    let _ = receiver.open_in_place(&mut packet);
});
