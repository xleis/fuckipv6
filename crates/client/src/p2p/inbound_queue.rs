//! Bounded ingress queue for authenticated client data.
//!
//! Secure-session control frames are handled by the UDP dispatcher before this
//! queue and therefore cannot be displaced by a data burst. Ordinary data uses
//! drop-newest admission: once the bounded channel is full, the arriving packet
//! is released and counted instead of growing memory without limit.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::connection::InboundPacket;

pub(crate) const DEFAULT_INBOUND_QUEUE_CAPACITY: usize = 1024;
pub(crate) const MAX_INBOUND_QUEUE_CAPACITY: usize = 65_536;
pub(crate) const MAX_INBOUND_QUEUE_BYTES: u64 = 64 * 1024 * 1024;
const QUEUE_BUDGET_BYTES_PER_PACKET: u64 = 65_536;

pub(crate) fn queue_byte_budget_for_capacity(capacity: usize) -> u64 {
    (capacity as u64)
        .saturating_mul(QUEUE_BUDGET_BYTES_PER_PACKET)
        .min(MAX_INBOUND_QUEUE_BYTES)
}

/// Point-in-time bounded ingress queue counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboundQueueMetrics {
    pub capacity: usize,
    /// Retained backing-allocation byte budget.
    pub max_bytes: u64,
    pub depth: u64,
    pub max_depth: u64,
    /// Retained backing-allocation bytes currently queued.
    pub depth_bytes: u64,
    pub max_depth_bytes: u64,
    pub enqueued: u64,
    /// Retained backing-allocation bytes accepted by the queue.
    pub enqueued_bytes: u64,
    pub dequeued: u64,
    /// Retained backing-allocation bytes released after dequeue.
    pub dequeued_bytes: u64,
    /// Newest data packets dropped because the packet-count limit was full.
    pub dropped_full: u64,
    /// Newest data packets dropped because the byte budget was full.
    pub dropped_byte_full: u64,
    /// Enqueue attempts rejected because the receive side was shut down.
    pub rejected_closed: u64,
    /// Secure control frames handled before the data queue.
    pub control_frames_bypassed: u64,
    /// Consumer receive attempts that found the queue empty and had to wait.
    pub consumer_waits: u64,
}

