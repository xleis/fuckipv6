use fuckipv6_common::FallbackThreshold;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const BANDWIDTH_WINDOW: Duration = Duration::from_secs(30);
const TRAFFIC_BUCKET_COUNT: usize = 31;
const TRAFFIC_WINDOW_SECS: u64 = 30;
const TRAFFIC_BUCKET_GENERATION_SHIFT: u32 = 32;
/// Byte counts are packed into the low 32 bits of each one-second bucket and
/// saturate at `u32::MAX`. Saturation only bounds the precision of the reported
/// bandwidth; activity and probe health use separate state.
const TRAFFIC_BUCKET_VALUE_MASK: u64 = u32::MAX as u64;
const NANOS_PER_SEC: u64 = 1_000_000_000;
const NO_ACTIVITY: u64 = 0;

#[derive(Debug, Clone)]
pub struct ConnectionHealthSnapshot {
    pub rtt_us: Option<u64>,
    pub loss_rate: f32,
    pub bandwidth_bps: u64,
    pub probes_sent: u64,
    pub probes_acked: u64,
    pub window_probes: usize,
    pub window_lost: usize,
    pub probe_success_rate: f32,
    pub last_activity_secs: Option<u64>,
    pub last_success_secs: Option<u64>,
    pub remote_quality: Option<RemoteQualitySnapshot>,
    pub healthy: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RemoteQualitySnapshot {
    pub rtt_us: u64,
    pub loss_rate: f32,
    pub bandwidth_bps: u64,
    pub age_secs: u64,
}

#[derive(Debug)]
pub(crate) struct PeerHealth {
    traffic: TrafficWindow,
    probes: Mutex<ProbeHealth>,
}

#[derive(Debug)]
struct ProbeHealth {
    window_size: usize,
    samples: VecDeque<ProbeSample>,
    probes_sent: u64,
    probes_acked: u64,
    rtt_us: Option<u64>,
    last_success: Option<Instant>,
    remote_quality: Option<RemoteQuality>,
}

#[derive(Debug)]
struct TrafficWindow {
    started_at: Instant,
    last_activity_nanos: AtomicU64,
    buckets: [AtomicU64; TRAFFIC_BUCKET_COUNT],
    first_seen: [AtomicU64; TRAFFIC_BUCKET_COUNT],
}

#[derive(Debug, Clone, Copy)]
struct ProbeSample {
    timestamp: u64,
    acked: bool,
}

#[derive(Debug, Clone, Copy)]
struct RemoteQuality {
    rtt_us: u64,
    loss_rate: f32,
    bandwidth_bps: u64,
    received_at: Instant,
}

impl PeerHealth {
    pub(crate) fn new(window_size: usize) -> Self {
        Self {
            traffic: TrafficWindow::new(),
            probes: Mutex::new(ProbeHealth::new(window_size)),
        }
    }

    pub(crate) fn record_inbound(&self) {
        self.traffic.record_inbound();
    }

    /// Payload accounting is lock-free. Each packet performs a small fixed set
    /// of atomic updates for activity, first-seen, and byte buckets;
    /// probe/RTT/quality state is only touched on the low-frequency control
    /// path.
    pub(crate) fn record_payload(&self, bytes: usize) {
        self.traffic.record_payload(bytes);
    }

    pub(crate) fn record_remote_quality(&self, rtt_us: u64, loss_rate: f32, bandwidth_bps: u64) {
        self.traffic.record_inbound();
        self.probes().remote_quality = Some(RemoteQuality {
            rtt_us,
            loss_rate: loss_rate.clamp(0.0, 1.0),
            bandwidth_bps,
            received_at: Instant::now(),
        });
    }

    pub(crate) fn record_probe_sent(&self, timestamp: u64) {
        let mut probes = self.probes();
        probes.probes_sent += 1;
        probes.samples.push_back(ProbeSample {
            timestamp,
            acked: false,
        });

        while probes.samples.len() > probes.window_size {
            probes.samples.pop_front();
        }
    }

