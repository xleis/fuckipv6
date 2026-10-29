//! Transport-independent primitives for the fuckipv6 secure session protocol v2.
//!
//! This crate intentionally does not implement the post-quantum handshake yet.
//! It contains the byte-exact data frame, nonce construction, replay window,
//! bounded epoch lifecycle, allocation-free AEAD packet operations, and the
//! online `REKEY_INIT`/`REKEY_ACCEPT`/`REKEY_CONFIRM` state machine that can be
//! tested independently from the eventually selected X-Wing and ML-DSA
//! implementations.

pub mod confirmation;
pub mod handshake;
pub mod handshake_epoch;
pub mod key_schedule;
pub mod rekey;
pub mod rekey_codec;
pub mod transcript;

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, Tag};
use std::time::{Duration, Instant};
use thiserror::Error;
use zeroize::Zeroize;

/// Protocol version encoded in every fixed data header.
pub const PROTOCOL_VERSION: u8 = 2;
/// Version and packet kind byte for an encrypted data packet.
pub const DATA_PACKET_KIND: u8 = 0x20;
/// Fixed unencrypted header size for a v2 data packet.
pub const DATA_HEADER_LEN: usize = 16;
/// Encoded session identifier size from the protocol spec.
pub const SESSION_ID_LEN: usize = 16;
/// Poly1305/GCM authentication tag size.
pub const AEAD_TAG_LEN: usize = 16;
/// Smallest valid v2 data packet, including an empty plaintext.
pub const MIN_DATA_PACKET_LEN: usize = DATA_HEADER_LEN + AEAD_TAG_LEN;
/// Largest UDP payload accepted by the protocol parser.
pub const MAX_DATA_PACKET_LEN: usize = u16::MAX as usize;
/// AEAD associated-data length: header, length-prefixed SID, and ciphertext length.
pub const DATA_AAD_LEN: usize = DATA_HEADER_LEN + 2 + SESSION_ID_LEN + 2;
/// Per-epoch packet counter limit. Counters use the range `0..MAX_PACKETS_PER_EPOCH`.
pub const MAX_PACKETS_PER_EPOCH: u64 = 1 << 32;
/// Number of packet counters tracked by each replay window.
pub const REPLAY_WINDOW_SIZE: u64 = 4096;
/// Hard per-epoch plaintext byte limit from the protocol specification.
pub const MAX_EPOCH_BYTES: u64 = 1 << 60;
/// Maximum active epoch lifetime before rekeying is required.
pub const MAX_EPOCH_LIFETIME: Duration = Duration::from_secs(120);
/// Time allowed for packets encrypted under the immediately previous epoch.
pub const RETIRED_EPOCH_GRACE: Duration = Duration::from_secs(5);
/// Number of retired receive epochs retained for in-flight packets.
pub const MAX_RETIRED_RECEIVE_EPOCHS: usize = 2;
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

