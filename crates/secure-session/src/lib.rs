//! Transport-independent primitives for the fuckipv6 secure session protocol v2.
//!
//! This crate intentionally does not implement the post-quantum handshake yet.
//! It contains the byte-exact data frame, nonce construction, and replay-window
//! state machine that can be tested independently from the eventually selected
//! X-Wing and ML-DSA implementations.

use thiserror::Error;

/// Protocol version encoded in every fixed data header.
pub const PROTOCOL_VERSION: u8 = 2;
/// Version and packet kind byte for an encrypted data packet.
pub const DATA_PACKET_KIND: u8 = 0x20;
/// Fixed unencrypted header size for a v2 data packet.
pub const DATA_HEADER_LEN: usize = 16;
/// Poly1305/GCM authentication tag size.
pub const AEAD_TAG_LEN: usize = 16;
/// Smallest valid v2 data packet, including an empty plaintext.
pub const MIN_DATA_PACKET_LEN: usize = DATA_HEADER_LEN + AEAD_TAG_LEN;
/// Largest UDP payload accepted by the protocol parser.
pub const MAX_DATA_PACKET_LEN: usize = u16::MAX as usize;
/// Number of packet counters tracked by each replay window.
pub const REPLAY_WINDOW_SIZE: u64 = 4096;
const REPLAY_WINDOW_WORDS: usize = (REPLAY_WINDOW_SIZE as usize) / 64;

/// Direction-specific nonce domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Initiator to responder.
    InitiatorToResponder,
    /// Responder to initiator.
    ResponderToInitiator,
}

impl Direction {
    /// Direction-specific four-byte Nonce prefix from the protocol spec.
    pub const fn prefix(self) -> [u8; 4] {
        match self {
            Self::InitiatorToResponder => [0, 0, 0, 1],
            Self::ResponderToInitiator => [0, 0, 0, 2],
        }
    }
}

/// Construct the 96-bit AEAD Nonce for a direction and packet counter.
///
/// The counter is encoded as an unsigned big-endian integer. Reusing a counter
/// in the same direction and epoch is forbidden by the session state machine.
pub const fn nonce_for(direction: Direction, counter: u64) -> [u8; 12] {
    let prefix = direction.prefix();
    let counter = counter.to_be_bytes();
    [
        prefix[0], prefix[1], prefix[2], prefix[3], counter[0], counter[1], counter[2], counter[3],
        counter[4], counter[5], counter[6], counter[7],
    ]
}

/// Fixed 16-byte header preceding every v2 encrypted data payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataHeader {
    pub flags: u8,
    pub receiver_index: u32,
    pub epoch: u16,
    pub counter: u64,
}

impl DataHeader {
    pub const fn new(receiver_index: u32, epoch: u16, counter: u64) -> Self {
        Self {
            flags: 0,
            receiver_index,
            epoch,
            counter,
        }
    }

    /// Encode the header in network byte order.
    pub const fn encode(self) -> [u8; DATA_HEADER_LEN] {
        let epoch = self.epoch.to_be_bytes();
        let receiver_index = self.receiver_index.to_be_bytes();
        let counter = self.counter.to_be_bytes();
        [
            DATA_PACKET_KIND,
            self.flags,
            epoch[0],
            epoch[1],
            receiver_index[0],
            receiver_index[1],
            receiver_index[2],
            receiver_index[3],
            counter[0],
            counter[1],
            counter[2],
            counter[3],
            counter[4],
            counter[5],
            counter[6],
            counter[7],
        ]
    }

    /// Decode and validate a fixed data header.
    pub fn decode(bytes: &[u8]) -> Result<Self, SessionError> {
        if bytes.len() < DATA_HEADER_LEN {
            return Err(SessionError::PacketTooShort);
        }

        if bytes[0] != DATA_PACKET_KIND {
            return Err(SessionError::UnsupportedPacketKind(bytes[0]));
        }

        let flags = bytes[1];
        if flags != 0 {
            return Err(SessionError::UnsupportedFlags(flags));
        }

        Ok(Self {
            flags,
            epoch: u16::from_be_bytes(bytes[2..4].try_into().expect("fixed slice")),
            receiver_index: u32::from_be_bytes(bytes[4..8].try_into().expect("fixed slice")),
            counter: u64::from_be_bytes(bytes[8..16].try_into().expect("fixed slice")),
        })
    }
}