    pub(crate) fn record_probe_response(&self, timestamp: u64, rtt_us: u64) {
        self.traffic.record_inbound();
        let mut probes = self.probes();

        let mut newly_acked = false;
        if let Some(sample) = probes
            .samples
            .iter_mut()
            .rev()
            .find(|sample| sample.timestamp == timestamp)
        {
            if !sample.acked {
                sample.acked = true;
                newly_acked = true;
            }
        }

        if newly_acked {
            probes.probes_acked += 1;
        }

        probes.last_success = Some(Instant::now());
        probes.rtt_us = Some(match probes.rtt_us {
            Some(previous) => (previous * 7 + rtt_us) / 8,
            None => rtt_us,
        });
    }

    pub(crate) fn snapshot(
        &self,
        now_micros: u64,
        response_grace: Duration,
        threshold: &FallbackThreshold,
    ) -> ConnectionHealthSnapshot {
        let now_nanos = self.traffic.elapsed_nanos();
        let probes = self.probes();
        let (window_probes, window_lost, loss_rate) =
            probes.window_loss(now_micros, response_grace);
        let probe_success_rate = if window_probes == 0 {
            0.0
        } else {
            1.0 - loss_rate
        };
        let healthy = self.is_healthy_from_state(&probes, now_micros, response_grace, threshold);

        ConnectionHealthSnapshot {
            rtt_us: probes.rtt_us,
            loss_rate,
            bandwidth_bps: self.traffic.bandwidth_bps(now_nanos),
            probes_sent: probes.probes_sent,
            probes_acked: probes.probes_acked,
            window_probes,
            window_lost,
            probe_success_rate,
            last_activity_secs: self
                .traffic
                .last_activity_age_nanos(now_nanos)
                .map(|age| age / NANOS_PER_SEC),
            last_success_secs: probes
                .last_success
                .map(|instant| instant.elapsed().as_secs()),
            remote_quality: probes.remote_quality.map(|quality| quality.snapshot()),
            healthy,
        }
    }

    fn is_healthy_from_state(
        &self,
        probes: &ProbeHealth,
        now_micros: u64,
        response_grace: Duration,
        threshold: &FallbackThreshold,
    ) -> bool {
        let now_nanos = self.traffic.elapsed_nanos();
        let Some(last_activity_age) = self.traffic.last_activity_age_nanos(now_nanos) else {
            return false;
        };

        if last_activity_age > threshold.timeout_secs.saturating_mul(NANOS_PER_SEC) {
            return false;
        }

        if let Some(rtt_us) = probes.rtt_us {
            if rtt_us > threshold.max_rtt_ms.saturating_mul(1000) {
                return false;
            }
        }

        let (eligible_probes, _, loss_rate) = probes.window_loss(now_micros, response_grace);
        if eligible_probes > 0 && loss_rate > threshold.max_loss_rate {
            return false;
        }

        true
    }

    fn probes(&self) -> MutexGuard<'_, ProbeHealth> {
        self.probes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProbeHealth {
    fn new(window_size: usize) -> Self {
        Self {
            window_size: window_size.max(1),
            samples: VecDeque::with_capacity(window_size.max(1)),
            probes_sent: 0,
            probes_acked: 0,
            rtt_us: None,
            last_success: None,
            remote_quality: None,
        }
    }

    fn window_loss(&self, now_micros: u64, response_grace: Duration) -> (usize, usize, f32) {
        let grace_micros = response_grace.as_micros() as u64;
        let mut eligible = 0usize;
        let mut lost = 0usize;

        for probe in &self.samples {
            let expired = probe
                .timestamp
                .checked_add(grace_micros)
                .map(|deadline| deadline <= now_micros)
                .unwrap_or(true);

            if probe.acked || expired {
                eligible += 1;
                if !probe.acked {
                    lost += 1;
                }
            }
        }

        let loss_rate = if eligible == 0 {
            0.0
        } else {
            lost as f32 / eligible as f32
        };

        (eligible, lost, loss_rate)
    }
}

impl TrafficWindow {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            last_activity_nanos: AtomicU64::new(NO_ACTIVITY),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            first_seen: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn record_inbound(&self) {
        let now_nanos = self.elapsed_nanos();
        self.record_inbound_at(now_nanos);
    }

