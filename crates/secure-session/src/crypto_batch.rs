//! Bounded CPU worker dispatch for multi-packet AEAD operations.
//!
//! Rayon owns the persistent worker threads. A separate in-flight gate ensures
//! that concurrent data-plane callers cannot enqueue an unbounded number of
//! batch jobs: when all slots are occupied the caller uses the serial path,
//! which is safe and observable through [`CryptoBatchMetrics`].

use rayon::ThreadPool;
use rayon::ThreadPoolBuilder;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_CONFIGURED_WORKERS: usize = 256;
const DEFAULT_MAX_WORKERS: usize = 8;

static CONFIGURED_WORKERS: AtomicUsize = AtomicUsize::new(0);
static CONFIGURED: AtomicBool = AtomicBool::new(false);
static WORKER_INITIALIZED: AtomicBool = AtomicBool::new(false);
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Snapshot of crypto-worker usage and overload behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CryptoBatchMetrics {
    pub worker_threads: usize,
    pub max_in_flight_jobs: usize,
    pub parallel_jobs: u64,
    pub parallel_packets: u64,
    pub serial_jobs: u64,
    pub overloaded_jobs: u64,
    pub failed_worker_initializations: u64,
}

#[derive(Default)]
struct Metrics {
    parallel_jobs: AtomicU64,
    parallel_packets: AtomicU64,
    serial_jobs: AtomicU64,
    overloaded_jobs: AtomicU64,
}

struct CryptoWorker {
    pool: ThreadPool,
    worker_threads: usize,
    max_in_flight_jobs: usize,
    in_flight: Arc<AtomicUsize>,
    metrics: Metrics,
}

impl CryptoWorker {
    fn new(requested_workers: usize) -> Result<Self, String> {
        let available = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let worker_threads = if requested_workers == 0 {
            available.saturating_sub(1).clamp(1, DEFAULT_MAX_WORKERS)
        } else if requested_workers <= MAX_CONFIGURED_WORKERS {
            requested_workers
        } else {
            return Err(format!(
                "crypto worker count must be 0 or 1..={MAX_CONFIGURED_WORKERS}, got {requested_workers}"
            ));
        };
        let pool = ThreadPoolBuilder::new()
            .num_threads(worker_threads)
            .thread_name(|index| format!("fuckipv6-crypto-{index}"))
            .build()
            .map_err(|error| format!("failed to build crypto worker pool: {error}"))?;
        Ok(Self {
            pool,
            worker_threads,
            max_in_flight_jobs: worker_threads,
            in_flight: Arc::new(AtomicUsize::new(0)),
            metrics: Metrics::default(),
        })
    }
}

static WORKER: OnceLock<Option<CryptoWorker>> = OnceLock::new();
static INITIALIZATION_FAILED: AtomicBool = AtomicBool::new(false);

/// Configure the process-wide data-plane crypto worker count.
///
/// `0` selects `min(available_parallelism - 1, 8)`. A non-zero value is bounded
/// to `1..=256`. Configuration is process-global and first-call-wins because
/// all peers share one persistent pool. Invalid or late configuration is
/// reported to the caller but never changes established worker state.
pub fn configure_crypto_workers(requested_workers: usize) -> Result<(), String> {
    if requested_workers > MAX_CONFIGURED_WORKERS {
        return Err(format!(
            "crypto worker count must be 0 or 1..={MAX_CONFIGURED_WORKERS}, got {requested_workers}"
        ));
    }
    let _config = CONFIG_LOCK
        .lock()
        .map_err(|_| "crypto worker configuration lock is poisoned".to_string())?;
    if WORKER_INITIALIZED.load(Ordering::Acquire) {
        return Err("crypto worker pool was initialized before configuration".to_string());
    }
    if CONFIGURED.load(Ordering::Acquire) {
        return if CONFIGURED_WORKERS.load(Ordering::Acquire) == requested_workers {
            Ok(())
        } else {
            Err("crypto worker pool configuration was already set".to_string())
        };
    }
    CONFIGURED_WORKERS.store(requested_workers, Ordering::Release);
    CONFIGURED.store(true, Ordering::Release);
    Ok(())
}