/// Sliding replay window for one `(session, epoch, direction)` triple.
///
/// Call [`check`](Self::check) before AEAD decryption to reject cheap repeats,
/// then call [`mark_authenticated`](Self::mark_authenticated) only after the
/// packet authenticates successfully. The two-step API prevents a forged packet
/// from consuming a replay-window slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayWindow {
    max_counter: Option<u64>,
    bits: [u64; REPLAY_WINDOW_WORDS],
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    pub const fn new() -> Self {
        Self {
            max_counter: None,
            bits: [0; REPLAY_WINDOW_WORDS],
        }
    }

    /// Validate a counter without modifying the window.
    pub fn check(&self, counter: u64) -> Result<(), SessionError> {
        let Some(max_counter) = self.max_counter else {
            return Ok(());
        };

        if counter > max_counter {
            return Ok(());
        }

        let distance = max_counter - counter;
        if distance >= REPLAY_WINDOW_SIZE {
            return Err(SessionError::CounterTooOld);
        }

        if self.bit_is_set(distance as usize) {
            return Err(SessionError::ReplayDetected);
        }

        Ok(())
    }

    /// Mark an authenticated counter as received.
    ///
    /// This method revalidates the counter, so a caller cannot accidentally mark
    /// a duplicate or expired packet after the AEAD operation.
    pub fn mark_authenticated(&mut self, counter: u64) -> Result<(), SessionError> {
        match self.max_counter {
            None => {
                self.max_counter = Some(counter);
                self.set_bit(0);
            }
            Some(max_counter) if counter > max_counter => {
                let advance = counter - max_counter;
                self.shift_left(advance);
                self.max_counter = Some(counter);
                self.set_bit(0);
            }
            Some(max_counter) => {
                let distance = max_counter - counter;
                if distance >= REPLAY_WINDOW_SIZE {
                    return Err(SessionError::CounterTooOld);
                }
                if self.bit_is_set(distance as usize) {
                    return Err(SessionError::ReplayDetected);
                }
                self.set_bit(distance as usize);
            }
        }

        Ok(())
    }

    pub const fn max_counter(&self) -> Option<u64> {
        self.max_counter
    }

    fn bit_is_set(&self, index: usize) -> bool {
        let word = index / 64;
        let bit = index % 64;
        (self.bits[word] & (1u64 << bit)) != 0
    }

    fn set_bit(&mut self, index: usize) {
        let word = index / 64;
        let bit = index % 64;
        self.bits[word] |= 1u64 << bit;
    }

    fn shift_left(&mut self, distance: u64) {
        if distance >= REPLAY_WINDOW_SIZE {
            self.bits = [0; REPLAY_WINDOW_WORDS];
            return;
        }

        let word_shift = (distance / 64) as usize;
        let bit_shift = (distance % 64) as u32;
        let mut shifted = [0u64; REPLAY_WINDOW_WORDS];

        for (destination, slot) in shifted.iter_mut().enumerate().skip(word_shift) {
            let source = destination - word_shift;
            let mut value = self.bits[source] << bit_shift;
            if bit_shift != 0 && source != 0 {
                value |= self.bits[source - 1] >> (64 - bit_shift);
            }
            *slot = value;
        }

        self.bits = shifted;
    }
}

