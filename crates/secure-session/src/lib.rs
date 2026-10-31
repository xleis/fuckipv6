//! Transport-independent primitives for the fuckipv6 secure session protocol v2.
//!
//! It contains the byte-exact data frame, nonce construction, replay window,
//! bounded epoch lifecycle, allocation-free AEAD packet operations, and the
//! online `REKEY_INIT`/`REKEY_ACCEPT`/`REKEY_CONFIRM` state machine. With the
//! optional `pq` feature, it also provides X-Wing draft-11, composite
//! Ed25519 + ML-DSA-65 identities, and an experimental hybrid
//! `HELLO/ACCEPT/CONFIRM` handshake with a transport-independent
//! [`driver::SessionDriver`] adapter. That hybrid profile is not connected to
//! the production control channel or `P2pEngine`, and its upstream PQ crates
//! are not audited.

pub mod confirmation;
pub mod cookie;
mod crypto_batch;
pub mod data_cipher;
pub mod driver;
pub mod handshake;
pub mod handshake_epoch;
#[cfg(feature = "pq")]
pub mod hybrid;
pub mod key_schedule;
#[cfg(feature = "pq")]
pub mod pq;
pub mod rekey;
pub mod rekey_codec;
pub mod rekey_session;
pub mod transcript;
pub mod wire;

pub use crypto_batch::{configure_crypto_workers, crypto_batch_metrics, CryptoBatchMetrics};
use data_cipher::AeadCipher;
pub use data_cipher::DataCipherSuite;
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

/// Safety margins used to schedule a rekey before an epoch is exhausted.
///
/// The active epoch is due for replacement when any remaining time, plaintext
/// byte, or packet-counter budget is less than or equal to its corresponding
/// margin. Callers should choose a time margin that leaves enough time to
/// complete the authenticated rekey exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyMargins {
    /// Time remaining before the hard epoch deadline.
    pub time: Duration,
    /// Plaintext bytes remaining before the hard per-epoch byte limit.
    pub bytes: u64,
    /// Outgoing packet counters remaining before counter exhaustion.
    pub packets: u64,
}

impl RekeyMargins {
    /// Zero margins schedule a rekey only once a hard limit has been reached.
    pub const ZERO: Self = Self {
        time: Duration::ZERO,
        bytes: 0,
        packets: 0,
    };

    /// Construct explicit time, byte, and packet-counter safety margins.
    pub const fn new(time: Duration, bytes: u64, packets: u64) -> Self {
        Self {
            time,
            bytes,
            packets,
        }
    }
}

impl Default for RekeyMargins {
    fn default() -> Self {
        Self::ZERO
    }
}

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
    suite: DataCipherSuite,
    cipher: AeadCipher,
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    epoch: u16,
    peer_receiver_index: u32,
    next_send_counter: u64,
}

impl SendCipher {
    /// Create ChaCha20-Poly1305 send state from a 32-byte derived epoch key.
    ///
    /// This compatibility constructor is equivalent to
    /// [`new_with_suite`](Self::new_with_suite) with
    /// [`DataCipherSuite::ChaCha20Poly1305`]. New handshake integrations must
    /// pass the suite negotiated by the authenticated handshake.
    ///
    /// `peer_receiver_index` must be the local receiver index advertised by the
    /// peer, not the sender's own index. The input key is zeroized after the
    /// AEAD state has been initialized.
    pub fn new(
        key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        peer_receiver_index: u32,
    ) -> Self {
        Self::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            key,
            session_id,
            direction,
            epoch,
            peer_receiver_index,
        )
    }

    /// Create send state for a negotiated data-cipher suite.
    ///
    /// The suite must come from the authenticated handshake; it is not encoded
    /// in or selected by an individual data packet.
    pub fn new_with_suite(
        suite: DataCipherSuite,
        mut key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        peer_receiver_index: u32,
    ) -> Self {
        let cipher = AeadCipher::new(suite, &key);
        key.zeroize();

        Self {
            suite,
            cipher,
            session_id,
            direction,
            epoch,
            peer_receiver_index,
            next_send_counter: 0,
        }
    }

    /// Negotiated data-cipher suite.
    pub const fn suite(&self) -> DataCipherSuite {
        self.suite
    }

    /// Next outgoing packet counter.
    pub const fn next_send_counter(&self) -> u64 {
        self.next_send_counter
    }

    pub(crate) const fn session_id(&self) -> [u8; SESSION_ID_LEN] {
        self.session_id
    }

    pub(crate) const fn epoch(&self) -> u16 {
        self.epoch
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
                &nonce,
                &aad,
                &mut packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext_len],
            )
            .map_err(|_| SessionError::AuthenticationFailed)?;

        packet[DATA_HEADER_LEN + plaintext_len..packet_len].copy_from_slice(&tag);
        self.next_send_counter += 1;
        Ok(())
    }

    /// Seal multiple independent plaintexts into one preallocated contiguous buffer.
    ///
    /// Packet `i` begins at `packets[i * stride]`, and each slot must be large
    /// enough for its header, plaintext and authentication tag. The method
    /// performs all layout and counter checks before writing any packet, so a
    /// layout error does not partially advance the send counter. The caller owns
    /// all input and output storage; this method performs no per-packet
    /// allocation.
    ///
    /// Returns the number of sealed packets.
    pub fn seal_batch(
        &mut self,
        plaintexts: &[&[u8]],
        packets: &mut [u8],
        stride: usize,
    ) -> Result<usize, SessionError> {
        if plaintexts.is_empty() {
            return Ok(0);
        }
        let packet_count =
            u64::try_from(plaintexts.len()).map_err(|_| SessionError::PacketTooLong)?;
        if self
            .next_send_counter
            .checked_add(packet_count)
            .is_none_or(|next| next > MAX_PACKETS_PER_EPOCH)
        {
            return Err(SessionError::CounterExhausted);
        }

        for (index, plaintext) in plaintexts.iter().enumerate() {
            let packet_len = data_packet_len(plaintext.len())?;
            if packet_len > stride {
                return Err(SessionError::BufferTooSmall);
            }
            let offset = index
                .checked_mul(stride)
                .ok_or(SessionError::PacketTooLong)?;
            let end = offset
                .checked_add(stride)
                .ok_or(SessionError::PacketTooLong)?;
            if end > packets.len() {
                return Err(SessionError::BufferTooSmall);
            }
        }

        let start_counter = self.next_send_counter;
        let batch_bytes = plaintexts
            .len()
            .checked_mul(stride)
            .ok_or(SessionError::PacketTooLong)?;
        let mut slots: Vec<&mut [u8]> = Vec::with_capacity(plaintexts.len());
        for (slot, plaintext) in packets[..batch_bytes]
            .chunks_exact_mut(stride)
            .zip(plaintexts)
        {
            slot[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext.len()].copy_from_slice(plaintext);
            slots.push(slot);
        }
        let plaintext_lens: Vec<usize> =
            plaintexts.iter().map(|plaintext| plaintext.len()).collect();
        seal_prepared_slots(
            &self.cipher,
            BatchSealContext {
                session_id: self.session_id,
                direction: self.direction,
                peer_receiver_index: self.peer_receiver_index,
                epoch: self.epoch,
                start_counter,
            },
            &mut slots,
            &plaintext_lens,
        )?;
        self.next_send_counter += plaintexts.len() as u64;
        Ok(plaintexts.len())
    }

    /// Seal packets whose plaintext already occupies each buffer at
    /// `DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext_len`.
    ///
    /// Validation and counter reservation complete before any buffer is
    /// modified. Counters are committed only after every AEAD operation
    /// succeeds, so an error cannot create a nonce gap or advance the sender.
    /// Large batches use the bounded process-wide crypto worker pool; small
    /// batches stay serial to avoid dispatch overhead.
    pub fn seal_batch_in_place(
        &mut self,
        packets: &mut [&mut [u8]],
        plaintext_lens: &[usize],
    ) -> Result<usize, SessionError> {
        let _next = validate_in_place_batch(packets, plaintext_lens, self.next_send_counter)?;
        let start_counter = self.next_send_counter;
        seal_prepared_slots(
            &self.cipher,
            BatchSealContext {
                session_id: self.session_id,
                direction: self.direction,
                peer_receiver_index: self.peer_receiver_index,
                epoch: self.epoch,
                start_counter,
            },
            packets,
            plaintext_lens,
        )?;
        self.next_send_counter += packets.len() as u64;
        Ok(packets.len())
    }
}

