//! Bounded ownership-preserving buffers for the UDP receive hot path.
//!
//! A receive buffer is borrowed for exactly one datagram and returned when the
//! authenticated plaintext leaves the client data plane. The wrapper never
//! exposes the underlying allocation length, so shrinking a packet only changes
//! the visible slice and never zero-fills or reallocates the 64 KiB allocation.

use bytes::BytesMut;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) const UDP_RECEIVE_BUFFER_SIZE: usize = 65_536;
pub(crate) const DEFAULT_UDP_BUFFER_POOL_CAPACITY: usize = 256;
const BUFFER_POOL_SHARDS: usize = 16;

/// Allocation and ownership counters for the UDP receive buffer pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpBufferPoolMetrics {
    /// Configured maximum number of retained free-list buffers.
    pub capacity: u64,
    /// Calls that found an existing buffer in the free list.
    pub hits: u64,
    /// Calls that had to allocate a new buffer because the free list was empty.
    pub allocations: u64,
    /// Buffers returned to the free list.
    pub returns: u64,
    /// Buffers dropped instead of retained because the free list was full.
    pub full_returns: u64,
}

impl UdpBufferPoolMetrics {
    /// Number of buffers currently owned by packets or the receiver.
    pub fn in_flight(self) -> u64 {
        self.hits
            .saturating_add(self.allocations)
            .saturating_sub(self.returns)
            .saturating_sub(self.full_returns)
    }

    /// Free-list hit rate in the inclusive range 0..=1.
    pub fn hit_rate(self) -> f64 {
        let acquisitions = self.hits.saturating_add(self.allocations);
        if acquisitions == 0 {
            0.0
        } else {
            self.hits as f64 / acquisitions as f64
        }
    }
}

#[derive(Debug, Default)]
struct PoolCounters {
    hits: AtomicU64,
    allocations: AtomicU64,
    returns: AtomicU64,
    full_returns: AtomicU64,
}

/// Fixed-capacity pool of unique 64 KiB UDP receive allocations.
#[derive(Debug)]
pub(crate) struct UdpBufferPool {
    capacity: usize,
    /// Sharded free lists prevent one batch acquisition/recycle pass from
    /// serializing every UDP datagram on a single pool mutex. The per-shard
    /// capacities sum to `capacity`, so the total retained free memory remains
    /// bounded by the configured pool size.
    free: Box<[Mutex<Vec<Vec<u8>>>]>,
    shard_capacities: Box<[usize]>,
    next_shard: AtomicUsize,
    counters: PoolCounters,
}