/// Build the associated data authenticated by every v2 data packet.
///
/// `payload_len` is the plaintext/ciphertext length in bytes, excluding the
/// fixed header and authentication tag. The session identifier and payload
/// length use the protocol's two-byte big-endian length prefix.
pub const fn data_aad(
    header: DataHeader,
    session_id: [u8; SESSION_ID_LEN],
    payload_len: u16,
) -> [u8; DATA_AAD_LEN] {
    let header = header.encode();
    let payload_len = payload_len.to_be_bytes();
    let mut aad = [0u8; DATA_AAD_LEN];

    let mut index = 0;
    while index < DATA_HEADER_LEN {
        aad[index] = header[index];
        index += 1;
    }

    aad[DATA_HEADER_LEN] = 0;
    aad[DATA_HEADER_LEN + 1] = SESSION_ID_LEN as u8;

    let session_offset = DATA_HEADER_LEN + 2;
    let mut session_index = 0;
    while session_index < SESSION_ID_LEN {
        aad[session_offset + session_index] = session_id[session_index];
        session_index += 1;
    }

    let payload_offset = session_offset + SESSION_ID_LEN;
    aad[payload_offset] = payload_len[0];
    aad[payload_offset + 1] = payload_len[1];
    aad
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

/// Sending state for one direction of a v2 session epoch.
///
/// The outgoing counter starts at zero and never wraps. `peer_receiver_index`
/// is the receiver index advertised by the peer and is written into every
/// outgoing header. The underlying AEAD implementation zeroizes its expanded
/// key material when this value is dropped.
pub struct SendCipher {
    cipher: ChaCha20Poly1305,
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    epoch: u16,
    peer_receiver_index: u32,
    next_send_counter: u64,
}

impl SendCipher {
    /// Create send state from a 32-byte derived epoch key.
    ///
    /// `peer_receiver_index` must be the local receiver index advertised by the
    /// peer, not the sender's own index. The input key is zeroized after the
    /// AEAD state has been initialized.
    pub fn new(
        mut key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        peer_receiver_index: u32,
    ) -> Self {
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .expect("ChaCha20-Poly1305 requires a 32-byte key");
        key.zeroize();

        Self {
            cipher,
            session_id,
            direction,
            epoch,
            peer_receiver_index,
            next_send_counter: 0,
        }
    }

    /// Next outgoing packet counter.
    pub const fn next_send_counter(&self) -> u64 {
        self.next_send_counter
    }

    /// Encrypt a caller-provided plaintext into a packet buffer without allocating.
    ///
    /// The returned length is `DATA_HEADER_LEN + plaintext.len() + AEAD_TAG_LEN`.
    /// The plaintext is copied into the packet buffer before in-place encryption.
    /// Callers that already store payloads at the fixed header offset should use
    /// [`seal_in_place`](Self::seal_in_place).
    pub fn seal(&mut self, plaintext: &[u8], packet: &mut [u8]) -> Result<usize, SessionError> {
        let packet_len = data_packet_len(plaintext.len())?;
        if packet.len() < packet_len {
            return Err(SessionError::BufferTooSmall);
        }
        if self.next_send_counter >= MAX_PACKETS_PER_EPOCH {
            return Err(SessionError::CounterExhausted);
        }

        packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext.len()].copy_from_slice(plaintext);
        self.seal_in_place(packet, plaintext.len())?;
        Ok(packet_len)
    }

    /// Encrypt a plaintext already located at `packet[DATA_HEADER_LEN..]` in place.
    ///
    /// `packet` must contain enough storage for the fixed header, the
    /// `plaintext_len` bytes to encrypt, and the 16-byte authentication tag.
    /// The fixed header and authentication tag are overwritten by this method.
    pub fn seal_in_place(
        &mut self,
        packet: &mut [u8],
        plaintext_len: usize,
    ) -> Result<(), SessionError> {
        let packet_len = data_packet_len(plaintext_len)?;
        if packet.len() < packet_len {
            return Err(SessionError::BufferTooSmall);
        }
        if self.next_send_counter >= MAX_PACKETS_PER_EPOCH {
            return Err(SessionError::CounterExhausted);
        }

        let counter = self.next_send_counter;
        let header = DataHeader::new(self.peer_receiver_index, self.epoch, counter);
        packet[..DATA_HEADER_LEN].copy_from_slice(&header.encode());
        let aad = data_aad(header, self.session_id, plaintext_len as u16);
        let nonce = nonce_for(self.direction, counter);
        let tag = self
            .cipher
            .encrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                &aad,
                &mut packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext_len],
            )
            .map_err(|_| SessionError::AuthenticationFailed)?;

        packet[DATA_HEADER_LEN + plaintext_len..packet_len].copy_from_slice(&tag);
        self.next_send_counter += 1;
        Ok(())
    }
}