/// Errors produced by protocol-v2 framing and replay checks.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    #[error("packet is shorter than the fixed v2 data header")]
    PacketTooShort,
    #[error("unsupported v2 packet kind: 0x{0:02x}")]
    UnsupportedPacketKind(u8),
    #[error("unsupported data flags: 0x{0:02x}")]
    UnsupportedFlags(u8),
    #[error("packet counter was already accepted")]
    ReplayDetected,
    #[error("packet counter is outside the replay window")]
    CounterTooOld,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_header_round_trips_in_network_order() {
        let header = DataHeader::new(0x0102_0304, 0x0506, 0x1112_1314_1516_1718);
        let encoded = header.encode();

        assert_eq!(encoded[0], DATA_PACKET_KIND);
        assert_eq!(&encoded[2..4], &[5, 6]);
        assert_eq!(&encoded[4..8], &[1, 2, 3, 4]);
        assert_eq!(
            &encoded[8..16],
            &[0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]
        );
        assert_eq!(DataHeader::decode(&encoded).expect("decode header"), header);
    }

    #[test]
    fn data_header_rejects_invalid_fields() {
        let valid = DataHeader::new(7, 9, 11).encode();
        assert_eq!(
            DataHeader::decode(&valid[..DATA_HEADER_LEN - 1]),
            Err(SessionError::PacketTooShort)
        );

        let mut invalid = valid;
        invalid[0] = 0x21;
        assert_eq!(
            DataHeader::decode(&invalid),
            Err(SessionError::UnsupportedPacketKind(0x21))
        );

        let mut invalid = valid;
        invalid[1] = 1;
        assert_eq!(
            DataHeader::decode(&invalid),
            Err(SessionError::UnsupportedFlags(1))
        );
    }

    #[test]
    fn direction_nonces_are_domain_separated() {
        let i2r = nonce_for(Direction::InitiatorToResponder, 0x0102_0304_0506_0708);
        let r2i = nonce_for(Direction::ResponderToInitiator, 0x0102_0304_0506_0708);

        assert_eq!(&i2r[..4], &[0, 0, 0, 1]);
        assert_eq!(&r2i[..4], &[0, 0, 0, 2]);
        assert_eq!(&i2r[4..], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_ne!(i2r, r2i);
    }

    #[test]
    fn replay_window_accepts_first_packet_once() {
        let mut window = ReplayWindow::new();
        assert_eq!(window.check(42), Ok(()));
        assert_eq!(window.mark_authenticated(42), Ok(()));
        assert_eq!(window.max_counter(), Some(42));
        assert_eq!(window.check(42), Err(SessionError::ReplayDetected));
        assert_eq!(
            window.mark_authenticated(42),
            Err(SessionError::ReplayDetected)
        );
    }

    #[test]
    fn replay_window_accepts_out_of_order_packets() {
        let mut window = ReplayWindow::new();
        window.mark_authenticated(100).expect("first packet");

        assert_eq!(window.check(99), Ok(()));
        window
            .mark_authenticated(99)
            .expect("older in-window packet");
        assert_eq!(window.check(99), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(100), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(101), Ok(()));
    }

    #[test]
    fn replay_window_expires_old_packets_after_advance() {
        let mut window = ReplayWindow::new();
        window.mark_authenticated(0).expect("first packet");
        window
            .mark_authenticated(REPLAY_WINDOW_SIZE)
            .expect("advance to window boundary");

        assert_eq!(window.check(0), Err(SessionError::CounterTooOld));
        assert_eq!(window.check(1), Ok(()));
        assert_eq!(window.mark_authenticated(1), Ok(()));
    }

    #[test]
    fn replay_window_handles_multiword_shift() {
        let mut window = ReplayWindow::new();
        window.mark_authenticated(0).expect("packet zero");
        window.mark_authenticated(64).expect("packet 64");
        window.mark_authenticated(65).expect("packet 65");

        assert_eq!(window.check(0), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(64), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(65), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(66), Ok(()));

        window.mark_authenticated(128).expect("packet 128");
        assert_eq!(window.check(64), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(65), Err(SessionError::ReplayDetected));
        assert_eq!(window.check(66), Ok(()));
        window
            .mark_authenticated(66)
            .expect("unseen packet after shift");
    }

    #[test]
    fn replay_window_large_jump_clears_history() {
        let mut window = ReplayWindow::new();
        window.mark_authenticated(7).expect("initial packet");
        window
            .mark_authenticated(REPLAY_WINDOW_SIZE * 10)
            .expect("large jump");

        assert_eq!(window.check(7), Err(SessionError::CounterTooOld));
        assert_eq!(
            window.check(REPLAY_WINDOW_SIZE * 10),
            Err(SessionError::ReplayDetected)
        );
    }

    #[test]
    fn replay_window_supports_full_advertised_range() {
        let mut window = ReplayWindow::new();
        let start = 1_000_000;
        window.mark_authenticated(start).expect("window start");

        for offset in (1..REPLAY_WINDOW_SIZE).rev() {
            let counter = start + offset;
            assert_eq!(window.check(counter), Ok(()));
            window
                .mark_authenticated(counter)
                .expect("in-window packet");
        }

        assert_eq!(window.check(start - 1), Err(SessionError::CounterTooOld));
        assert_eq!(
            window.check(start + REPLAY_WINDOW_SIZE - 1),
            Err(SessionError::ReplayDetected)
        );
    }
}