impl UdpBufferPool {
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        let shard_count = capacity.clamp(1, BUFFER_POOL_SHARDS);
        let base = capacity / shard_count;
        let remainder = capacity % shard_count;
        let shard_capacities: Vec<usize> = (0..shard_count)
            .map(|index| base + usize::from(index < remainder))
            .collect();
        let free = shard_capacities
            .iter()
            .map(|capacity| Mutex::new(Vec::with_capacity(*capacity)))
            .collect::<Vec<_>>();
        Arc::new(Self {
            capacity,
            free: free.into_boxed_slice(),
            shard_capacities: shard_capacities.into_boxed_slice(),
            next_shard: AtomicUsize::new(0),
            counters: PoolCounters::default(),
        })
    }

    pub(crate) fn acquire(self: &Arc<Self>) -> InboundPayload {
        let start = self.next_shard.fetch_add(1, Ordering::Relaxed) % self.free.len();
        let existing = (0..self.free.len()).find_map(|offset| {
            let shard = (start + offset) % self.free.len();
            self.free[shard]
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop()
                .map(|buffer| (shard, buffer))
        });
        let (shard, buffer) = match existing {
            Some(buffer) => {
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                buffer
            }
            None => {
                self.counters.allocations.fetch_add(1, Ordering::Relaxed);
                (start, vec![0u8; UDP_RECEIVE_BUFFER_SIZE])
            }
        };
        debug_assert_eq!(buffer.len(), UDP_RECEIVE_BUFFER_SIZE);
        InboundPayload {
            storage: InboundStorage::Vec(buffer),
            valid_len: UDP_RECEIVE_BUFFER_SIZE,
            pool: Some(self.clone()),
            pool_shard: Some(shard),
        }
    }

    fn recycle(&self, shard: usize, buffer: Vec<u8>) {
        debug_assert_eq!(buffer.len(), UDP_RECEIVE_BUFFER_SIZE);
        let mut free = self.free[shard]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if free.len() >= self.shard_capacities[shard] {
            self.counters.full_returns.fetch_add(1, Ordering::Relaxed);
            return;
        }
        free.push(buffer);
        self.counters.returns.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn metrics(&self) -> UdpBufferPoolMetrics {
        UdpBufferPoolMetrics {
            capacity: self.capacity as u64,
            hits: self.counters.hits.load(Ordering::Relaxed),
            allocations: self.counters.allocations.load(Ordering::Relaxed),
            returns: self.counters.returns.load(Ordering::Relaxed),
            full_returns: self.counters.full_returns.load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn free_len(&self) -> usize {
        self.free
            .iter()
            .map(|free| {
                free.lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len()
            })
            .sum()
    }
}

/// Owned packet bytes that may retain a pooled UDP receive allocation.
///
/// Pooled buffers expose only `valid_len` through `Deref`; the 64 KiB backing
/// allocation remains intact until the payload is returned to the pool.
pub struct InboundPayload {
    storage: InboundStorage,
    valid_len: usize,
    pool: Option<Arc<UdpBufferPool>>,
    pool_shard: Option<usize>,
}

enum InboundStorage {
    Vec(Vec<u8>),
    Bytes(BytesMut),
}

impl InboundPayload {
    pub(crate) fn from_vec(buffer: Vec<u8>) -> Self {
        let valid_len = buffer.len();
        Self {
            storage: InboundStorage::Vec(buffer),
            valid_len,
            pool: None,
            pool_shard: None,
        }
    }

    /// Preserve a relay plaintext backed by `BytesMut` without converting it
    /// back into a fresh `Vec`.
    pub(crate) fn from_bytes(buffer: BytesMut) -> Self {
        let valid_len = buffer.len();
        Self {
            storage: InboundStorage::Bytes(buffer),
            valid_len,
            pool: None,
            pool_shard: None,
        }
    }

    /// Shrink the visible payload without changing the owned allocation.
    ///
    /// Growing is deliberately forbidden: pooled buffers are not wiped on
    /// recycle, so allowing a reused short packet to expose its old tail would
    /// turn an internal zero-copy optimization into a data-disclosure hazard.
    pub(crate) fn truncate(&mut self, len: usize) {
        assert!(
            len <= self.valid_len,
            "pooled payload length cannot grow from {} to {len}",
            self.valid_len
        );
        self.valid_len = len;
    }

    /// Bytes held by the backing allocation, not just the visible payload.
    ///
    /// The UDP receive pool recycles the full 64 KiB allocation even after a
    /// packet is truncated to its authenticated plaintext length. Queue memory
    /// accounting must therefore charge retained capacity.
    pub(crate) fn retained_len(&self) -> usize {
        match &self.storage {
            InboundStorage::Vec(buffer) => buffer.capacity(),
            InboundStorage::Bytes(buffer) => buffer.capacity(),
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        match &self.storage {
            InboundStorage::Vec(buffer) => &buffer[..self.valid_len],
            InboundStorage::Bytes(buffer) => &buffer[..self.valid_len],
        }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        match &mut self.storage {
            InboundStorage::Vec(buffer) => &mut buffer[..self.valid_len],
            InboundStorage::Bytes(buffer) => &mut buffer[..self.valid_len],
        }
    }

    #[cfg(test)]
    pub(crate) fn is_pooled(&self) -> bool {
        self.pool.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_bytes_backed(&self) -> bool {
        matches!(self.storage, InboundStorage::Bytes(_))
    }
}

impl Deref for InboundPayload {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for InboundPayload {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl From<Vec<u8>> for InboundPayload {
    fn from(buffer: Vec<u8>) -> Self {
        Self::from_vec(buffer)
    }
}

impl Drop for InboundPayload {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            let storage = std::mem::replace(&mut self.storage, InboundStorage::Vec(Vec::new()));
            let InboundStorage::Vec(buffer) = storage else {
                unreachable!("pooled inbound payloads always use Vec storage");
            };
            self.valid_len = 0;
            let shard = self
                .pool_shard
                .take()
                .expect("pooled inbound payload has a shard");
            pool.recycle(shard, buffer);
        }
    }
}

impl fmt::Debug for InboundPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InboundPayload")
            .field("len", &self.valid_len)
            .field("pooled", &self.pool.is_some())
            .finish()
    }
}

impl PartialEq for InboundPayload {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for InboundPayload {}

impl PartialEq<Vec<u8>> for InboundPayload {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<&[u8]> for InboundPayload {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_slice() == *other
    }
}

impl<const N: usize> PartialEq<[u8; N]> for InboundPayload {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_slice() == other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for InboundPayload {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_slice() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_backed_payload_preserves_codec_allocation() {
        let source = BytesMut::from(&b"relay plaintext"[..]);
        let source_ptr = source.as_ptr();
        let mut payload = InboundPayload::from_bytes(source);
        assert!(payload.is_bytes_backed());
        assert!(!payload.is_pooled());
        assert_eq!(&payload[..], b"relay plaintext");
        assert_eq!(payload.as_ptr(), source_ptr);
        payload.truncate(5);
        assert_eq!(&payload[..], b"relay");
        assert_eq!(payload.retained_len(), 15);
    }

    #[test]
    fn pool_reuses_shrunk_buffers_without_reallocation() {
        let pool = UdpBufferPool::new(2);
        let mut first = pool.acquire();
        first.truncate(17);

        let metrics = pool.metrics();
        assert_eq!(metrics.capacity, 2);
        assert_eq!(metrics.allocations, 1);
        assert_eq!(metrics.hits, 0);
        assert_eq!(pool.free_len(), 0);

        drop(first);
        assert_eq!(pool.metrics().returns, 1);
        assert_eq!(pool.free_len(), 1);

        let second = pool.acquire();
        assert_eq!(pool.metrics().allocations, 1);
        assert_eq!(pool.metrics().hits, 1);
        assert_eq!(second.len(), UDP_RECEIVE_BUFFER_SIZE);
        assert_eq!(second.as_slice().len(), UDP_RECEIVE_BUFFER_SIZE);
    }

    #[test]
    fn pool_full_drops_excess_buffers_with_metrics() {
        let pool = UdpBufferPool::new(1);
        let retained = pool.acquire();
        let excess = pool.acquire();
        drop(retained);
        assert_eq!(pool.metrics().returns, 1);
        assert_eq!(pool.free_len(), 1);

        drop(excess);
        let metrics = pool.metrics();
        assert_eq!(metrics.full_returns, 1);
        assert_eq!(pool.free_len(), 1);
    }

    #[test]
    #[should_panic(expected = "pooled payload length cannot grow")]
    fn pooled_payload_cannot_expose_stale_tail() {
        let pool = UdpBufferPool::new(1);
        let mut buffer = pool.acquire();
        buffer.as_mut_slice()[..32].fill(0xA5);
        buffer.truncate(8);
        buffer.truncate(16);
    }

    #[test]
    fn short_read_restores_full_receive_slice() {
        let pool = UdpBufferPool::new(1);
        let mut buffer = pool.acquire();
        buffer.truncate(24);
        assert_eq!(buffer.len(), 24);
        drop(buffer);

        let buffer = pool.acquire();
        assert_eq!(buffer.len(), UDP_RECEIVE_BUFFER_SIZE);
    }

    #[test]
    fn sharded_pool_keeps_total_capacity_bounded() {
        let pool = UdpBufferPool::new(8);
        let buffers: Vec<_> = (0..32).map(|_| pool.acquire()).collect();
        drop(buffers);

        let metrics = pool.metrics();
        assert_eq!(metrics.capacity, 8);
        assert_eq!(metrics.allocations, 32);
        assert_eq!(metrics.returns, 8);
        assert_eq!(metrics.full_returns, 24);
        assert_eq!(pool.free_len(), 8);
        assert!(pool.free_len() as u64 <= metrics.capacity);
    }

    #[test]
    fn concurrent_acquire_recycle_does_not_leak_or_exceed_capacity() {
        let pool = UdpBufferPool::new(32);
        let mut workers = Vec::new();
        for _ in 0..8 {
            let pool = pool.clone();
            workers.push(std::thread::spawn(move || {
                for _ in 0..2_000 {
                    let mut buffer = pool.acquire();
                    buffer.truncate(64);
                    drop(buffer);
                }
            }));
        }
        for worker in workers {
            worker.join().expect("buffer worker should not panic");
        }

        let metrics = pool.metrics();
        assert_eq!(metrics.in_flight(), 0);
        assert!(pool.free_len() <= 32);
        assert!(metrics.hits > 0);
        assert_eq!(
            metrics.allocations + metrics.hits,
            metrics.returns + metrics.full_returns
        );
    }
}
