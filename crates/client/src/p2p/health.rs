use fuckipv6_common::FallbackThreshold;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const BANDWIDTH_WINDOW: Duration = Duration::from_secs(30);

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

#[derive(Debug, Clone)]
pub(crate) struct PeerHealth {
    window_size: usize,
    probes: VecDeque<ProbeSample>,
    traffic_samples: VecDeque<TrafficSample>,
    probes_sent: u64,
    probes_acked: u64,
    rtt_us: Option<u64>,
    bandwidth_bps: u64,
    last_activity: Option<Instant>,
    last_success: Option<Instant>,
    remote_quality: Option<RemoteQuality>,
}

#[derive(Debug, Clone, Copy)]
struct ProbeSample {
    timestamp: u64,
    acked: bool,
}

#[derive(Debug, Clone, Copy)]
struct TrafficSample {
    timestamp: Instant,
    bytes: usize,
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
            window_size: window_size.max(1),
            probes: VecDeque::with_capacity(window_size.max(1)),
            traffic_samples: VecDeque::new(),
            probes_sent: 0,
            probes_acked: 0,
            rtt_us: None,
            bandwidth_bps: 0,
            last_activity: None,
            last_success: None,
            remote_quality: None,
        }
    }

    pub(crate) fn record_inbound(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    pub(crate) fn record_payload(&mut self, bytes: usize) {
        self.record_inbound();
        let now = Instant::now();
        self.traffic_samples.push_back(TrafficSample {
            timestamp: now,
            bytes,
        });
        self.prune_traffic(now);
        self.update_bandwidth(now);
    }

    pub(crate) fn record_remote_quality(
        &mut self,
        rtt_us: u64,
        loss_rate: f32,
        bandwidth_bps: u64,
    ) {
        self.record_inbound();
        self.remote_quality = Some(RemoteQuality {
            rtt_us,
            loss_rate: loss_rate.clamp(0.0, 1.0),
            bandwidth_bps,
            received_at: Instant::now(),
        });
    }

    pub(crate) fn record_probe_sent(&mut self, timestamp: u64) {
        self.probes_sent += 1;
        self.probes.push_back(ProbeSample {
            timestamp,
            acked: false,
        });

        while self.probes.len() > self.window_size {
            self.probes.pop_front();
        }
    }

    pub(crate) fn record_probe_response(&mut self, timestamp: u64, rtt_us: u64) {
        self.record_inbound();

        let mut newly_acked = false;
        if let Some(sample) = self
            .probes
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
            self.probes_acked += 1;
        }

        self.last_success = Some(Instant::now());
        self.rtt_us = Some(match self.rtt_us {
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
        let (window_probes, window_lost, loss_rate) = self.window_loss(now_micros, response_grace);
        let probe_success_rate = if window_probes == 0 {
            0.0
        } else {
            1.0 - loss_rate
        };
        let healthy = self.is_healthy(now_micros, response_grace, threshold);

        ConnectionHealthSnapshot {
            rtt_us: self.rtt_us,
            loss_rate,
            bandwidth_bps: self.bandwidth_bps,
            probes_sent: self.probes_sent,
            probes_acked: self.probes_acked,
            window_probes,
            window_lost,
            probe_success_rate,
            last_activity_secs: self
                .last_activity
                .map(|instant| instant.elapsed().as_secs()),
            last_success_secs: self.last_success.map(|instant| instant.elapsed().as_secs()),
            remote_quality: self.remote_quality.map(|quality| quality.snapshot()),
            healthy,
        }
    }

    pub(crate) fn is_healthy(
        &self,
        now_micros: u64,
        response_grace: Duration,
        threshold: &FallbackThreshold,
    ) -> bool {
        let Some(last_activity) = self.last_activity else {
            return false;
        };

        if last_activity.elapsed() > Duration::from_secs(threshold.timeout_secs) {
            return false;
        }

        if let Some(rtt_us) = self.rtt_us {
            if rtt_us > threshold.max_rtt_ms.saturating_mul(1000) {
                return false;
            }
        }

        let (eligible_probes, _, loss_rate) = self.window_loss(now_micros, response_grace);
        if eligible_probes > 0 && loss_rate > threshold.max_loss_rate {
            return false;
        }

        true
    }

    fn window_loss(&self, now_micros: u64, response_grace: Duration) -> (usize, usize, f32) {
        let grace_micros = response_grace.as_micros() as u64;
        let mut eligible = 0usize;
        let mut lost = 0usize;

        for probe in &self.probes {
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

    fn prune_traffic(&mut self, now: Instant) {
        while self
            .traffic_samples
            .front()
            .map(|sample| now.duration_since(sample.timestamp) > BANDWIDTH_WINDOW)
            .unwrap_or(false)
        {
            self.traffic_samples.pop_front();
        }
    }

    fn update_bandwidth(&mut self, now: Instant) {
        let Some(first) = self.traffic_samples.front() else {
            self.bandwidth_bps = 0;
            return;
        };

        let total_bytes = self
            .traffic_samples
            .iter()
            .map(|sample| sample.bytes as u64)
            .sum::<u64>();
        let elapsed = now
            .duration_since(first.timestamp)
            .max(Duration::from_secs(1));
        self.bandwidth_bps = total_bytes.saturating_mul(8) / elapsed.as_secs();
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

    fn threshold() -> FallbackThreshold {
        FallbackThreshold {
            max_rtt_ms: 200,
            max_loss_rate: 0.25,
            timeout_secs: 30,
        }
    }

    #[test]
    fn records_rtt_and_successful_probe() {
        let mut health = PeerHealth::new(8);
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
        let mut health = PeerHealth::new(8);
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
        let mut health = PeerHealth::new(8);
        health.record_payload(10_000);

        let snapshot = health.snapshot(10_000, Duration::from_millis(1), &threshold());

        assert_eq!(snapshot.bandwidth_bps, 80_000);
        assert!(snapshot.healthy);
    }

    #[test]
    fn records_remote_quality_report() {
        let mut health = PeerHealth::new(8);
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
