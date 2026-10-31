//! Authenticated UDP control messages for the LXT SYN-ACK rescue path.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

pub const MAGIC: &[u8; 7] = b"LXTPUB4";
pub const MAX_UDP_DATAGRAM: usize = 4096;
pub const MAX_BODY_LEN: usize = 2000;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = MAGIC.len() + 1 + 8 + NONCE_LEN + 2;
const TIMESTAMP_WINDOW_SECS: u64 = 30;
const REPLAY_TTL: Duration = Duration::from_secs(60);
const MAX_REPLAY_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageKind {
    Hello = 1,
    HelloAck = 2,
    SynAck = 3,
}

impl TryFrom<u8> for MessageKind {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::HelloAck),
            3 => Ok(Self::SynAck),
            _ => Err(ProtocolError::UnknownMessageType(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: MessageKind,
    pub body: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("LXT UDP datagram exceeds {MAX_UDP_DATAGRAM} bytes")]
    DatagramTooLarge,
    #[error("LXT UDP datagram is truncated")]
    Truncated,
    #[error("LXT UDP datagram has an invalid magic value")]
    InvalidMagic,
    #[error("LXT UDP message body is too large")]
    BodyTooLarge,
    #[error("LXT UDP message type {0} is unknown")]
    UnknownMessageType(u8),
    #[error("LXT UDP message timestamp is outside the allowed window")]
    StaleTimestamp,
    #[error("LXT UDP message authentication failed")]
    InvalidTag,
    #[error("LXT UDP message nonce was replayed")]
    ReplayedNonce,
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
}

/// Owns the shared key and a bounded replay cache for one process.
pub struct Protocol {
    key: Zeroizing<Vec<u8>>,
    replay: ReplayCache,
}

impl Protocol {
    pub fn new(key: Zeroizing<Vec<u8>>) -> Self {
        Self {
            key,
            replay: ReplayCache::default(),
        }
    }

    pub fn encode(&self, kind: MessageKind, body: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let mut nonce = [0_u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| ProtocolError::RandomnessUnavailable)?;
        self.encode_at(kind, body, unix_time_now()?, &nonce)
    }

    pub fn decode(&mut self, datagram: &[u8]) -> Result<Message, ProtocolError> {
        self.decode_at(datagram, unix_time_now()?, Instant::now())
    }

    pub(crate) fn encode_at(
        &self,
        kind: MessageKind,
        body: &[u8],
        timestamp: u64,
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Vec<u8>, ProtocolError> {
        if body.len() > MAX_BODY_LEN {
            return Err(ProtocolError::BodyTooLarge);
        }

        let body_len = u16::try_from(body.len()).expect("bounded body length fits in u16");
        let mut datagram = Vec::with_capacity(HEADER_LEN + body.len() + TAG_LEN);
        datagram.extend_from_slice(MAGIC);
        datagram.push(kind as u8);
        datagram.extend_from_slice(&timestamp.to_be_bytes());
        datagram.extend_from_slice(nonce);
        datagram.extend_from_slice(&body_len.to_be_bytes());
        datagram.extend_from_slice(body);

        let mut mac =
            HmacSha256::new_from_slice(&self.key[..]).expect("HMAC accepts keys of any length");
        mac.update(&datagram);
        datagram.extend_from_slice(&mac.finalize().into_bytes()[..TAG_LEN]);
        Ok(datagram)
    }

    pub(crate) fn decode_at(
        &mut self,
        datagram: &[u8],
        timestamp_now: u64,
        monotonic_now: Instant,
    ) -> Result<Message, ProtocolError> {
        if datagram.len() > MAX_UDP_DATAGRAM {
            return Err(ProtocolError::DatagramTooLarge);
        }
        if datagram.len() < HEADER_LEN + TAG_LEN {
            return Err(ProtocolError::Truncated);
        }
        if &datagram[..MAGIC.len()] != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }

        let body_len =
            u16::from_be_bytes([datagram[HEADER_LEN - 2], datagram[HEADER_LEN - 1]]) as usize;
        if body_len > MAX_BODY_LEN || datagram.len() != HEADER_LEN + body_len + TAG_LEN {
            return Err(ProtocolError::Truncated);
        }

        let signed = &datagram[..datagram.len() - TAG_LEN];
        let tag = &datagram[datagram.len() - TAG_LEN..];
        let mut mac =
            HmacSha256::new_from_slice(&self.key[..]).expect("HMAC accepts keys of any length");
        mac.update(signed);
        mac.verify_truncated_left(tag)
            .map_err(|_| ProtocolError::InvalidTag)?;

        let timestamp = u64::from_be_bytes(
            datagram[MAGIC.len() + 1..MAGIC.len() + 9]
                .try_into()
                .expect("timestamp slice has a fixed length"),
        );
        if timestamp_now.abs_diff(timestamp) > TIMESTAMP_WINDOW_SECS {
            return Err(ProtocolError::StaleTimestamp);
        }

        let kind = MessageKind::try_from(datagram[MAGIC.len()])?;
        let nonce: [u8; NONCE_LEN] = datagram[MAGIC.len() + 9..MAGIC.len() + 21]
            .try_into()
            .expect("nonce slice has a fixed length");
        if !self.replay.insert(nonce, monotonic_now) {
            return Err(ProtocolError::ReplayedNonce);
        }

        let body_start = HEADER_LEN;
        let body_end = body_start + body_len;
        Ok(Message {
            kind,
            body: datagram[body_start..body_end].to_vec(),
        })
    }
}