fn validate_in_place_batch(
    packets: &[&mut [u8]],
    plaintext_lens: &[usize],
    next_send_counter: u64,
) -> Result<u64, SessionError> {
    if packets.len() != plaintext_lens.len() {
        return Err(SessionError::BufferTooSmall);
    }
    let packet_count = u64::try_from(packets.len()).map_err(|_| SessionError::PacketTooLong)?;
    let next = next_send_counter
        .checked_add(packet_count)
        .filter(|next| *next <= MAX_PACKETS_PER_EPOCH)
        .ok_or(SessionError::CounterExhausted)?;
    for (packet, plaintext_len) in packets.iter().zip(plaintext_lens) {
        let required = data_packet_len(*plaintext_len)?;
        if packet.len() < required {
            return Err(SessionError::BufferTooSmall);
        }
    }
    Ok(next)
}

#[derive(Clone, Copy)]
struct BatchSealContext {
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    peer_receiver_index: u32,
    epoch: u16,
    start_counter: u64,
}

#[derive(Clone, Copy)]
struct BatchOpenContext {
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    epoch: u16,
    local_receiver_index: u32,
}

fn decrypt_prepared_packet(
    cipher: &AeadCipher,
    context: BatchOpenContext,
    packet: &mut [u8],
) -> Result<(), SessionError> {
    let header = DataHeader::decode(packet)?;
    if header.receiver_index != context.local_receiver_index {
        return Err(SessionError::ReceiverIndexMismatch);
    }
    if header.epoch != context.epoch {
        return Err(SessionError::EpochMismatch);
    }
    if header.counter >= MAX_PACKETS_PER_EPOCH {
        return Err(SessionError::CounterExhausted);
    }
    let plaintext_len = packet.len() - DATA_HEADER_LEN - AEAD_TAG_LEN;
    let aad = data_aad(header, context.session_id, plaintext_len as u16);
    let nonce = nonce_for(context.direction, header.counter);
    let (_, tail) = packet.split_at_mut(DATA_HEADER_LEN);
    let (plaintext, tag) = tail.split_at_mut(plaintext_len);
    let mut encoded_tag = [0u8; AEAD_TAG_LEN];
    encoded_tag.copy_from_slice(tag);
    cipher
        .decrypt_in_place_detached(&nonce, &aad, plaintext, &encoded_tag)
        .map_err(|_| SessionError::AuthenticationFailed)
}

fn decrypt_batch_serial(
    cipher: &AeadCipher,
    context: BatchOpenContext,
    packets: &mut [u8],
    packet_lengths: &[usize],
    stride: usize,
) -> Result<(), SessionError> {
    packets
        .chunks_mut(stride)
        .zip(packet_lengths)
        .try_for_each(|(packet, packet_len)| {
            decrypt_prepared_packet(cipher, context, &mut packet[..*packet_len])
        })
}

fn decrypt_batch_parallel(
    cipher: &AeadCipher,
    context: BatchOpenContext,
    packets: &mut [u8],
    packet_lengths: &[usize],
    stride: usize,
) -> Result<(), SessionError> {
    use rayon::prelude::*;

    packets
        .par_chunks_mut(stride)
        .zip(packet_lengths.par_iter())
        .try_for_each(|(packet, packet_len)| {
            decrypt_prepared_packet(cipher, context, &mut packet[..*packet_len])
        })
}

fn seal_prepared_slots(
    cipher: &AeadCipher,
    context: BatchSealContext,
    packets: &mut [&mut [u8]],
    plaintext_lens: &[usize],
) -> Result<(), SessionError> {
    use rayon::prelude::*;

    let plaintext_bytes = plaintext_lens
        .iter()
        .try_fold(0usize, |total, len| total.checked_add(*len))
        .ok_or(SessionError::PacketTooLong)?;
    let parallel = crate::crypto_batch::parallel_threshold_reached(packets.len(), plaintext_bytes);

    let seal_one = |(index, (packet, plaintext_len)): (usize, (&mut &mut [u8], &usize))| {
        let plaintext_len = *plaintext_len;
        let packet_len = data_packet_len(plaintext_len)?;
        let counter = context.start_counter + index as u64;
        let header = DataHeader::new(context.peer_receiver_index, context.epoch, counter);
        packet[..DATA_HEADER_LEN].copy_from_slice(&header.encode());
        let aad = data_aad(header, context.session_id, plaintext_len as u16);
        let nonce = nonce_for(context.direction, counter);
        let tag = cipher
            .encrypt_in_place_detached(
                &nonce,
                &aad,
                &mut packet[DATA_HEADER_LEN..DATA_HEADER_LEN + plaintext_len],
            )
            .map_err(|_| SessionError::AuthenticationFailed)?;
        packet[DATA_HEADER_LEN + plaintext_len..packet_len].copy_from_slice(&tag);
        Ok::<(), SessionError>(())
    };

    if parallel {
        let worker_result = crate::crypto_batch::try_run(|| {
            packets
                .par_iter_mut()
                .zip(plaintext_lens.par_iter())
                .enumerate()
                .try_for_each(seal_one)
        });
        if let Some(result) = worker_result {
            crate::crypto_batch::record_parallel(packets.len());
            return result;
        }
    }

    crate::crypto_batch::record_serial();
    packets
        .iter_mut()
        .zip(plaintext_lens)
        .enumerate()
        .try_for_each(seal_one)
}