/// Receiving state for one direction of a v2 session epoch.
///
/// Incoming packets are checked against `local_receiver_index` and a replay
/// window before AEAD verification. A counter is marked received only after
/// successful authentication. The underlying AEAD implementation zeroizes its
/// expanded key material when this value is dropped.
pub struct ReceiveCipher {
    cipher: ChaCha20Poly1305,
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    epoch: u16,
    local_receiver_index: u32,
    replay_window: ReplayWindow,
}

impl ReceiveCipher {
    /// Create receive state from a 32-byte derived epoch key.
    ///
    /// `local_receiver_index` is the index assigned by this endpoint and must
    /// match the `receiver_index` field written by the peer. The input key is
    /// zeroized after the AEAD state has been initialized.
    pub fn new(
        mut key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        local_receiver_index: u32,
    ) -> Self {
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .expect("ChaCha20-Poly1305 requires a 32-byte key");
        key.zeroize();

        Self {
            cipher,
            session_id,
            direction,
            epoch,
            local_receiver_index,
            replay_window: ReplayWindow::new(),
        }
    }

    /// Authenticate and decrypt a v2 data packet in place without allocating.
    ///
    /// On success, returns the decrypted plaintext beginning at the fixed header
    /// offset. On failure, the replay window is left unchanged so an attacker
    /// cannot consume a counter with an invalid packet.
    pub fn open_in_place<'a>(
        &mut self,
        packet: &'a mut [u8],
    ) -> Result<&'a mut [u8], SessionError> {
        if packet.len() < MIN_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooShort);
        }
        if packet.len() > MAX_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooLong);
        }

        let header = DataHeader::decode(packet)?;
        if header.receiver_index != self.local_receiver_index {
            return Err(SessionError::ReceiverIndexMismatch);
        }
        if header.epoch != self.epoch {
            return Err(SessionError::EpochMismatch);
        }
        if header.counter >= MAX_PACKETS_PER_EPOCH {
            return Err(SessionError::CounterExhausted);
        }
        self.replay_window.check(header.counter)?;

        let plaintext_len = packet.len() - DATA_HEADER_LEN - AEAD_TAG_LEN;
        let aad = data_aad(header, self.session_id, plaintext_len as u16);
        let nonce = nonce_for(self.direction, header.counter);
        let (_, tail) = packet.split_at_mut(DATA_HEADER_LEN);
        let (plaintext, tag) = tail.split_at_mut(plaintext_len);

        self.cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                &aad,
                plaintext,
                Tag::from_slice(tag),
            )
            .map_err(|_| SessionError::AuthenticationFailed)?;

        self.replay_window.mark_authenticated(header.counter)?;
        Ok(plaintext)
    }
}

struct RetiredReceiveCipher {
    cipher: ReceiveCipher,
    discard_at: Instant,
}

/// Active and recently retired data ciphers for one authenticated v2 session.
///
/// `EpochSession` enforces the protocol's time and byte limits, requires a
/// monotonically increasing epoch on rotation, and retains a bounded number of
/// previous receive ciphers so in-flight packets can complete after rekeying.
/// Retired keys are dropped, and therefore zeroized by the AEAD implementation,
/// as soon as their grace period expires or the retention bound is exceeded.
pub struct EpochSession {
    active_send: SendCipher,
    active_receive: ReceiveCipher,
    active_started_at: Instant,
    sent_bytes: u64,
    retired_receive: [Option<RetiredReceiveCipher>; MAX_RETIRED_RECEIVE_EPOCHS],
}

impl EpochSession {
    /// Create a session from matched send and receive ciphers for one epoch.
    pub fn new(
        send: SendCipher,
        receive: ReceiveCipher,
        now: Instant,
    ) -> Result<Self, SessionError> {
        validate_epoch_pair(&send, &receive)?;
        Ok(Self {
            active_send: send,
            active_receive: receive,
            active_started_at: now,
            sent_bytes: 0,
            retired_receive: std::array::from_fn(|_| None),
        })
    }