fn unix_time_now() -> Result<u64, ProtocolError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ProtocolError::InvalidSystemClock)
}

#[derive(Default)]
struct ReplayCache {
    timestamps: HashMap<[u8; NONCE_LEN], Instant>,
    order: VecDeque<[u8; NONCE_LEN]>,
}

impl ReplayCache {
    fn insert(&mut self, nonce: [u8; NONCE_LEN], now: Instant) -> bool {
        self.prune(now);
        if self.timestamps.contains_key(&nonce) {
            return false;
        }

        self.timestamps.insert(nonce, now);
        self.order.push_back(nonce);
        while self.order.len() > MAX_REPLAY_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.timestamps.remove(&oldest);
            }
        }
        true
    }

    fn prune(&mut self, now: Instant) {
        while let Some(nonce) = self.order.front().copied() {
            let expired = self
                .timestamps
                .get(&nonce)
                .is_some_and(|seen| now.duration_since(*seen) > REPLAY_TTL);
            if !expired {
                break;
            }
            self.order.pop_front();
            self.timestamps.remove(&nonce);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const NONCE: [u8; NONCE_LEN] = [0x42; NONCE_LEN];

    fn protocol() -> Protocol {
        Protocol::new(Zeroizing::new(vec![0x7a; 32]))
    }

    fn hex_bytes(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    _ => panic!("fixed vector must contain lowercase hex"),
                };
                (digit(pair[0]) << 4) | digit(pair[1])
            })
            .collect()
    }

    #[test]
    fn fixed_vector_matches_experimental_protocol() {
        let sender = protocol();
        let datagram = sender
            .encode_at(MessageKind::SynAck, b"payload", NOW, &NONCE)
            .unwrap();
        let expected = hex_bytes(
            "4c58545055423403000000006553f10042424242424242424242424200077061796c6f6164daee9ae20b5b00c9dc3cc19e06d8c1ee",
        );
        assert_eq!(datagram, expected);
    }

    #[test]
    fn authenticated_messages_round_trip() {
        let sender = protocol();
        let mut receiver = protocol();
        let datagram = sender
            .encode_at(MessageKind::SynAck, b"payload", NOW, &NONCE)
            .unwrap();
        let message = receiver.decode_at(&datagram, NOW, Instant::now()).unwrap();
        assert_eq!(message.kind, MessageKind::SynAck);
        assert_eq!(message.body, b"payload");
    }

    #[test]
    fn tampered_messages_are_rejected() {
        let mut receiver = protocol();
        let mut datagram = receiver
            .encode_at(MessageKind::Hello, b"", NOW, &NONCE)
            .unwrap();
        let last = datagram.len() - 1;
        datagram[last] ^= 1;
        assert_eq!(
            receiver.decode_at(&datagram, NOW, Instant::now()),
            Err(ProtocolError::InvalidTag)
        );
    }

    #[test]
    fn stale_and_future_messages_are_rejected() {
        let sender = protocol();
        let mut receiver = protocol();
        for now in [NOW - 31, NOW + 31] {
            let datagram = sender
                .encode_at(MessageKind::Hello, b"", NOW, &NONCE)
                .unwrap();
            assert_eq!(
                receiver.decode_at(&datagram, now, Instant::now()),
                Err(ProtocolError::StaleTimestamp)
            );
        }
    }

    #[test]
    fn nonce_replay_is_rejected() {
        let sender = protocol();
        let mut receiver = protocol();
        let datagram = sender
            .encode_at(MessageKind::Hello, b"", NOW, &NONCE)
            .unwrap();
        receiver.decode_at(&datagram, NOW, Instant::now()).unwrap();
        assert_eq!(
            receiver.decode_at(&datagram, NOW, Instant::now()),
            Err(ProtocolError::ReplayedNonce)
        );
    }

    #[test]
    fn body_size_is_bounded() {
        let sender = protocol();
        assert_eq!(
            sender.encode_at(
                MessageKind::SynAck,
                &vec![0_u8; MAX_BODY_LEN + 1],
                NOW,
                &NONCE
            ),
            Err(ProtocolError::BodyTooLarge)
        );
    }

    #[test]
    fn truncated_and_oversized_datagrams_are_rejected() {
        let mut receiver = protocol();
        assert_eq!(
            receiver.decode_at(&[0_u8; 8], NOW, Instant::now()),
            Err(ProtocolError::Truncated)
        );
        assert_eq!(
            receiver.decode_at(&vec![0_u8; MAX_UDP_DATAGRAM + 1], NOW, Instant::now()),
            Err(ProtocolError::DatagramTooLarge)
        );
    }
}