/// Receiving state for one direction of a v2 session epoch.
///
/// Incoming packets are checked against `local_receiver_index` and a replay
/// window before AEAD verification. A counter is marked received only after
/// successful authentication. The underlying AEAD implementation zeroizes its
/// expanded key material when this value is dropped.
pub struct ReceiveCipher {
    suite: DataCipherSuite,
    cipher: AeadCipher,
    session_id: [u8; SESSION_ID_LEN],
    direction: Direction,
    epoch: u16,
    local_receiver_index: u32,
    replay_window: ReplayWindow,
}

impl ReceiveCipher {
    /// Create ChaCha20-Poly1305 receive state from a 32-byte derived epoch key.
    ///
    /// This compatibility constructor is equivalent to
    /// [`new_with_suite`](Self::new_with_suite) with
    /// [`DataCipherSuite::ChaCha20Poly1305`]. New handshake integrations must
    /// pass the suite negotiated by the authenticated handshake.
    ///
    /// `local_receiver_index` is the index assigned by this endpoint and must
    /// match the `receiver_index` field written by the peer. The input key is
    /// zeroized after the AEAD state has been initialized.
    pub fn new(
        key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        local_receiver_index: u32,
    ) -> Self {
        Self::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            key,
            session_id,
            direction,
            epoch,
            local_receiver_index,
        )
    }

    /// Create receive state for a negotiated data-cipher suite.
    ///
    /// The suite must come from the authenticated handshake; it is not encoded
    /// in or selected by an individual data packet.
    pub fn new_with_suite(
        suite: DataCipherSuite,
        mut key: [u8; 32],
        session_id: [u8; SESSION_ID_LEN],
        direction: Direction,
        epoch: u16,
        local_receiver_index: u32,
    ) -> Self {
        let cipher = AeadCipher::new(suite, &key);
        key.zeroize();

        Self {
            suite,
            cipher,
            session_id,
            direction,
            epoch,
            local_receiver_index,
            replay_window: ReplayWindow::new(),
        }
    }

    /// Negotiated data-cipher suite.
    pub const fn suite(&self) -> DataCipherSuite {
        self.suite
    }

    pub(crate) const fn session_id(&self) -> [u8; SESSION_ID_LEN] {
        self.session_id
    }

    pub(crate) const fn epoch(&self) -> u16 {
        self.epoch
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

        let mut encoded_tag = [0u8; AEAD_TAG_LEN];
        encoded_tag.copy_from_slice(tag);
        self.cipher
            .decrypt_in_place_detached(&nonce, &aad, plaintext, &encoded_tag)
            .map_err(|_| SessionError::AuthenticationFailed)?;

        self.replay_window.mark_authenticated(header.counter)?;
        Ok(plaintext)
    }

    /// Authenticate and decrypt multiple packets from one preallocated buffer.
    ///
    /// `packet_lengths[i]` is the exact datagram length at
    /// `packets[i * stride]`. The complete layout and replay windows are
    /// validated before any packet is decrypted. AEAD work may run in parallel;
    /// replay counters are committed in ascending counter order only after the
    /// complete batch authenticates, so worker completion order cannot reorder
    /// replay state and an out-of-order batch cannot expire its own members.
    /// On any error no packet in this batch consumes a replay counter, but
    /// successfully authenticated packets may already have been decrypted in
    /// place. The caller must discard or clear the complete output batch after
    /// every error; the valid prefix is not a stable API contract.
    ///
    /// Returns the total number of authenticated plaintext bytes.
    pub fn open_batch_in_place(
        &mut self,
        packets: &mut [u8],
        packet_lengths: &[usize],
        stride: usize,
    ) -> Result<usize, SessionError> {
        if packet_lengths.is_empty() {
            return Ok(0);
        }
        validate_batch_layout(packets.len(), packet_lengths, stride)?;

        let mut headers = Vec::with_capacity(packet_lengths.len());
        let mut plaintext_bytes = 0usize;
        for (index, packet_len) in packet_lengths.iter().copied().enumerate() {
            let offset = index
                .checked_mul(stride)
                .ok_or(SessionError::PacketTooLong)?;
            let packet = &packets[offset..offset + packet_len];
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
            plaintext_bytes = plaintext_bytes
                .checked_add(packet_len - DATA_HEADER_LEN - AEAD_TAG_LEN)
                .ok_or(SessionError::PacketTooLong)?;
            headers.push(header);
        }
        headers.sort_unstable_by_key(|header| header.counter);
        if headers
            .windows(2)
            .any(|pair| pair[0].counter == pair[1].counter)
        {
            return Err(SessionError::ReplayDetected);
        }

        let context = BatchOpenContext {
            session_id: self.session_id,
            direction: self.direction,
            epoch: self.epoch,
            local_receiver_index: self.local_receiver_index,
        };
        let parallel =
            crate::crypto_batch::parallel_threshold_reached(packet_lengths.len(), plaintext_bytes);
        if parallel {
            if let Some(result) = crate::crypto_batch::try_run(|| {
                decrypt_batch_parallel(&self.cipher, context, packets, packet_lengths, stride)
            }) {
                crate::crypto_batch::record_parallel(packet_lengths.len());
                result?;
            } else {
                crate::crypto_batch::record_serial();
                decrypt_batch_serial(&self.cipher, context, packets, packet_lengths, stride)?;
            }
        } else {
            crate::crypto_batch::record_serial();
            decrypt_batch_serial(&self.cipher, context, packets, packet_lengths, stride)?;
        }

        headers.sort_unstable_by_key(|header| header.counter);
        for header in headers {
            self.replay_window.mark_authenticated(header.counter)?;
        }
        Ok(plaintext_bytes)
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
/// Retired cipher state is dropped at the exact grace deadline, when the
/// retention bound is exceeded, or when the session is closed. The AEAD
/// constructors zeroize their input key material and their destructors clear
/// their private cipher state as provided by the underlying cipher crates.
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

    /// Negotiated data-cipher suite for the active epoch.
    pub const fn suite(&self) -> DataCipherSuite {
        self.active_send.suite
    }

    /// Encrypt a packet under the active epoch.
    pub fn seal(
        &mut self,
        plaintext: &[u8],
        packet: &mut [u8],
        now: Instant,
    ) -> Result<usize, SessionError> {
        self.prune_retired_receive(now);
        self.ensure_active_epoch_usable(plaintext.len(), now)?;
        let packet_len = self.active_send.seal(plaintext, packet)?;
        self.sent_bytes = self
            .sent_bytes
            .checked_add(plaintext.len() as u64)
            .ok_or(SessionError::RekeyRequired)?;
        Ok(packet_len)
    }

    /// Encrypt a plaintext already stored at `packet[DATA_HEADER_LEN..]` under
    /// the active epoch.
    ///
    /// This is the zero-copy outbound counterpart to [`Self::seal`]. The caller
    /// must reserve `DATA_HEADER_LEN` bytes before the plaintext and enough
    /// space for the authentication tag. The epoch byte and time limits are
    /// charged before the packet is modified.
    pub fn seal_in_place(
        &mut self,
        packet: &mut [u8],
        plaintext_len: usize,
        now: Instant,
    ) -> Result<usize, SessionError> {
        self.prune_retired_receive(now);
        self.ensure_active_epoch_usable(plaintext_len, now)?;
        let packet_len = data_packet_len(plaintext_len)?;
        self.active_send.seal_in_place(packet, plaintext_len)?;
        self.sent_bytes = self
            .sent_bytes
            .checked_add(plaintext_len as u64)
            .ok_or(SessionError::RekeyRequired)?;
        Ok(packet_len)
    }

    /// Seal multiple packets into one preallocated contiguous buffer.
    ///
    /// All plaintext lengths are charged to the active epoch before any packet
    /// is written. The buffer layout is identical to
    /// [`SendCipher::seal_batch`]; no per-packet allocation is performed.
    pub fn seal_batch(
        &mut self,
        plaintexts: &[&[u8]],
        packets: &mut [u8],
        stride: usize,
        now: Instant,
    ) -> Result<usize, SessionError> {
        self.prune_retired_receive(now);
        let plaintext_bytes = plaintexts.iter().try_fold(0u64, |total, plaintext| {
            total.checked_add(plaintext.len() as u64)
        });
        let plaintext_bytes = plaintext_bytes.ok_or(SessionError::RekeyRequired)?;
        self.ensure_active_epoch_bytes(plaintext_bytes, now)?;
        let sealed = self.active_send.seal_batch(plaintexts, packets, stride)?;
        self.sent_bytes = self
            .sent_bytes
            .checked_add(plaintext_bytes)
            .ok_or(SessionError::RekeyRequired)?;
        Ok(sealed)
    }

    /// Encrypt a batch whose plaintexts already occupy their packet buffers.
    ///
    /// Validation and epoch byte accounting succeed before the output buffers
    /// are modified. The active sender commits all counters only after every
    /// AEAD operation succeeds.
    pub fn seal_batch_in_place(
        &mut self,
        packets: &mut [&mut [u8]],
        plaintext_lens: &[usize],
        now: Instant,
    ) -> Result<usize, SessionError> {
        self.prune_retired_receive(now);
        let plaintext_bytes = plaintext_lens
            .iter()
            .try_fold(0u64, |total, len| total.checked_add(*len as u64));
        let plaintext_bytes = plaintext_bytes.ok_or(SessionError::RekeyRequired)?;
        self.ensure_active_epoch_bytes(plaintext_bytes, now)?;
        let sealed = self
            .active_send
            .seal_batch_in_place(packets, plaintext_lens)?;
        self.sent_bytes = self
            .sent_bytes
            .checked_add(plaintext_bytes)
            .ok_or(SessionError::RekeyRequired)?;
        Ok(sealed)
    }

    /// Authenticate and decrypt a batch, routing each packet to its epoch.
    ///
    /// The complete buffer layout is validated first. Packets are opened
    /// serially and fail-fast, so replay counters for a successfully opened
    /// prefix remain committed if a later packet fails. This differs from
    /// [`ReceiveCipher::open_batch_in_place`], which commits no replay counters
    /// unless the complete batch authenticates. No per-packet allocation is
    /// performed.
    pub fn open_batch_in_place(
        &mut self,
        packets: &mut [u8],
        packet_lengths: &[usize],
        stride: usize,
        now: Instant,
    ) -> Result<usize, SessionError> {
        validate_batch_layout(packets.len(), packet_lengths, stride)?;
        self.prune_retired_receive(now);
        let mut plaintext_bytes = 0usize;
        for (index, packet_len) in packet_lengths.iter().copied().enumerate() {
            let offset = index
                .checked_mul(stride)
                .ok_or(SessionError::PacketTooLong)?;
            let plaintext = self.open_in_place(&mut packets[offset..offset + packet_len], now)?;
            plaintext_bytes = plaintext_bytes
                .checked_add(plaintext.len())
                .ok_or(SessionError::PacketTooLong)?;
        }
        Ok(plaintext_bytes)
    }

    /// Authenticate and decrypt a packet under the active or a retained epoch.
    pub fn open_in_place<'a>(
        &mut self,
        packet: &'a mut [u8],
        now: Instant,
    ) -> Result<&'a mut [u8], SessionError> {
        self.prune_retired_receive(now);
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
    ///
    /// This fails closed at the exact time, plaintext-byte, or outgoing packet
    /// counter limit. Receive processing remains available for the active and
    /// retained receive epochs.
    pub fn rekey_required(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.active_started_at) >= MAX_EPOCH_LIFETIME
            || self.sent_bytes >= MAX_EPOCH_BYTES
            || self.active_send.next_send_counter() >= MAX_PACKETS_PER_EPOCH
    }

    /// Whether the active epoch is within any pre-expiry safety margin.
    ///
    /// This is a scheduling signal, not permission to keep sending. The data
    /// path still fails closed independently at the hard limits enforced by
    /// [`rekey_required`](Self::rekey_required).
    pub fn rekey_due(&self, now: Instant, margins: RekeyMargins) -> bool {
        let elapsed = now.saturating_duration_since(self.active_started_at);
        let remaining_time = MAX_EPOCH_LIFETIME.saturating_sub(elapsed);
        let remaining_bytes = MAX_EPOCH_BYTES.saturating_sub(self.sent_bytes);
        let remaining_packets =
            MAX_PACKETS_PER_EPOCH.saturating_sub(self.active_send.next_send_counter());

        remaining_time <= margins.time
            || remaining_bytes <= margins.bytes
            || remaining_packets <= margins.packets
    }

    /// Install a new epoch and retain the previous receive cipher for a short grace period.
    pub fn rotate(
        &mut self,
        send: SendCipher,
        receive: ReceiveCipher,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.validate_rotation(&send, &receive)?;
        self.prune_retired_receive(now);

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

    pub(crate) fn validate_rotation(
        &self,
        send: &SendCipher,
        receive: &ReceiveCipher,
    ) -> Result<(), SessionError> {
        validate_epoch_pair(send, receive)?;
        if send.suite() != self.suite() || receive.suite() != self.suite() {
            return Err(SessionError::InvalidEpochState);
        }
        if receive.epoch <= self.active_receive.epoch {
            return Err(SessionError::EpochRollback);
        }
        Ok(())
    }

    fn ensure_active_epoch_usable(
        &self,
        plaintext_len: usize,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.ensure_active_epoch_bytes(plaintext_len as u64, now)
    }

    fn ensure_active_epoch_bytes(
        &self,
        plaintext_len: u64,
        now: Instant,
    ) -> Result<(), SessionError> {
        if self.rekey_required(now) {
            return Err(SessionError::RekeyRequired);
        }
        let next_bytes = self
            .sent_bytes
            .checked_add(plaintext_len)
            .ok_or(SessionError::RekeyRequired)?;
        if next_bytes > MAX_EPOCH_BYTES {
            return Err(SessionError::RekeyRequired);
        }
        Ok(())
    }

    /// Drop receive epochs whose exact grace deadline has been reached.
    ///
    /// Returns the number of retained epochs removed. The receive cipher and
    /// its AEAD state are dropped before this method returns. This can be
    /// called from a maintenance tick even when no packets are arriving.
    pub fn prune_retired_receive(&mut self, now: Instant) -> usize {
        let mut removed = 0;
        for entry in &mut self.retired_receive {
            if entry
                .as_ref()
                .is_some_and(|retired| now >= retired.discard_at)
            {
                *entry = None;
                removed += 1;
            }
        }
        removed
    }

    /// Number of receive epochs currently retained for in-flight packets.
    pub fn retired_receive_epoch_count(&self) -> usize {
        self.retired_receive
            .iter()
            .filter(|entry| entry.is_some())
            .count()
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
    if !opposite_directions
        || send.session_id != receive.session_id
        || send.epoch != receive.epoch
        || send.suite != receive.suite
    {
        return Err(SessionError::InvalidEpochState);
    }
    Ok(())
}

fn receive_matches(receive: &ReceiveCipher, header: DataHeader) -> bool {
    receive.epoch == header.epoch && receive.local_receiver_index == header.receiver_index
}

fn validate_batch_layout(
    buffer_len: usize,
    packet_lengths: &[usize],
    stride: usize,
) -> Result<(), SessionError> {
    for (index, packet_len) in packet_lengths.iter().copied().enumerate() {
        if packet_len < MIN_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooShort);
        }
        if packet_len > MAX_DATA_PACKET_LEN {
            return Err(SessionError::PacketTooLong);
        }
        if packet_len > stride {
            return Err(SessionError::BufferTooSmall);
        }
        let offset = index
            .checked_mul(stride)
            .ok_or(SessionError::PacketTooLong)?;
        let end = offset
            .checked_add(packet_len)
            .ok_or(SessionError::PacketTooLong)?;
        if end > buffer_len {
            return Err(SessionError::BufferTooSmall);
        }
    }
    Ok(())
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
    #[error("unsupported v2 data-cipher suite {0:#06x}")]
    UnsupportedCipherSuite(u16),
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
    fn both_data_suites_share_framing_and_round_trip() {
        for suite in [
            DataCipherSuite::Aes256Gcm,
            DataCipherSuite::ChaCha20Poly1305,
        ] {
            let mut sender = SendCipher::new_with_suite(
                suite,
                TEST_KEY,
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                17,
                0x0102_0304,
            );
            let mut receiver = ReceiveCipher::new_with_suite(
                suite,
                TEST_KEY,
                TEST_SESSION_ID,
                Direction::InitiatorToResponder,
                17,
                0x0102_0304,
            );
            assert_eq!(sender.suite(), suite);
            assert_eq!(receiver.suite(), suite);

            let mut packet = [0u8; 128];
            let packet_len = sender
                .seal(b"suite-neutral data frame", &mut packet)
                .expect("seal with negotiated suite");
            assert_eq!(
                packet_len,
                DATA_HEADER_LEN + b"suite-neutral data frame".len() + AEAD_TAG_LEN
            );
            assert_eq!(
                DataHeader::decode(&packet).expect("decode shared header"),
                DataHeader::new(0x0102_0304, 17, 0)
            );
            assert_eq!(
                receiver
                    .open_in_place(&mut packet[..packet_len])
                    .expect("open with negotiated suite"),
                b"suite-neutral data frame"
            );
            assert_eq!(
                receiver
                    .open_in_place(&mut packet[..packet_len])
                    .expect_err("replay must fail"),
                SessionError::ReplayDetected
            );
        }
    }

    #[test]
    fn suite_mismatch_tampering_and_reflection_fail_without_consuming_counters() {
        let mut sender = SendCipher::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut packet = [0u8; 128];
        let packet_len = sender
            .seal(b"authenticated bytes", &mut packet)
            .expect("seal");
        let original = packet;

        let mut wrong_suite = ReceiveCipher::new_with_suite(
            DataCipherSuite::Aes256Gcm,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut cross_suite_packet = original;
        assert_eq!(
            wrong_suite
                .open_in_place(&mut cross_suite_packet[..packet_len])
                .expect_err("cross-suite packet must fail"),
            SessionError::AuthenticationFailed
        );

        let mut receiver = ReceiveCipher::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut tampered_header = original;
        tampered_header[15] ^= 1;
        assert_eq!(
            receiver
                .open_in_place(&mut tampered_header[..packet_len])
                .expect_err("header tampering must fail"),
            SessionError::AuthenticationFailed
        );

        let mut tampered_tag = original;
        tampered_tag[packet_len - 1] ^= 1;
        assert_eq!(
            receiver
                .open_in_place(&mut tampered_tag[..packet_len])
                .expect_err("tag tampering must fail"),
            SessionError::AuthenticationFailed
        );

        let mut valid = original;
        assert_eq!(
            receiver
                .open_in_place(&mut valid[..packet_len])
                .expect("failed attempts must not consume the counter"),
            b"authenticated bytes"
        );

        let mut reflected = [0u8; 128];
        let reflected_len = sender
            .seal(b"direction reflection", &mut reflected)
            .expect("seal second packet");
        let mut wrong_direction = ReceiveCipher::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::ResponderToInitiator,
            17,
            0x0102_0304,
        );
        assert_eq!(
            wrong_direction
                .open_in_place(&mut reflected[..reflected_len])
                .expect_err("direction reflection must fail"),
            SessionError::AuthenticationFailed
        );
    }

    #[test]
    fn batch_cipher_paths_use_one_preallocated_contiguous_buffer() {
        let plaintexts: [&[u8]; 3] = [b"alpha", b"batch-payload", b"omega"];
        let stride = 64usize;
        let mut packets = vec![0u8; stride * plaintexts.len()];
        let mut sender = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        assert_eq!(
            sender
                .seal_batch(&plaintexts, &mut packets, stride)
                .expect("seal contiguous batch"),
            plaintexts.len()
        );
        assert_eq!(sender.next_send_counter(), plaintexts.len() as u64);

        let packet_lengths: Vec<usize> = plaintexts
            .iter()
            .map(|plaintext| DATA_HEADER_LEN + plaintext.len() + AEAD_TAG_LEN)
            .collect();
        for (index, packet_len) in packet_lengths.iter().copied().enumerate() {
            let offset = index * stride;
            let header = DataHeader::decode(&packets[offset..offset + packet_len])
                .expect("decode batch packet");
            assert_eq!(header.counter, index as u64);
            assert_eq!(header.receiver_index, 0x0102_0304);
        }

        let mut receiver = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        assert_eq!(
            receiver
                .open_batch_in_place(&mut packets, &packet_lengths, stride)
                .expect("open contiguous batch"),
            plaintexts.iter().map(|plaintext| plaintext.len()).sum()
        );
        for (index, expected) in plaintexts.iter().enumerate() {
            let offset = index * stride + DATA_HEADER_LEN;
            assert_eq!(&packets[offset..offset + expected.len()], *expected);
        }
    }

    #[test]
    fn in_place_batch_uses_worker_pool_and_preserves_nonce_order() {
        let mut sender = SendCipher::new_with_suite(
            DataCipherSuite::Aes256Gcm,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let payloads: Vec<Vec<u8>> = (0..32).map(|index| vec![index as u8; 1420]).collect();
        let mut packets: Vec<Vec<u8>> = payloads
            .iter()
            .map(|payload| {
                let mut packet = vec![0u8; DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN];
                packet[DATA_HEADER_LEN..DATA_HEADER_LEN + payload.len()].copy_from_slice(payload);
                packet
            })
            .collect();
        let mut slots: Vec<&mut [u8]> = packets.iter_mut().map(Vec::as_mut_slice).collect();
        let lens: Vec<usize> = payloads.iter().map(Vec::len).collect();

        assert_eq!(
            sender
                .seal_batch_in_place(&mut slots, &lens)
                .expect("parallel in-place seal"),
            payloads.len()
        );
        assert_eq!(sender.next_send_counter(), payloads.len() as u64);
        drop(slots);

        let metrics = crate::crypto_batch_metrics();
        assert!(metrics.worker_threads > 0);
        assert!(metrics.parallel_packets >= payloads.len() as u64);

        let mut receiver = ReceiveCipher::new_with_suite(
            DataCipherSuite::Aes256Gcm,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        for (index, packet) in packets.iter_mut().enumerate() {
            let header = DataHeader::decode(packet).expect("decode ordered packet");
            assert_eq!(header.counter, index as u64);
            let plaintext = receiver.open_in_place(packet).expect("open ordered packet");
            assert_eq!(plaintext, payloads[index]);
        }
    }

    fn seal_contiguous_batch(payloads: &[&[u8]], stride: usize) -> (Vec<u8>, Vec<usize>) {
        let mut sender = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut packets = vec![0u8; stride * payloads.len()];
        assert_eq!(
            sender
                .seal_batch(payloads, &mut packets, stride)
                .expect("seal test batch"),
            payloads.len()
        );
        let lengths = payloads
            .iter()
            .map(|payload| DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN)
            .collect();
        (packets, lengths)
    }

    fn receive_batch_cipher() -> ReceiveCipher {
        ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        )
    }

    #[test]
    fn parallel_batch_open_marks_every_counter_for_replay() {
        let payload = vec![0x5a; 1420];
        let payloads = vec![payload.as_slice(); 32];
        let stride = DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN;
        let (mut packets, lengths) = seal_contiguous_batch(&payloads, stride);
        let mut receiver = receive_batch_cipher();
        let before = crate::crypto_batch_metrics();

        assert_eq!(
            receiver
                .open_batch_in_place(&mut packets, &lengths, stride)
                .expect("parallel batch open"),
            payloads.len() * payload.len()
        );

        let after = crate::crypto_batch_metrics();
        assert!(after.parallel_jobs > before.parallel_jobs);
        assert!(
            after
                .parallel_packets
                .saturating_sub(before.parallel_packets)
                >= payloads.len() as u64
        );
        for (index, packet_len) in lengths.iter().copied().enumerate() {
            let offset = index * stride;
            let header = DataHeader::decode(&packets[offset..offset + packet_len])
                .expect("decode authenticated batch header");
            assert_eq!(
                receiver.replay_window.check(header.counter),
                Err(SessionError::ReplayDetected)
            );
        }
        assert_eq!(
            receiver.open_batch_in_place(&mut packets, &lengths, stride),
            Err(SessionError::ReplayDetected)
        );
    }

    #[test]
    fn batch_open_authentication_failure_commits_no_replay_counter() {
        let payload = vec![0x31; 1420];
        let payloads = vec![payload.as_slice(); 8];
        let stride = DATA_HEADER_LEN + payload.len() + AEAD_TAG_LEN;
        let (mut tampered, lengths) = seal_contiguous_batch(&payloads, stride);
        let clean = tampered.clone();
        tampered[stride + lengths[1] - 1] ^= 1;
        let mut receiver = receive_batch_cipher();

        assert_eq!(
            receiver.open_batch_in_place(&mut tampered, &lengths, stride),
            Err(SessionError::AuthenticationFailed)
        );
        let mut retry = clean;
        assert_eq!(
            receiver
                .open_batch_in_place(&mut retry, &lengths, stride)
                .expect("failed batch must not consume replay state"),
            payloads.len() * payload.len()
        );
    }

    #[test]
    fn batch_open_rejects_duplicate_counter_before_decrypting() {
        let payloads: [&[u8]; 2] = [b"duplicate-a", b"duplicate-b"];
        let stride = 64usize;
        let (source, lengths) = seal_contiguous_batch(&payloads, stride);
        let mut duplicated = vec![0u8; stride * 3];
        duplicated[..stride].copy_from_slice(&source[..stride]);
        duplicated[stride..stride * 2].copy_from_slice(&source[..stride]);
        duplicated[stride * 2..stride * 3].copy_from_slice(&source[stride..stride * 2]);
        let duplicate_lengths = [lengths[0], lengths[0], lengths[1]];
        let mut receiver = receive_batch_cipher();

        assert_eq!(
            receiver.open_batch_in_place(&mut duplicated, &duplicate_lengths, stride),
            Err(SessionError::ReplayDetected)
        );
        let mut clean = source;
        assert_eq!(
            receiver
                .open_batch_in_place(&mut clean, &lengths, stride)
                .expect("duplicate preflight must not consume counters"),
            payloads.iter().map(|payload| payload.len()).sum()
        );
    }

    #[test]
    fn batch_open_commits_out_of_order_counters_without_self_expiry() {
        let payloads: [&[u8]; 4] = [
            b"counter-zero",
            b"counter-one",
            b"counter-two",
            b"counter-three",
        ];
        let stride = 64usize;
        let (source, lengths) = seal_contiguous_batch(&payloads, stride);
        let order = [3usize, 0, 2, 1];
        let mut reordered = vec![0u8; stride * order.len()];
        for (destination, source_index) in order.into_iter().enumerate() {
            reordered[destination * stride..(destination + 1) * stride]
                .copy_from_slice(&source[source_index * stride..(source_index + 1) * stride]);
        }
        let reordered_lengths: Vec<usize> = order.iter().map(|index| lengths[*index]).collect();
        let mut receiver = receive_batch_cipher();

        assert_eq!(
            receiver
                .open_batch_in_place(&mut reordered, &reordered_lengths, stride)
                .expect("out-of-order batch open"),
            payloads.iter().map(|payload| payload.len()).sum()
        );
        for (destination, source_index) in order.into_iter().enumerate() {
            let offset = destination * stride + DATA_HEADER_LEN;
            assert_eq!(
                &reordered[offset..offset + payloads[source_index].len()],
                payloads[source_index]
            );
        }
    }

    #[test]
    fn batch_open_rejects_expired_and_exhausted_counters_before_decrypt() {
        let mut sender = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let stride = 64usize;
        let mut packets = vec![0u8; stride * 2];
        let old_len = sender
            .seal(b"old", &mut packets[..stride])
            .expect("seal old packet");
        sender.next_send_counter = REPLAY_WINDOW_SIZE + 1;
        let new_len = sender
            .seal(b"new", &mut packets[stride..stride * 2])
            .expect("seal new packet");
        let mut receiver = receive_batch_cipher();
        receiver
            .replay_window
            .mark_authenticated(REPLAY_WINDOW_SIZE)
            .expect("advance replay window");

        assert_eq!(
            receiver.open_batch_in_place(&mut packets, &[old_len, new_len], stride),
            Err(SessionError::CounterTooOld)
        );
        assert_eq!(
            receiver
                .open_in_place(&mut packets[stride..stride + new_len])
                .expect("expired batch must leave newer counter untouched"),
            b"new"
        );

        let mut exhausted = vec![0u8; MIN_DATA_PACKET_LEN];
        exhausted[..DATA_HEADER_LEN]
            .copy_from_slice(&DataHeader::new(0x0102_0304, 17, MAX_PACKETS_PER_EPOCH).encode());
        let mut receiver = receive_batch_cipher();
        assert_eq!(
            receiver.open_batch_in_place(&mut exhausted, &[MIN_DATA_PACKET_LEN], stride),
            Err(SessionError::CounterExhausted)
        );
    }

    #[test]
    fn in_place_batch_layout_error_does_not_advance_counter() {
        let mut sender = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut first = vec![0u8; MIN_DATA_PACKET_LEN];
        let mut too_short = vec![0u8; MIN_DATA_PACKET_LEN - 1];
        let mut slots: Vec<&mut [u8]> = vec![&mut first, &mut too_short];
        assert_eq!(
            sender.seal_batch_in_place(&mut slots, &[0, 0]),
            Err(SessionError::BufferTooSmall)
        );
        assert_eq!(sender.next_send_counter(), 0);
    }

    #[test]
    fn batch_open_accepts_compact_trailing_slot_without_panicking() {
        let payloads: [&[u8]; 2] = [b"first", b"second"];
        let stride = 64usize;
        let (source, lengths) = seal_contiguous_batch(&payloads, stride);
        let compact_len = stride + lengths[1];
        let mut compact = source[..compact_len].to_vec();
        let mut receiver = receive_batch_cipher();

        assert_eq!(
            receiver
                .open_batch_in_place(&mut compact, &lengths, stride)
                .expect("compact trailing slot"),
            payloads.iter().map(|payload| payload.len()).sum()
        );
    }

    #[test]
    fn batch_layout_failure_does_not_advance_or_decrypt_partial_prefix() {
        let mut sender = SendCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let mut too_narrow = [0u8; 64];
        assert_eq!(
            sender
                .seal_batch(&[b"payload that does not fit"], &mut too_narrow, 16)
                .expect_err("stride must hold every packet"),
            SessionError::BufferTooSmall
        );
        assert_eq!(sender.next_send_counter(), 0);

        let mut packet = [0u8; 64];
        let packet_len = sender
            .seal(b"one packet", &mut packet)
            .expect("seal fixture");
        let mut receiver = ReceiveCipher::new(
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0x0102_0304,
        );
        let original = packet;
        assert_eq!(
            receiver
                .open_batch_in_place(&mut packet, &[packet_len, packet_len], packet_len)
                .expect_err("declared second packet exceeds buffer"),
            SessionError::BufferTooSmall
        );
        assert_eq!(packet, original);
    }

    #[test]
    fn epoch_batch_round_trip_routes_all_packets_across_directions() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(17, 0, now);
        let mut responder = responder_epoch(17, 0, now);
        let plaintexts: [&[u8]; 4] = [b"one", b"two", b"three", b"four"];
        let stride = 64usize;
        let mut packets = vec![0u8; stride * plaintexts.len()];
        assert_eq!(
            initiator
                .seal_batch(&plaintexts, &mut packets, stride, now)
                .expect("epoch seal batch"),
            plaintexts.len()
        );
        let packet_lengths: Vec<usize> = plaintexts
            .iter()
            .map(|plaintext| DATA_HEADER_LEN + plaintext.len() + AEAD_TAG_LEN)
            .collect();
        assert_eq!(
            responder
                .open_batch_in_place(&mut packets, &packet_lengths, stride, now)
                .expect("epoch open batch"),
            plaintexts.iter().map(|plaintext| plaintext.len()).sum()
        );
        for (index, expected) in plaintexts.iter().enumerate() {
            let offset = index * stride + DATA_HEADER_LEN;
            assert_eq!(&packets[offset..offset + expected.len()], *expected);
        }
    }

    #[test]
    fn mixed_suite_epoch_pairs_are_rejected() {
        let send = SendCipher::new_with_suite(
            DataCipherSuite::ChaCha20Poly1305,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            17,
            0xa1a2_a3a4,
        );
        let receive = ReceiveCipher::new_with_suite(
            DataCipherSuite::Aes256Gcm,
            TEST_KEY,
            TEST_SESSION_ID,
            Direction::ResponderToInitiator,
            17,
            0xb1b2_b3b4,
        );
        assert_eq!(
            EpochSession::new(send, receive, Instant::now()).err(),
            Some(SessionError::InvalidEpochState)
        );
        assert_eq!(DataCipherSuite::from_id(0x9999), None);
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
    fn epoch_session_seals_a_reserved_packet_buffer_in_place() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(1, 0, now);
        let mut responder = responder_epoch(1, 0, now);
        let mut packet = [0u8; 128];
        packet[DATA_HEADER_LEN..DATA_HEADER_LEN + 7].copy_from_slice(b"request");
        let allocation = packet.as_ptr();

        let packet_len = initiator
            .seal_in_place(&mut packet, 7, now)
            .expect("seal reserved request buffer");

        assert_eq!(packet.as_ptr(), allocation);
        assert_eq!(initiator.sent_bytes, 7);
        assert_eq!(
            DataHeader::decode(&packet[..packet_len])
                .expect("decode reserved packet")
                .counter,
            0
        );
        assert_eq!(
            responder
                .open_in_place(&mut packet[..packet_len], now)
                .expect("open in-place request"),
            b"request"
        );

        let mut short = [0u8; DATA_HEADER_LEN + 6 + AEAD_TAG_LEN];
        assert_eq!(
            initiator.seal_in_place(&mut short, 7, now),
            Err(SessionError::BufferTooSmall)
        );
        assert_eq!(initiator.sent_bytes, 7);
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

        session.sent_bytes = 0;
        session.active_send.next_send_counter = MAX_PACKETS_PER_EPOCH;
        assert!(session.rekey_required(now));
        assert_eq!(
            session.seal(b"counter exhausted", &mut packet, now),
            Err(SessionError::RekeyRequired)
        );
    }

    #[test]
    fn epoch_session_schedules_rekey_at_time_byte_and_counter_margins() {
        let now = Instant::now();
        let mut session = initiator_epoch(1, 0, now);

        let time_margin = Duration::from_secs(10);
        let margins = RekeyMargins::new(time_margin, 0, 0);
        assert!(!session.rekey_due(
            now + MAX_EPOCH_LIFETIME - time_margin - Duration::from_nanos(1),
            margins
        ));
        assert!(session.rekey_due(now + MAX_EPOCH_LIFETIME - time_margin, margins));

        let byte_margin = 4096;
        let margins = RekeyMargins::new(Duration::ZERO, byte_margin, 0);
        session.sent_bytes = MAX_EPOCH_BYTES - byte_margin - 1;
        assert!(!session.rekey_due(now, margins));
        session.sent_bytes = MAX_EPOCH_BYTES - byte_margin;
        assert!(session.rekey_due(now, margins));

        let packet_margin = 64;
        let margins = RekeyMargins::new(Duration::ZERO, 0, packet_margin);
        session.sent_bytes = 0;
        session.active_send.next_send_counter = MAX_PACKETS_PER_EPOCH - packet_margin - 1;
        assert!(!session.rekey_due(now, margins));
        session.active_send.next_send_counter = MAX_PACKETS_PER_EPOCH - packet_margin;
        assert!(session.rekey_due(now, margins));

        assert!(!session.rekey_due(now, RekeyMargins::ZERO));
        session.active_send.next_send_counter = MAX_PACKETS_PER_EPOCH;
        assert!(session.rekey_due(now, RekeyMargins::ZERO));
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
        assert_eq!(responder.retired_receive_epoch_count(), 1);
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
        assert_eq!(responder.retired_receive_epoch_count(), 0);

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
    fn retired_receive_is_pruned_at_the_exact_grace_deadline_without_traffic() {
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
        let (send, receive) = responder_ciphers(2, 1);
        responder
            .rotate(send, receive, rotate_at)
            .expect("rotate responder");
        assert_eq!(responder.retired_receive_epoch_count(), 1);

        let before_deadline = rotate_at + RETIRED_EPOCH_GRACE - Duration::from_nanos(1);
        assert_eq!(responder.prune_retired_receive(before_deadline), 0);
        assert_eq!(responder.retired_receive_epoch_count(), 1);
        assert_eq!(
            responder
                .open_in_place(&mut old_first[..first_len], before_deadline)
                .expect("old epoch remains valid before deadline"),
            b"old first"
        );

        let deadline = rotate_at + RETIRED_EPOCH_GRACE;
        assert_eq!(responder.prune_retired_receive(deadline), 1);
        assert_eq!(responder.retired_receive_epoch_count(), 0);
        assert_eq!(
            responder.open_in_place(&mut old_second[..second_len], deadline),
            Err(SessionError::UnknownEpoch)
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
    fn epoch_session_rejects_a_matched_cipher_suite_change() {
        let now = Instant::now();
        let mut initiator = initiator_epoch(1, 0, now);
        let mut responder = responder_epoch(1, 0, now);
        let mut old_packet = [0u8; 128];
        let old_len = initiator
            .seal(b"active suite", &mut old_packet, now)
            .expect("seal active-suite packet");

        let suite = DataCipherSuite::Aes256Gcm;
        let send = SendCipher::new_with_suite(
            suite,
            [0x21; 32],
            TEST_SESSION_ID,
            Direction::ResponderToInitiator,
            2,
            0xb1b2_b3b4,
        );
        let receive = ReceiveCipher::new_with_suite(
            suite,
            [0x11; 32],
            TEST_SESSION_ID,
            Direction::InitiatorToResponder,
            2,
            0xa1a2_a3a4,
        );
        assert_eq!(send.suite(), receive.suite());

        assert_eq!(
            responder.rotate(send, receive, now + Duration::from_secs(1)),
            Err(SessionError::InvalidEpochState)
        );
        assert_eq!(responder.suite(), DataCipherSuite::ChaCha20Poly1305);
        assert_eq!(responder.epoch(), 1);
        assert_eq!(responder.retired_receive_epoch_count(), 0);
        assert_eq!(
            responder
                .open_in_place(&mut old_packet[..old_len], now + Duration::from_secs(1))
                .expect("active receive remains usable after suite downgrade attempt"),
            b"active suite"
        );

        let (send, receive) = responder_ciphers(2, 1);
        responder
            .rotate(send, receive, now + Duration::from_secs(2))
            .expect("matching negotiated suite rotates");
        assert_eq!(responder.suite(), DataCipherSuite::ChaCha20Poly1305);
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
            responder.retired_receive_epoch_count(),
            MAX_RETIRED_RECEIVE_EPOCHS
        );

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