    /// Session identifier installed in both active ciphers.
    pub const fn session_id(&self) -> [u8; SESSION_ID_LEN] {
        self.active_send.session_id
    }

    /// Active data epoch.
    pub const fn epoch(&self) -> u16 {
        self.active_send.epoch
    }

    /// Encrypt a packet under the active epoch.
    pub fn seal(
        &mut self,
        plaintext: &[u8],
        packet: &mut [u8],
        now: Instant,
    ) -> Result<usize, SessionError> {
        self.ensure_active_epoch_usable(plaintext.len(), now)?;
        let packet_len = self.active_send.seal(plaintext, packet)?;
        self.sent_bytes = self
            .sent_bytes
            .checked_add(plaintext.len() as u64)
            .ok_or(SessionError::RekeyRequired)?;
        Ok(packet_len)
    }

    /// Authenticate and decrypt a packet under the active or a retained epoch.
    pub fn open_in_place<'a>(
        &mut self,
        packet: &'a mut [u8],
        now: Instant,
    ) -> Result<&'a mut [u8], SessionError> {
        self.prune_retired(now);
        if packet.len() < MIN_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooShort);
        }
        if packet.len() > MAX_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooLong);
        }
        let header = DataHeader::decode(packet)?;

        let retired_index = if receive_matches(&self.active_receive, header) {
            None
        } else {
            self.retired_receive.iter().position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|retired| receive_matches(&retired.cipher, header))
            })
        };

        match retired_index {
            Some(index) => self.retired_receive[index]
                .as_mut()
                .expect("position only returns occupied entries")
                .cipher
                .open_in_place(packet),
            None if receive_matches(&self.active_receive, header) => {
                self.active_receive.open_in_place(packet)
            }
            None => Err(SessionError::UnknownEpoch),
        }
    }

    /// Whether the active epoch must be replaced before another packet is sent.
    pub fn rekey_required(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.active_started_at) >= MAX_EPOCH_LIFETIME
            || self.sent_bytes >= MAX_EPOCH_BYTES
    }

    /// Install a new epoch and retain the previous receive cipher for a short grace period.
    pub fn rotate(
        &mut self,
        send: SendCipher,
        receive: ReceiveCipher,
        now: Instant,
    ) -> Result<(), SessionError> {
        validate_epoch_pair(&send, &receive)?;
        if receive.epoch <= self.active_receive.epoch {
            return Err(SessionError::EpochRollback);
        }

        let old_send = std::mem::replace(&mut self.active_send, send);
        drop(old_send);
        let old_receive = std::mem::replace(&mut self.active_receive, receive);
        for index in (1..MAX_RETIRED_RECEIVE_EPOCHS).rev() {
            self.retired_receive[index] = self.retired_receive[index - 1].take();
        }
        self.retired_receive[0] = Some(RetiredReceiveCipher {
            cipher: old_receive,
            discard_at: now + RETIRED_EPOCH_GRACE,
        });
        self.active_started_at = now;
        self.sent_bytes = 0;
        Ok(())
    }

    fn ensure_active_epoch_usable(
        &self,
        plaintext_len: usize,
        now: Instant,
    ) -> Result<(), SessionError> {
        if self.rekey_required(now) {
            return Err(SessionError::RekeyRequired);
        }
        let next_bytes = self
            .sent_bytes
            .checked_add(plaintext_len as u64)
            .ok_or(SessionError::RekeyRequired)?;
        if next_bytes > MAX_EPOCH_BYTES {
            return Err(SessionError::RekeyRequired);
        }
        Ok(())
    }

    fn prune_retired(&mut self, now: Instant) {
        for entry in &mut self.retired_receive {
            if entry
                .as_ref()
                .is_some_and(|retired| now >= retired.discard_at)
            {
                *entry = None;
            }
        }
    }
}

