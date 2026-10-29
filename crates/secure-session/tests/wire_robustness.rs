use fuckipv6_secure_session::confirmation::ConfirmationTag;
use fuckipv6_secure_session::cookie::{decode_cookie_challenge, Cookie};
use fuckipv6_secure_session::rekey::{RekeyAccept, RekeyConfirm, RekeyInit};
use fuckipv6_secure_session::{
    DataCipherSuite, DataHeader, Direction, ReceiveCipher, AEAD_TAG_LEN, DATA_HEADER_LEN,
    MAX_PACKETS_PER_EPOCH, MIN_DATA_PACKET_LEN, SESSION_ID_LEN,
};

const ITERATIONS: usize = 4096;

/// Small deterministic PRNG so the robustness corpus is reproducible without a
/// new dependency or network access.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn next_usize(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }

    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            let value = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
    }
}

#[test]
fn every_public_v2_decoder_accepts_arbitrary_bytes_without_panicking() {
    let mut rng = XorShift64(0x6a09_e667_f3bc_c909);
    let fixed_lengths = [
        0, 1, 2, 7, 8, 15, 16, 31, 32, 33, 40, 41, 42, 43, 44, 60, 188, 219, 220, 221,
    ];

    for case in 0..ITERATIONS {
        let len = if case < fixed_lengths.len() {
            fixed_lengths[case]
        } else {
            rng.next_usize(4097)
        };
        let mut bytes = vec![0u8; len];
        rng.fill(&mut bytes);

        let _ = DataHeader::decode(&bytes);
        let _ = Cookie::decode(&bytes);
        let _ = decode_cookie_challenge(&bytes);
        let _ = RekeyInit::decode(&bytes);
        let _ = RekeyAccept::decode(&bytes);
        let _ = RekeyConfirm::decode(&bytes);
        let _ = ConfirmationTag::from_slice(&bytes);
    }
}

#[test]
fn random_ciphertext_with_a_matching_header_never_authenticates() {
    let mut rng = XorShift64(0xbb67_ae85_84ca_a73b);
    let session_id = [0x42; SESSION_ID_LEN];

    for _ in 0..ITERATIONS {
        let mut receiver = ReceiveCipher::new_with_suite(
            if rng.next_u64() & 1 == 0 {
                DataCipherSuite::ChaCha20Poly1305
            } else {
                DataCipherSuite::Aes256Gcm
            },
            [0x24; 32],
            session_id,
            Direction::InitiatorToResponder,
            7,
            0x0102_0304,
        );
        let payload_len = rng.next_usize(512);
        let mut packet = vec![0u8; DATA_HEADER_LEN + payload_len + AEAD_TAG_LEN];
        let counter = rng.next_u64() % MAX_PACKETS_PER_EPOCH;
        packet[..DATA_HEADER_LEN]
            .copy_from_slice(&DataHeader::new(0x0102_0304, 7, counter).encode());
        rng.fill(&mut packet[DATA_HEADER_LEN..]);

        assert!(packet.len() >= MIN_DATA_PACKET_LEN);
        assert!(receiver.open_in_place(&mut packet).is_err());
    }
}