    fn record_payload(&self, bytes: usize) {
        let now_nanos = self.elapsed_nanos();
        self.record_payload_at(now_nanos, bytes);
    }

    fn record_payload_at(&self, now_nanos: u64, bytes: usize) {
        self.record_inbound_at(now_nanos);
        let bytes = u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .min(TRAFFIC_BUCKET_VALUE_MASK);
        self.record_bytes_at(now_nanos, bytes);
    }

    fn record_inbound_at(&self, now_nanos: u64) {
        // Store activity as `now + 1` so zero remains the no-activity sentinel
        // while fetch_max prevents a stale concurrent update from moving back.
        self.last_activity_nanos
            .fetch_max(now_nanos.saturating_add(1), Ordering::Release);
    }

    fn record_bytes_at(&self, now_nanos: u64, bytes: u64) {
        let now_secs = now_nanos / NANOS_PER_SEC;
        let generation = (now_secs as u32 as u64) << TRAFFIC_BUCKET_GENERATION_SHIFT;
        let bucket_index = now_secs as usize % TRAFFIC_BUCKET_COUNT;

        // Publish the first packet in this one-second bucket before publishing
        // bytes. A concurrent snapshot may briefly observe zero bytes, but it can
        // never attribute bytes from an earlier generation to this bucket.
        let first_value = generation | ((now_nanos % NANOS_PER_SEC) + 1);
        let first = &self.first_seen[bucket_index];
        loop {
            let current = first.load(Ordering::Relaxed);
            let already_set = current >> TRAFFIC_BUCKET_GENERATION_SHIFT == now_secs as u32 as u64
                && current & TRAFFIC_BUCKET_VALUE_MASK != 0;
            if already_set {
                break;
            }
            if first
                .compare_exchange_weak(current, first_value, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        let bucket = &self.buckets[bucket_index];
        loop {
            let current = bucket.load(Ordering::Relaxed);
            let current_bytes =
                if current >> TRAFFIC_BUCKET_GENERATION_SHIFT == now_secs as u32 as u64 {
                    current & TRAFFIC_BUCKET_VALUE_MASK
                } else {
                    0
                };
            let next_bytes = current_bytes
                .saturating_add(bytes)
                .min(TRAFFIC_BUCKET_VALUE_MASK);
            let next = generation | next_bytes;
            if bucket
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    fn elapsed_nanos(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64
    }

    fn last_activity_age_nanos(&self, now_nanos: u64) -> Option<u64> {
        let last_activity = self.last_activity_nanos.load(Ordering::Acquire);
        (last_activity != NO_ACTIVITY)
            .then(|| now_nanos.saturating_sub(last_activity.saturating_sub(1)))
    }

    fn bandwidth_bps(&self, now_nanos: u64) -> u64 {
        let Some((bytes, oldest_first_seen)) = self.traffic_in_window(now_nanos) else {
            return 0;
        };
        if bytes == 0 {
            return 0;
        }

        let elapsed_secs = now_nanos
            .saturating_sub(oldest_first_seen)
            .max(NANOS_PER_SEC)
            / NANOS_PER_SEC;
        bytes.saturating_mul(8) / elapsed_secs
    }

    /// Return bytes and the timestamp of the first retained packet. The 31
    /// one-second buckets cover the inclusive 30-second window without a
    /// per-packet collection or lock.
    fn traffic_in_window(&self, now_nanos: u64) -> Option<(u64, u64)> {
        let now_secs = now_nanos / NANOS_PER_SEC;
        let window_nanos = BANDWIDTH_WINDOW.as_nanos() as u64;
        let mut bytes = 0u64;
        let mut oldest = None;

        for age in 0..=TRAFFIC_WINDOW_SECS {
            let Some(secs) = now_secs.checked_sub(age) else {
                break;
            };
            let index = secs as usize % TRAFFIC_BUCKET_COUNT;
            let first = self.first_seen[index].load(Ordering::Relaxed);
            if first >> TRAFFIC_BUCKET_GENERATION_SHIFT != secs as u32 as u64
                || first & TRAFFIC_BUCKET_VALUE_MASK == 0
            {
                continue;
            }

            let offset_nanos = (first & TRAFFIC_BUCKET_VALUE_MASK) - 1;
            let first_seen = secs.saturating_mul(NANOS_PER_SEC) + offset_nanos;
            if now_nanos.saturating_sub(first_seen) > window_nanos {
                continue;
            }

            let bucket = self.buckets[index].load(Ordering::Relaxed);
            if bucket >> TRAFFIC_BUCKET_GENERATION_SHIFT != secs as u32 as u64
                || bucket & TRAFFIC_BUCKET_VALUE_MASK == 0
            {
                continue;
            }
            bytes = bytes.saturating_add(bucket & TRAFFIC_BUCKET_VALUE_MASK);
            oldest = Some(oldest.map_or(first_seen, |current: u64| current.min(first_seen)));
        }

        oldest.map(|oldest| (bytes, oldest))
    }
}

impl RemoteQuality {
    fn snapshot(self) -> RemoteQualitySnapshot {
        RemoteQualitySnapshot {
            rtt_us: self.rtt_us,
            loss_rate: self.loss_rate,
            bandwidth_bps: self.bandwidth_bps,
            age_secs: self.received_at.elapsed().as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::thread;

    fn threshold() -> FallbackThreshold {
        FallbackThreshold {
            max_rtt_ms: 200,
            max_loss_rate: 0.25,
            timeout_secs: 30,
        }
    }

    #[test]
    fn records_rtt_and_successful_probe() {
        let health = PeerHealth::new(8);
        health.record_probe_sent(1_000);
        health.record_probe_response(1_000, 12_000);

        let snapshot = health.snapshot(2_000, Duration::from_millis(1), &threshold());

        assert_eq!(snapshot.rtt_us, Some(12_000));
        assert_eq!(snapshot.bandwidth_bps, 0);
        assert_eq!(snapshot.probes_sent, 1);
        assert_eq!(snapshot.probes_acked, 1);
        assert_eq!(snapshot.window_lost, 0);
        assert_eq!(snapshot.probe_success_rate, 1.0);
        assert!(snapshot.healthy);
    }

    #[test]
    fn loss_rate_ignores_unexpired_probes() {
        let health = PeerHealth::new(8);
        health.record_inbound();
        health.record_probe_sent(1_000);
        health.record_probe_sent(2_000);
        health.record_probe_response(1_000, 10_000);

        let fresh = health.snapshot(2_500, Duration::from_millis(2), &threshold());
        assert_eq!(fresh.window_probes, 1);
        assert_eq!(fresh.window_lost, 0);
        assert_eq!(fresh.loss_rate, 0.0);

        let expired = health.snapshot(5_000, Duration::from_millis(2), &threshold());
        assert_eq!(expired.window_probes, 2);
        assert_eq!(expired.window_lost, 1);
        assert_eq!(expired.loss_rate, 0.5);
        assert_eq!(expired.probe_success_rate, 0.5);
        assert!(!expired.healthy);
    }

    #[test]
    fn estimates_bandwidth_from_payload_window() {
        let health = PeerHealth::new(8);
        health.record_payload(10_000);

        let snapshot = health.snapshot(10_000, Duration::from_millis(1), &threshold());

        assert_eq!(snapshot.bandwidth_bps, 80_000);
        assert!(snapshot.healthy);
    }

    #[test]
    fn inbound_activity_does_not_move_backwards_when_updates_arrive_out_of_order() {
        let traffic = TrafficWindow::new();

        traffic.record_inbound_at(2 * NANOS_PER_SEC);
        traffic.record_inbound_at(NANOS_PER_SEC);

        assert_eq!(
            traffic.last_activity_age_nanos(3 * NANOS_PER_SEC),
            Some(NANOS_PER_SEC)
        );
    }

    #[test]
    fn bandwidth_bucket_saturates_at_u32_max_without_wrapping() {
        let traffic = TrafficWindow::new();
        let now_nanos = TRAFFIC_WINDOW_SECS * NANOS_PER_SEC + 500_000_000;

        traffic.record_payload_at(now_nanos, (u32::MAX - 1) as usize);
        traffic.record_payload_at(now_nanos, 10);

        let (bytes, _) = traffic
            .traffic_in_window(now_nanos)
            .expect("saturated payload bucket must remain visible");
        assert_eq!(bytes, u32::MAX as u64);
    }

    #[test]
    fn concurrent_payload_updates_are_not_lost() {
        const THREADS: usize = 8;
        const UPDATES_PER_THREAD: usize = 10_000;
        let health = Arc::new(PeerHealth::new(8));
        let barrier = Arc::new(std::sync::Barrier::new(THREADS));
        let mut workers = Vec::new();

        for _ in 0..THREADS {
            let health = health.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                for _ in 0..UPDATES_PER_THREAD {
                    health.record_payload(1);
                }
            }));
        }
        for worker in workers {
            worker.join().expect("payload worker must not panic");
        }

        let now_nanos = health.traffic.elapsed_nanos();
        let (bytes, _) = health
            .traffic
            .traffic_in_window(now_nanos)
            .expect("payload updates must be retained");
        assert_eq!(bytes, (THREADS * UPDATES_PER_THREAD) as u64);
    }

    #[test]
    fn payload_path_does_not_wait_for_probe_state() {
        let health = Arc::new(PeerHealth::new(8));
        let probe_guard = health.probes();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_health = health.clone();
        let worker = thread::spawn(move || {
            worker_health.record_payload(64);
            done_tx.send(()).expect("payload completion channel");
        });

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("payload accounting must not block on probe state");
        drop(probe_guard);
        worker.join().expect("payload worker must not panic");
        assert_eq!(
            health
                .snapshot(10_000, Duration::from_millis(1), &threshold())
                .bandwidth_bps,
            512
        );
    }

    #[test]
    fn snapshot_and_probe_updates_remain_consistent_under_payload_load() {
        let health = Arc::new(PeerHealth::new(8));
        health.record_probe_sent(1_000);
        let health_for_payload = health.clone();
        let payload_worker = thread::spawn(move || {
            for _ in 0..10_000 {
                health_for_payload.record_payload(64);
            }
        });
        let health_for_probe = health.clone();
        let probe_worker = thread::spawn(move || {
            for _ in 0..1_000 {
                health_for_probe.record_probe_response(1_000, 10_000);
            }
        });

        for _ in 0..1_000 {
            let _ = health.snapshot(10_000, Duration::from_millis(1), &threshold());
        }
        payload_worker
            .join()
            .expect("payload worker must not panic");
        probe_worker.join().expect("probe worker must not panic");

        let snapshot = health.snapshot(10_000, Duration::from_millis(1), &threshold());
        assert_eq!(snapshot.rtt_us, Some(10_000));
        assert_eq!(snapshot.probes_acked, 1);
    }

    #[test]
    fn records_remote_quality_report() {
        let health = PeerHealth::new(8);
        health.record_remote_quality(20_000, 0.25, 100_000);

        let snapshot = health.snapshot(10_000, Duration::from_millis(1), &threshold());
        let remote = snapshot
            .remote_quality
            .expect("remote quality report should be present");

        assert_eq!(remote.rtt_us, 20_000);
        assert_eq!(remote.loss_rate, 0.25);
        assert_eq!(remote.bandwidth_bps, 100_000);
    }
}