fn validate_epoch_pair(send: &SendCipher, receive: &ReceiveCipher) -> Result<(), SessionError> {
    let opposite_directions = matches!(
        (send.direction, receive.direction),
        (
            Direction::InitiatorToResponder,
            Direction::ResponderToInitiator
        ) | (
            Direction::ResponderToInitiator,
            Direction::InitiatorToResponder
        )
    );
    if !opposite_directions || send.session_id != receive.session_id || send.epoch != receive.epoch
    {
        return Err(SessionError::InvalidEpochState);
    }
    Ok(())
}

fn receive_matches(receive: &ReceiveCipher, header: DataHeader) -> bool {
    receive.epoch == header.epoch && receive.local_receiver_index == header.receiver_index
}

fn data_packet_len(plaintext_len: usize) -> Result<usize, SessionError> {
    let packet_len = DATA_HEADER_LEN
        .checked_add(plaintext_len)
        .and_then(|len| len.checked_add(AEAD_TAG_LEN))
        .ok_or(SessionError::PacketTooLong)?;
    if packet_len > MAX_DATA_PACKET_LEN {
        return Err(SessionError::PacketTooLong);
    }
    Ok(packet_len)
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
    #[error("packet is shorter than the minimum v2 data frame")]
    PacketTooShort,
    #[error("packet exceeds the maximum v2 UDP payload")]
    PacketTooLong,
    #[error("packet buffer is too small for the requested plaintext")]
    BufferTooSmall,
    #[error("unsupported v2 packet kind: 0x{0:02x}")]
    UnsupportedPacketKind(u8),
    #[error("unsupported data flags: 0x{0:02x}")]
    UnsupportedFlags(u8),
    #[error("packet counter was already accepted")]
    ReplayDetected,
    #[error("packet counter is outside the replay window")]
    CounterTooOld,
    #[error("packet counter exhausted this session epoch")]
    CounterExhausted,
    #[error("packet receiver index does not match this session")]
    ReceiverIndexMismatch,
    #[error("packet epoch does not match this session")]
    EpochMismatch,
    #[error("packet belongs to an unknown or expired epoch")]
    UnknownEpoch,
    #[error("active session epoch must be replaced")]
    RekeyRequired,
    #[error("new epoch is not newer than the active epoch")]
    EpochRollback,
    #[error("send and receive ciphers do not form a valid epoch pair")]
    InvalidEpochState,
    #[error("packet authentication failed")]
    AuthenticationFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: [u8; 32] = [0x42; 32];
    const TEST_SESSION_ID: [u8; SESSION_ID_LEN] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];

    fn cipher_pair() -> (SendCipher, ReceiveCipher) {
        (
            SendCipher::new(
                TEST_KEY,
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                17,
                0x0102_0304,
            ),
            ReceiveCipher::new(
                TEST_KEY,
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                17,
                0x0102_0304,
            ),
        )
    }

    fn initiator_ciphers(epoch: u16, key_generation: u8) -> (SendCipher, ReceiveCipher) {
        (
            SendCipher::new(
                [0x10 + key_generation; 32],
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                epoch,
                0xa1a2_a3a4,
            ),
            ReceiveCipher::new(
                [0x20 + key_generation; 32],
                TEST_SESSION_ID,
                Direction::ResponderToInitiator,
                epoch,
                0xb1b2_b3b4,
            ),
        )
    }

    fn responder_ciphers(epoch: u16, key_generation: u8) -> (SendCipher, ReceiveCipher) {
        (
            SendCipher::new(
                [0x20 + key_generation; 32],
                TEST_SESSION_ID,
                Direction::ResponderToInitiator,
                epoch,
                0xb1b2_b3b4,
            ),
            ReceiveCipher::new(
                [0x10 + key_generation; 32],
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                epoch,
                0xa1a2_a3a4,
            ),
        )
    }

    fn initiator_epoch(epoch: u16, key_generation: u8, now: Instant) -> EpochSession {
        let (send, receive) = initiator_ciphers(epoch, key_generation);
        EpochSession::new(send, receive, now).expect("valid initiator epoch")
    }

    fn responder_epoch(epoch: u16, key_generation: u8, now: Instant) -> EpochSession {
        let (send, receive) = responder_ciphers(epoch, key_generation);
        EpochSession::new(send, receive, now).expect("valid responder epoch")
    }

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
    fn data_aad_binds_header_session_and_payload_length() {
        let header = DataHeader::new(0x0102_0304, 0x0506, 0x0708_090a_0b0c_0d0e);
        let aad = data_aad(header, TEST_SESSION_ID, 0x1234);

        assert_eq!(&aad[..DATA_HEADER_LEN], &header.encode());
        assert_eq!(&aad[DATA_HEADER_LEN..DATA_HEADER_LEN + 2], &[0, 16]);
        assert_eq!(
            &aad[DATA_HEADER_LEN + 2..DATA_HEADER_LEN + 2 + SESSION_ID_LEN],
            &TEST_SESSION_ID
        );
        assert_eq!(&aad[DATA_AAD_LEN - 2..], &[0x12, 0x34]);
    }

    #[test]
    fn send_and_receive_ciphers_round_trip_and_write_expected_header() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut packet = [0u8; 128];

        let packet_len = sender.seal(b"hello wireguard", &mut packet).expect("seal");
        assert_eq!(
            packet_len,
            DATA_HEADER_LEN + b"hello wireguard".len() + AEAD_TAG_LEN
        );
        assert_eq!(sender.next_send_counter(), 1);

        let header = DataHeader::decode(&packet).expect("decode");
        assert_eq!(header.receiver_index, 0x0102_0304);
        assert_eq!(header.epoch, 17);
        assert_eq!(header.counter, 0);

        let plaintext = receiver
            .open_in_place(&mut packet[..packet_len])
            .expect("open");
        assert_eq!(plaintext, b"hello wireguard");
    }

    #[test]
    fn send_and_receive_ciphers_support_empty_plaintext() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut packet = [0u8; MIN_DATA_PACKET_LEN];

        let packet_len = sender.seal(&[], &mut packet).expect("seal");
        assert_eq!(packet_len, MIN_DATA_PACKET_LEN);
        let plaintext = receiver
            .open_in_place(&mut packet[..packet_len])
            .expect("open");
        assert!(plaintext.is_empty());
    }

    #[test]
    fn authentication_failure_does_not_consume_replay_counter() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut packet = [0u8; 128];
        let packet_len = sender.seal(b"authentic", &mut packet).expect("seal");
        let mut corrupted = packet[..packet_len].to_vec();
        *corrupted.last_mut().expect("tag") ^= 1;

        assert_eq!(
            receiver.open_in_place(&mut corrupted),
            Err(SessionError::AuthenticationFailed)
        );
        assert_eq!(
            receiver
                .open_in_place(&mut packet[..packet_len])
                .expect("original packet"),
            b"authentic"
        );
    }

    #[test]
    fn authenticated_packet_cannot_be_replayed() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut packet = [0u8; 128];
        let packet_len = sender.seal(b"once", &mut packet).expect("seal");

        {
            receiver
                .open_in_place(&mut packet[..packet_len])
                .expect("first delivery");
        }
        assert_eq!(
            receiver.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::ReplayDetected)
        );
    }

    #[test]
    fn packets_can_be_opened_out_of_order() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut first = [0u8; 128];
        let mut second = [0u8; 128];
        let first_len = sender.seal(b"first", &mut first).expect("first seal");
        let second_len = sender.seal(b"second", &mut second).expect("second seal");

        assert_eq!(
            receiver
                .open_in_place(&mut second[..second_len])
                .expect("second delivery"),
            b"second"
        );
        assert_eq!(
            receiver
                .open_in_place(&mut first[..first_len])
                .expect("late first delivery"),
            b"first"
        );
    }

    #[test]
    fn receive_cipher_binds_direction_session_epoch_and_receiver_index() {
        let (mut sender, _) = cipher_pair();
        let mut packet = [0u8; 128];
        let packet_len = sender.seal(b"bound", &mut packet).expect("seal");

        let mut wrong_direction = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::ResponderToInitiator,
            17,
            0x0102_0304,
        );
        assert_eq!(
            wrong_direction.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::AuthenticationFailed)
        );

        let mut wrong_session = ReceiveCipher::new(
            TEST_KEY,
            [0xff; SESSION_ID_LEN],
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        assert_eq!(
            wrong_session.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::AuthenticationFailed)
        );

        let mut wrong_epoch = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            18,
            0x0102_0304,
        );
        assert_eq!(
            wrong_epoch.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::EpochMismatch)
        );

        let mut wrong_index = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0506_0708,
        );
        assert_eq!(
            wrong_index.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::ReceiverIndexMismatch)
        );
    }

    #[test]
    fn send_and_receive_cipher_indices_are_distinct_roles() {
        const PEER_INDEX: u32 = 0xa1a2_a3a4;
        const LOCAL_INDEX: u32 = 0xb1b2_b3b4;

        let mut initiator_send = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            PEER_INDEX,
        );
        let mut initiator_receive = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            LOCAL_INDEX,
        );
        let mut packet = [0u8; 128];
        let packet_len = initiator_send
            .seal(b"index roles", &mut packet)
            .expect("seal");

        assert_eq!(
            initiator_receive.open_in_place(&mut packet[..packet_len]),
            Err(SessionError::ReceiverIndexMismatch)
        );
        assert_eq!(
            DataHeader::decode(&packet).expect("header").receiver_index,
            PEER_INDEX
        );
    }

    #[test]
    fn packet_counter_never_wraps() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut packet = [0u8; 128];
        sender.next_send_counter = MAX_PACKETS_PER_EPOCH - 1;

        let packet_len = sender.seal(b"last", &mut packet).expect("last packet");
        assert_eq!(
            DataHeader::decode(&packet).expect("decode").counter,
            MAX_PACKETS_PER_EPOCH - 1
        );
        assert_eq!(
            receiver
                .open_in_place(&mut packet[..packet_len])
                .expect("open last packet"),
            b"last"
        );

        assert_eq!(
            sender.seal(b"exhausted", &mut packet),
            Err(SessionError::CounterExhausted)
        );
        assert_eq!(sender.next_send_counter(), MAX_PACKETS_PER_EPOCH);
    }

    #[test]
    fn send_and_receive_ciphers_reject_short_and_oversized_buffers() {
        let (mut sender, mut receiver) = cipher_pair();
        let mut short = [0u8; MIN_DATA_PACKET_LEN - 1];
        assert_eq!(
            receiver.open_in_place(&mut short),
            Err(SessionError::PacketTooShort)
        );
        assert_eq!(
            sender.seal(b"payload", &mut short),
            Err(SessionError::BufferTooSmall)
        );
        assert_eq!(
            sender.seal_in_place(&mut short, usize::MAX),
            Err(SessionError::PacketTooLong)
        );
    }

    #[test]
    fn epoch_session_round_trips_both_directions() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(1, 0, now);
        let mut responder = responder_epoch(1, 0, now);
        let mut request = [0u8; 128];
        let mut response = [0u8; 128];

        let request_len = initiator
            .seal(b"request", &mut request, now)
            .expect("seal request");
        assert_eq!(
            responder
                .open_in_place(&mut request[..request_len], now)
                .expect("open request"),
            b"request"
        );

        let response_len = responder
            .seal(b"response", &mut response, now)
            .expect("seal response");
        assert_eq!(
            initiator
                .open_in_place(&mut response[..response_len], now)
                .expect("open response"),
            b"response"
        );
    }

    #[test]
    fn epoch_session_requires_rekey_at_time_and_byte_limits() {
        let now = Instant::now();
        let mut session = initiator_epoch(1, 0, now);
        let mut packet = [0u8; 128];

        assert!(!session.rekey_required(now + MAX_EPOCH_LIFETIME - Duration::from_millis(1)));
        assert!(session.rekey_required(now + MAX_EPOCH_LIFETIME));
        assert_eq!(
            session.seal(b"late", &mut packet, now + MAX_EPOCH_LIFETIME),
            Err(SessionError::RekeyRequired)
        );

        session.active_started_at = now;
        session.sent_bytes = MAX_EPOCH_BYTES - 1;
        assert_eq!(
            session.seal(b"xx", &mut packet, now),
            Err(SessionError::RekeyRequired)
        );
        session.sent_bytes = MAX_EPOCH_BYTES;
        assert!(session.rekey_required(now));
    }

    #[test]
    fn epoch_rotation_accepts_in_flight_old_packets_only_during_grace() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(1, 0, now);
        let mut responder = responder_epoch(1, 0, now);
        let mut old_first = [0u8; 128];
        let mut old_second = [0u8; 128];
        let first_len = initiator
            .seal(b"old first", &mut old_first, now)
            .expect("seal first old packet");
        let second_len = initiator
            .seal(b"old second", &mut old_second, now)
            .expect("seal second old packet");

        let rotate_at = now + Duration::from_secs(1);
        let (new_send, new_receive) = responder_ciphers(2, 1);
        responder
            .rotate(new_send, new_receive, rotate_at)
            .expect("rotate responder");
        assert_eq!(
            responder
                .open_in_place(&mut old_first[..first_len], rotate_at)
                .expect("open in-flight packet"),
            b"old first"
        );
        assert_eq!(
            responder.open_in_place(
                &mut old_second[..second_len],
                rotate_at + RETIRED_EPOCH_GRACE
            ),
            Err(SessionError::UnknownEpoch)
        );

        let (new_send, new_receive) = initiator_ciphers(2, 1);
        initiator
            .rotate(new_send, new_receive, rotate_at)
            .expect("rotate initiator");
        let mut new_packet = [0u8; 128];
        let new_len = initiator
            .seal(b"new epoch", &mut new_packet, rotate_at)
            .expect("seal new epoch");
        assert_eq!(
            responder
                .open_in_place(&mut new_packet[..new_len], rotate_at)
                .expect("open new epoch"),
            b"new epoch"
        );
    }

    #[test]
    fn epoch_session_rejects_rollback_and_mismatched_cipher_contexts() {
        let now = Instant::now();
        let mut session = responder_epoch(2, 0, now);
        let (old_send, old_receive) = responder_ciphers(1, 0);
        assert_eq!(
            session.rotate(old_send, old_receive, now),
            Err(SessionError::EpochRollback)
        );

        let (send, _) = initiator_ciphers(1, 0);
        let wrong_direction_receive = ReceiveCipher::new(
            [0x20; 32],
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            0xb1b2_b3b4,
        );
        assert_eq!(
            EpochSession::new(send, wrong_direction_receive, now).err(),
            Some(SessionError::InvalidEpochState)
        );
    }

    #[test]
    fn epoch_session_bounds_retired_receive_keys() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(1, 0, now);
        let mut responder = responder_epoch(1, 0, now);
        let mut old_packet = [0u8; 128];
        let old_len = initiator
            .seal(b"evicted", &mut old_packet, now)
            .expect("seal old packet");

        for epoch in 2..=4 {
            let (send, receive) = responder_ciphers(epoch, epoch as u8 - 1);
            responder
                .rotate(send, receive, now + Duration::from_secs(epoch as u64))
                .expect("rotate through retention bound");
        }

        assert_eq!(
            responder.open_in_place(&mut old_packet[..old_len], now + Duration::from_secs(4)),
            Err(SessionError::UnknownEpoch)
        );
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