fn worker() -> Option<&'static CryptoWorker> {
    WORKER
        .get_or_init(|| {
            let _config = CONFIG_LOCK.lock().ok();
            WORKER_INITIALIZED.store(true, Ordering::Release);
            let requested = CONFIGURED_WORKERS.load(Ordering::Acquire);
            match CryptoWorker::new(requested) {
                Ok(worker) => Some(worker),
                Err(error) => {
                    INITIALIZATION_FAILED.store(true, Ordering::Release);
                    let _ = error;
                    None
                }
            }
        })
        .as_ref()
}

/// Try to execute one batch job on the persistent pool.
///
/// Returns `None` when initialization failed or every in-flight slot is busy.
/// The caller must then execute the same operation serially.
pub(crate) fn try_run<R: Send>(job: impl FnOnce() -> R + Send) -> Option<R> {
    let worker = worker()?;
    try_run_on(worker, job)
}

fn try_run_on<R: Send>(worker: &CryptoWorker, job: impl FnOnce() -> R + Send) -> Option<R> {
    acquire_slot(worker)?;
    struct Permit<'a>(&'a AtomicUsize);
    impl Drop for Permit<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let _permit = Permit(&worker.in_flight);
    Some(worker.pool.install(job))
}

fn acquire_slot(worker: &CryptoWorker) -> Option<()> {
    let mut current = worker.in_flight.load(Ordering::Acquire);
    loop {
        if current >= worker.max_in_flight_jobs {
            worker
                .metrics
                .overloaded_jobs
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        match worker.in_flight.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Some(()),
            Err(observed) => current = observed,
        }
    }
}

pub(crate) fn record_parallel(packets: usize) {
    if let Some(worker) = WORKER.get().and_then(Option::as_ref) {
        worker.metrics.parallel_jobs.fetch_add(1, Ordering::Relaxed);
        worker
            .metrics
            .parallel_packets
            .fetch_add(packets as u64, Ordering::Relaxed);
    }
}

pub(crate) fn record_serial() {
    if let Some(worker) = WORKER.get().and_then(Option::as_ref) {
        worker.metrics.serial_jobs.fetch_add(1, Ordering::Relaxed);
    }
}

/// Return current worker usage, or zeroes if initialization has not happened.
pub fn crypto_batch_metrics() -> CryptoBatchMetrics {
    let failed = INITIALIZATION_FAILED.load(Ordering::Acquire);
    let Some(worker) = WORKER.get().and_then(Option::as_ref) else {
        return CryptoBatchMetrics {
            failed_worker_initializations: u64::from(failed),
            ..CryptoBatchMetrics::default()
        };
    };
    CryptoBatchMetrics {
        worker_threads: worker.worker_threads,
        max_in_flight_jobs: worker.max_in_flight_jobs,
        parallel_jobs: worker.metrics.parallel_jobs.load(Ordering::Relaxed),
        parallel_packets: worker.metrics.parallel_packets.load(Ordering::Relaxed),
        serial_jobs: worker.metrics.serial_jobs.load(Ordering::Relaxed),
        overloaded_jobs: worker.metrics.overloaded_jobs.load(Ordering::Relaxed),
        failed_worker_initializations: u64::from(failed),
    }
}

pub(crate) fn parallel_threshold_reached(packet_count: usize, plaintext_bytes: usize) -> bool {
    packet_count >= 4 && plaintext_bytes >= 4096
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_in_flight_gate_falls_back_and_records_overload() {
        let worker = CryptoWorker::new(1).expect("create isolated crypto worker");
        worker.in_flight.store(1, Ordering::Release);

        assert_eq!(try_run_on(&worker, || 7usize), None);
        assert_eq!(worker.metrics.overloaded_jobs.load(Ordering::Relaxed), 1);

        worker.in_flight.store(0, Ordering::Release);
        assert_eq!(try_run_on(&worker, || 9usize), Some(9));
        assert_eq!(worker.in_flight.load(Ordering::Acquire), 0);
    }
}