impl InboundQueueMetrics {
    pub fn drop_rate(self) -> f64 {
        let dropped = self.dropped_full.saturating_add(self.dropped_byte_full);
        let offered = self.enqueued.saturating_add(dropped);
        if offered == 0 {
            0.0
        } else {
            dropped as f64 / offered as f64
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct InboundQueueStats {
    capacity: AtomicUsize,
    max_bytes: AtomicU64,
    depth: AtomicU64,
    max_depth: AtomicU64,
    depth_bytes: AtomicU64,
    max_depth_bytes: AtomicU64,
    enqueued: AtomicU64,
    enqueued_bytes: AtomicU64,
    dequeued: AtomicU64,
    dequeued_bytes: AtomicU64,
    dropped_full: AtomicU64,
    dropped_byte_full: AtomicU64,
    rejected_closed: AtomicU64,
    control_frames_bypassed: AtomicU64,
    consumer_waits: AtomicU64,
}

impl InboundQueueStats {
    fn with_limits(capacity: usize, max_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            capacity: AtomicUsize::new(capacity),
            max_bytes: AtomicU64::new(max_bytes),
            ..Self::default()
        })
    }

    pub(crate) fn metrics(&self) -> InboundQueueMetrics {
        InboundQueueMetrics {
            capacity: self.capacity.load(Ordering::Relaxed),
            max_bytes: self.max_bytes.load(Ordering::Relaxed),
            depth: self.depth.load(Ordering::Relaxed),
            max_depth: self.max_depth.load(Ordering::Relaxed),
            depth_bytes: self.depth_bytes.load(Ordering::Relaxed),
            max_depth_bytes: self.max_depth_bytes.load(Ordering::Relaxed),
            enqueued: self.enqueued.load(Ordering::Relaxed),
            enqueued_bytes: self.enqueued_bytes.load(Ordering::Relaxed),
            dequeued: self.dequeued.load(Ordering::Relaxed),
            dequeued_bytes: self.dequeued_bytes.load(Ordering::Relaxed),
            dropped_full: self.dropped_full.load(Ordering::Relaxed),
            dropped_byte_full: self.dropped_byte_full.load(Ordering::Relaxed),
            rejected_closed: self.rejected_closed.load(Ordering::Relaxed),
            control_frames_bypassed: self.control_frames_bypassed.load(Ordering::Relaxed),
            consumer_waits: self.consumer_waits.load(Ordering::Relaxed),
        }
    }

    fn try_reserve_bytes(&self, bytes: u64) -> bool {
        let max = self.max_bytes.load(Ordering::Relaxed);
        let mut current = self.depth_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > max {
                return false;
            }
            match self.depth_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.max_depth_bytes.fetch_max(next, Ordering::AcqRel);
                    return true;
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn release_bytes(&self, bytes: u64) {
        self.depth_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    fn reserve_depth(&self) {
        self.depth.fetch_add(1, Ordering::AcqRel);
    }

    fn cancel_depth(&self) {
        self.depth.fetch_sub(1, Ordering::AcqRel);
    }

    fn record_enqueued(&self, bytes: u64) {
        self.enqueued.fetch_add(1, Ordering::Relaxed);
        self.enqueued_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.max_depth
            .fetch_max(self.depth.load(Ordering::Acquire), Ordering::AcqRel);
    }

    fn record_dequeued(&self, bytes: u64) {
        self.dequeued.fetch_add(1, Ordering::Relaxed);
        self.dequeued_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.depth.fetch_sub(1, Ordering::AcqRel);
        self.release_bytes(bytes);
    }

    fn record_closed(&self) {
        self.rejected_closed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Cloneable producer. Cloning it does not create another consumer.
#[derive(Clone)]
pub(crate) struct InboundQueueSender {
    tx: mpsc::Sender<InboundPacket>,
    stats: Arc<InboundQueueStats>,
}

impl InboundQueueSender {
    /// Drop-newest admission for ordinary data.
    pub(crate) fn try_send(&self, packet: InboundPacket) -> bool {
        let bytes = packet.retained_len() as u64;
        if !self.stats.try_reserve_bytes(bytes) {
            self.stats.dropped_byte_full.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // Reserve depth before publishing the packet. A receiver may dequeue it
        // immediately after `try_send` succeeds, so updating depth afterwards
        // would create a transient underflow window.
        self.stats.reserve_depth();
        match self.tx.try_send(packet) {
            Ok(()) => {
                self.stats.record_enqueued(bytes);
                true
            }
            Err(mpsc::error::TrySendError::Full(_dropped)) => {
                self.stats.cancel_depth();
                self.stats.release_bytes(bytes);
                self.stats.dropped_full.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_dropped)) => {
                self.stats.cancel_depth();
                self.stats.release_bytes(bytes);
                self.stats.record_closed();
                false
            }
        }
    }

    /// Secure-session control is processed inline and never enters this queue.
    pub(crate) fn record_control_frame_bypass(&self) {
        self.stats
            .control_frames_bypassed
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn metrics(&self) -> InboundQueueMetrics {
        self.stats.metrics()
    }
}

/// Single-consumer receive endpoint.
pub(crate) struct InboundQueueReceiver {
    rx: mpsc::Receiver<InboundPacket>,
    stats: Arc<InboundQueueStats>,
}

impl InboundQueueReceiver {
    pub(crate) async fn recv(&mut self) -> Option<InboundPacket> {
        let packet = match self.rx.try_recv() {
            Ok(packet) => Some(packet),
            Err(mpsc::error::TryRecvError::Empty) => {
                self.stats.consumer_waits.fetch_add(1, Ordering::Relaxed);
                self.rx.recv().await
            }
            Err(mpsc::error::TryRecvError::Disconnected) => None,
        };
        if let Some(packet) = packet.as_ref() {
            self.stats.record_dequeued(packet.retained_len() as u64);
        }
        packet
    }
}

pub(crate) fn channel(
    capacity: usize,
) -> (
    InboundQueueSender,
    InboundQueueReceiver,
    Arc<InboundQueueStats>,
) {
    channel_with_limits(capacity, queue_byte_budget_for_capacity(capacity))
}

fn channel_with_limits(
    capacity: usize,
    max_bytes: u64,
) -> (
    InboundQueueSender,
    InboundQueueReceiver,
    Arc<InboundQueueStats>,
) {
    assert!(capacity > 0, "inbound queue capacity must be non-zero");
    let (tx, rx) = mpsc::channel(capacity);
    let stats = InboundQueueStats::with_limits(capacity, max_bytes);
    (
        InboundQueueSender {
            tx,
            stats: stats.clone(),
        },
        InboundQueueReceiver {
            rx,
            stats: stats.clone(),
        },
        stats,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::buffer_pool::InboundPayload;
    use crate::p2p::connection::PeerRouteInfo;
    use fuckipv6_common::VirtualIps;
    use std::collections::{BTreeMap, HashSet};

    fn test_route() -> PeerRouteInfo {
        let ipv6 = "fd00::1".parse().expect("test IP");
        PeerRouteInfo {
            client_id: Arc::from("test-peer"),
            virtual_ip: ipv6,
            virtual_ips: VirtualIps::from_ipv6(ipv6),
        }
    }

    fn packet(marker: u8) -> InboundPacket {
        packet_with_len(marker, 1)
    }

    fn packet_with_len(marker: u8, len: usize) -> InboundPacket {
        InboundPacket::AuthenticatedPlaintext {
            src_ip: "fd00::1".parse().expect("test IP"),
            peer: test_route(),
            payload: InboundPayload::from(vec![marker; len]),
        }
    }

    fn indexed_payload(marker: usize, len: usize) -> Vec<u8> {
        assert!(len >= size_of::<u16>());
        let mut payload = vec![0; len];
        payload[..size_of::<u16>()].copy_from_slice(&(marker as u16).to_le_bytes());
        payload
    }

    fn indexed_packet(marker: usize, len: usize) -> InboundPacket {
        InboundPacket::AuthenticatedPlaintext {
            src_ip: "fd00::1".parse().expect("test IP"),
            peer: test_route(),
            payload: InboundPayload::from(indexed_payload(marker, len)),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct TaggedPacket {
        peer: u8,
        generation: u8,
        sequence: u16,
    }

    impl TaggedPacket {
        fn payload(self) -> Vec<u8> {
            let mut payload = vec![0u8; 8];
            payload[0] = self.peer;
            payload[1] = self.generation;
            payload[2..4].copy_from_slice(&self.sequence.to_le_bytes());
            payload
        }

        fn from_payload(payload: &[u8]) -> Self {
            assert!(payload.len() >= 4);
            Self {
                peer: payload[0],
                generation: payload[1],
                sequence: u16::from_le_bytes([payload[2], payload[3]]),
            }
        }
    }

    fn tagged_packet(packet: TaggedPacket) -> InboundPacket {
        InboundPacket::AuthenticatedPlaintext {
            src_ip: "fd00::1".parse().expect("test IP"),
            peer: test_route(),
            payload: InboundPayload::from(packet.payload()),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAdmissionOutcome {
        Accepted,
        RemovedPeer,
        Replay,
        OutsideGenerationWindow,
    }

    /// Test-local stand-in for the admission decisions made before this queue.
    ///
    /// The queue only sees packets that already passed peer removal, replay
    /// filtering, and rekey generation checks. Keeping that model explicit lets
    /// this module exercise the mixed workload without pretending to test
    /// cryptographic authentication here.
    struct TestPeerAdmission {
        current_generation: u8,
        active_peers: [bool; 3],
        seen: HashSet<TaggedPacket>,
        removed_rejected: usize,
        replay_rejected: usize,
        generation_rejected: usize,
    }

    impl TestPeerAdmission {
        fn new() -> Self {
            Self {
                current_generation: 0,
                active_peers: [true; 3],
                seen: HashSet::new(),
                removed_rejected: 0,
                replay_rejected: 0,
                generation_rejected: 0,
            }
        }

        fn remove_peer(&mut self, peer: u8) {
            self.active_peers[peer as usize] = false;
        }

        fn rekey(&mut self) {
            self.current_generation = self.current_generation.saturating_add(1);
        }

        fn admit(&mut self, packet: TaggedPacket) -> TestAdmissionOutcome {
            if !self.active_peers[packet.peer as usize] {
                self.removed_rejected += 1;
                return TestAdmissionOutcome::RemovedPeer;
            }

            if packet.generation > self.current_generation
                || self.current_generation.saturating_sub(packet.generation) > 1
            {
                self.generation_rejected += 1;
                return TestAdmissionOutcome::OutsideGenerationWindow;
            }

            if !self.seen.insert(packet) {
                self.replay_rejected += 1;
                return TestAdmissionOutcome::Replay;
            }

            TestAdmissionOutcome::Accepted
        }
    }

    fn admit_range(
        admission: &mut TestPeerAdmission,
        peer: u8,
        generation: u8,
        sequences: std::ops::Range<u16>,
        queue_offers: &mut Vec<TaggedPacket>,
    ) {
        for sequence in sequences {
            let packet = TaggedPacket {
                peer,
                generation,
                sequence,
            };
            if admission.admit(packet) == TestAdmissionOutcome::Accepted {
                queue_offers.push(packet);
            }
        }
    }

    #[tokio::test]
    async fn bounded_queue_drops_newest_and_reports_depth() {
        let (tx, mut rx, stats) = channel(2);
        assert!(tx.try_send(packet(1)));
        assert!(tx.try_send(packet(2)));
        assert!(!tx.try_send(packet(3)));

        let metrics = stats.metrics();
        assert_eq!(metrics.capacity, 2);
        assert_eq!(metrics.depth, 2);
        assert_eq!(metrics.max_depth, 2);
        assert_eq!(metrics.dropped_full, 1);
        assert_eq!(metrics.drop_rate(), 1.0 / 3.0);

        let first = rx.recv().await.expect("first packet");
        let second = rx.recv().await.expect("second packet");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = first else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload.as_slice(), &[1]);
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = second else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload.as_slice(), &[2]);
        assert_eq!(stats.metrics().depth, 0);
        assert_eq!(stats.metrics().dequeued, 2);
    }

    #[tokio::test]
    async fn byte_budget_drops_newest_and_releases_on_dequeue() {
        let (tx, mut rx, stats) = channel_with_limits(4, 6);
        assert!(tx.try_send(packet_with_len(1, 4)));
        assert!(!tx.try_send(packet_with_len(2, 4)));

        let saturated = stats.metrics();
        assert_eq!(saturated.max_bytes, 6);
        assert_eq!(saturated.depth, 1);
        assert_eq!(saturated.depth_bytes, 4);
        assert_eq!(saturated.dropped_full, 0);
        assert_eq!(saturated.dropped_byte_full, 1);
        assert_eq!(saturated.drop_rate(), 0.5);

        let received = rx.recv().await.expect("packet within byte budget");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload, vec![1; 4]);
        let drained = stats.metrics();
        assert_eq!(drained.depth_bytes, 0);
        assert_eq!(drained.dequeued_bytes, 4);

        assert!(tx.try_send(packet_with_len(3, 6)));
        let received = rx.recv().await.expect("full budget after dequeue");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload, vec![3; 6]);
    }

    #[tokio::test]
    async fn byte_budget_charges_pooled_allocation_not_visible_length() {
        use crate::p2p::buffer_pool::UdpBufferPool;

        let pool = UdpBufferPool::new(2);
        let mut payload = pool.acquire();
        let retained = payload.retained_len();
        payload.truncate(1);
        assert_eq!(payload.as_slice(), &[0]);

        let (tx, mut rx, stats) = channel_with_limits(4, retained as u64);
        assert!(tx.try_send(InboundPacket::AuthenticatedPlaintext {
            src_ip: "fd00::1".parse().expect("test IP"),
            peer: test_route(),
            payload,
        }));
        assert!(!tx.try_send(packet(2)));

        let saturated = stats.metrics();
        assert_eq!(saturated.depth_bytes, retained as u64);
        assert_eq!(saturated.dropped_byte_full, 1);

        let received = rx.recv().await.expect("pooled packet");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload.as_slice(), &[0]);
        assert_eq!(stats.metrics().depth_bytes, 0);
        assert_eq!(stats.metrics().dequeued_bytes, retained as u64);
    }

    #[tokio::test]
    async fn control_frames_bypass_full_data_queue() {
        let (tx, _rx, stats) = channel(1);
        assert!(tx.try_send(packet(1)));
        assert!(!tx.try_send(packet(2)));
        tx.record_control_frame_bypass();

        let metrics = stats.metrics();
        assert_eq!(metrics.dropped_full, 1);
        assert_eq!(metrics.control_frames_bypassed, 1);
    }

    #[tokio::test]
    async fn shutdown_unblocks_consumer() {
        let (tx, mut rx, stats) = channel(1);
        drop(tx);
        assert!(rx.recv().await.is_none());
        assert_eq!(stats.metrics().consumer_waits, 0);
    }

    #[tokio::test]
    async fn empty_receive_wait_is_counted() {
        let (tx, mut rx, stats) = channel(1);
        let receive = tokio::spawn(async move { rx.recv().await });
        tokio::task::yield_now().await;
        assert!(tx.try_send(packet(7)));
        let received = receive.await.expect("receiver task").expect("packet");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = received else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload.as_slice(), &[7]);
        assert_eq!(stats.metrics().consumer_waits, 1);
    }

    #[tokio::test]
    async fn default_capacity_4097_plus_burst_keeps_oldest_1024_and_drops_newest() {
        const BURST: usize = DEFAULT_INBOUND_QUEUE_CAPACITY * 8 + 1;
        let (tx, mut rx, stats) = channel(DEFAULT_INBOUND_QUEUE_CAPACITY);
        for marker in 0..BURST {
            let accepted = tx.try_send(indexed_packet(marker, 8));
            assert_eq!(
                accepted,
                marker < DEFAULT_INBOUND_QUEUE_CAPACITY,
                "drop-newest admission diverged at offered packet {marker}"
            );
        }

        let saturated = stats.metrics();
        assert_eq!(saturated.capacity, DEFAULT_INBOUND_QUEUE_CAPACITY);
        assert_eq!(saturated.depth, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(saturated.max_depth, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(saturated.enqueued, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(
            saturated.dropped_full,
            (BURST - DEFAULT_INBOUND_QUEUE_CAPACITY) as u64
        );
        assert_eq!(saturated.dropped_byte_full, 0);

        for marker in 0..DEFAULT_INBOUND_QUEUE_CAPACITY {
            let InboundPacket::AuthenticatedPlaintext { payload, .. } =
                rx.recv().await.expect("queued packet")
            else {
                panic!("expected authenticated plaintext");
            };
            assert_eq!(
                payload,
                indexed_payload(marker, 8),
                "the queue must retain the oldest packets and drop only new arrivals"
            );
        }

        let drained = stats.metrics();
        assert_eq!(drained.depth, 0);
        assert_eq!(drained.dequeued, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(
            drained.dequeued.saturating_add(drained.dropped_full),
            BURST as u64
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv())
                .await
                .is_err(),
            "dropped newest packets must not reappear"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn peer_remove_replay_rekey_pressure_is_bounded_and_exactly_accounted() {
        let (tx, mut rx, stats) = channel(DEFAULT_INBOUND_QUEUE_CAPACITY);
        let mut admission = TestPeerAdmission::new();
        let mut queue_offers = Vec::new();

        // Initial traffic from three live peers in generation zero.
        for peer in 0..3 {
            admit_range(&mut admission, peer, 0, 0..1024, &mut queue_offers);
        }
        assert_eq!(
            admission.admit(TaggedPacket {
                peer: 0,
                generation: 0,
                sequence: 0,
            }),
            TestAdmissionOutcome::Replay
        );

        // Future generations must not enter the queue before rekey completes.
        admit_range(&mut admission, 0, 1, 0..32, &mut queue_offers);

        // Remove one peer: all later authenticated attempts for it stay out.
        admission.remove_peer(2);
        admit_range(&mut admission, 2, 0, 1024..1536, &mut queue_offers);

        // Rekey to generation one. Previous-generation packets retain grace,
        // while current-generation traffic starts flowing.
        admission.rekey();
        admit_range(&mut admission, 0, 0, 1024..3072, &mut queue_offers);
        admit_range(&mut admission, 1, 1, 0..2048, &mut queue_offers);
        assert_eq!(
            admission.admit(TaggedPacket {
                peer: 1,
                generation: 1,
                sequence: 0,
            }),
            TestAdmissionOutcome::Replay
        );
        admit_range(&mut admission, 0, 2, 2048..2080, &mut queue_offers);

        // A second rekey makes generation zero too old, retains generation one
        // as the previous generation, and admits generation two.
        admission.rekey();
        admit_range(&mut admission, 0, 0, 3072..4096, &mut queue_offers);
        admit_range(&mut admission, 0, 1, 3072..4096, &mut queue_offers);
        admit_range(&mut admission, 1, 2, 2048..4096, &mut queue_offers);
        assert_eq!(
            admission.admit(TaggedPacket {
                peer: 1,
                generation: 2,
                sequence: 2048,
            }),
            TestAdmissionOutcome::Replay
        );

        assert!(admission.removed_rejected > 0);
        assert!(admission.replay_rejected > 0);
        assert!(admission.generation_rejected > 0);
        assert!(
            queue_offers.len() > 4097,
            "the mixed workload must exceed the default queue capacity several times over"
        );

        // Model per-peer/epoch receiver handles. They all share the queue but
        // can retire independently; no individual handle owns channel closure.
        let mut by_source: BTreeMap<(u8, u8), Vec<TaggedPacket>> = BTreeMap::new();
        for packet in queue_offers.iter().copied() {
            by_source
                .entry((packet.peer, packet.generation))
                .or_default()
                .push(packet);
        }
        let mut producers = Vec::new();
        for ((_peer, _generation), packets) in by_source {
            let sender = tx.clone();
            producers.push(tokio::spawn(async move {
                let mut accepted = Vec::new();
                let mut dropped = Vec::new();
                for packet in packets {
                    if sender.try_send(tagged_packet(packet)) {
                        accepted.push(packet);
                    } else {
                        dropped.push(packet);
                    }
                }
                (accepted, dropped)
            }));
        }

        let mut accepted = HashSet::new();
        let mut dropped = HashSet::new();
        for producer in producers {
            let (producer_accepted, producer_dropped) = producer.await.expect("producer task");
            for packet in producer_accepted {
                assert!(accepted.insert(packet), "duplicate queue admission");
            }
            for packet in producer_dropped {
                assert!(dropped.insert(packet), "duplicate queue drop");
            }
        }

        assert_eq!(accepted.len(), DEFAULT_INBOUND_QUEUE_CAPACITY);
        assert_eq!(accepted.len() + dropped.len(), queue_offers.len());
        assert!(accepted.is_disjoint(&dropped));

        let saturated = stats.metrics();
        assert_eq!(saturated.depth, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(saturated.enqueued, DEFAULT_INBOUND_QUEUE_CAPACITY as u64);
        assert_eq!(
            saturated.dropped_full,
            (queue_offers.len() - DEFAULT_INBOUND_QUEUE_CAPACITY) as u64
        );
        assert_eq!(saturated.dropped_byte_full, 0);

        let consumer = tokio::spawn(async move {
            let mut received = HashSet::new();
            while let Some(packet) = rx.recv().await {
                let InboundPacket::AuthenticatedPlaintext { payload, .. } = packet else {
                    panic!("expected authenticated plaintext");
                };
                assert!(
                    received.insert(TaggedPacket::from_payload(payload.as_slice())),
                    "an accepted packet was delivered more than once"
                );
            }
            received
        });
        drop(tx);

        let received = consumer.await.expect("consumer task");
        assert_eq!(received, accepted);
        assert!(received.is_disjoint(&dropped));

        let drained = stats.metrics();
        assert_eq!(drained.depth, 0);
        assert_eq!(drained.dequeued, accepted.len() as u64);
        assert_eq!(drained.dropped_full, dropped.len() as u64);
        assert_eq!(
            drained.dequeued.saturating_add(drained.dropped_full),
            queue_offers.len() as u64
        );
    }

    #[tokio::test]
    async fn slow_consumer_stays_bounded_and_recovers_after_drain() {
        let (tx, mut rx, stats) = channel(8);
        for marker in 0..32 {
            let _ = tx.try_send(packet(marker));
        }

        let saturated = stats.metrics();
        assert_eq!(saturated.depth, 8);
        assert_eq!(saturated.max_depth, 8);
        assert_eq!(saturated.enqueued, 8);
        assert_eq!(saturated.dropped_full, 24);

        for marker in 0..8 {
            let packet = rx.recv().await.expect("queued packet");
            let InboundPacket::AuthenticatedPlaintext { payload, .. } = packet else {
                panic!("expected authenticated plaintext");
            };
            assert_eq!(payload.as_slice(), &[marker]);
        }
        assert_eq!(stats.metrics().depth, 0);

        assert!(tx.try_send(packet(99)));
        let packet = rx.recv().await.expect("new packet after drain");
        let InboundPacket::AuthenticatedPlaintext { payload, .. } = packet else {
            panic!("expected authenticated plaintext");
        };
        assert_eq!(payload.as_slice(), &[99]);
    }

    #[tokio::test]
    async fn closed_queue_rejections_are_counted() {
        let (tx, rx, stats) = channel(1);
        drop(rx);
        assert!(!tx.try_send(packet(1)));
        let metrics = stats.metrics();
        assert_eq!(metrics.rejected_closed, 1);
        assert_eq!(metrics.depth, 0);
        assert_eq!(metrics.enqueued, 0);
    }
}
