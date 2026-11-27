//! Android implementation of [`EmbeddedPlatform`].
//!
//! Bridges the authoritative Rust [`EmbeddedNodeRuntime`] to Kotlin's
//! `FuckIpv6VpnService` through [`AndroidHostCallbacks`], owning each active
//! TUN packet pump and guaranteeing idempotent teardown on close, cancel, or
//! runtime shutdown.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fuckipv6_client::embedded::{
    EmbeddedNetwork, EmbeddedNetworkLease, EmbeddedNetworkRequest, EmbeddedPlatform,
};
use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use fuckipv6_client::packet_io::ExternalPacketDevice;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::settings::AndroidTunnelSettings;
use crate::tun_pump::{TunnelCounters, TunnelCountersSnapshot};

#[cfg(unix)]
use crate::tun_pump::TunPump;

#[cfg(unix)]
type PumpSlot = Arc<Mutex<Option<TunPump>>>;
#[cfg(unix)]
type ActivePumps = Arc<Mutex<HashMap<String, PumpSlot>>>;

pub const PACKET_QUEUE_CAPACITY: usize = 256;

/// Platform callback surface implemented by JNI on Android and fakes in tests.
pub trait AndroidHostCallbacks: Send + Sync + 'static {
    /// Applies `settings_json` via `VpnService.Builder` and returns the detached
    /// non-negative TUN file descriptor.
    fn open_network(&self, settings_json: &str) -> Result<i32, NodeProblem>;

    /// Tears down Android-side VPN session metadata for `network_name`.
    fn close_network(&self, network_name: &str) -> Result<(), NodeProblem>;
}

pub struct AndroidEmbeddedPlatform {
    host: Arc<dyn AndroidHostCallbacks>,
    handle: Handle,
    counters: Arc<TunnelCounters>,
    active_sessions: Arc<AtomicUsize>,
    #[cfg(unix)]
    active_pumps: ActivePumps,
    closed: AtomicBool,
}

impl AndroidEmbeddedPlatform {
    pub fn new(host: Arc<dyn AndroidHostCallbacks>, handle: Handle) -> Arc<Self> {
        Arc::new(Self {
            host,
            handle,
            counters: Arc::new(TunnelCounters::default()),
            active_sessions: Arc::new(AtomicUsize::new(0)),
            #[cfg(unix)]
            active_pumps: Arc::new(Mutex::new(HashMap::new())),
            closed: AtomicBool::new(false),
        })
    }

    pub fn counters_snapshot(&self) -> TunnelCountersSnapshot {
        self.counters.snapshot()
    }

    pub fn active_session_count(&self) -> usize {
        self.active_sessions.load(Ordering::Acquire)
    }
}

impl EmbeddedPlatform for AndroidEmbeddedPlatform {
    fn open_network(
        &self,
        request: EmbeddedNetworkRequest,
    ) -> BoxFuture<'_, Result<EmbeddedNetwork, NodeProblem>> {
        Box::pin(async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "Android VPN host is shutting down",
                ));
            }
            let settings = AndroidTunnelSettings::derive(&request)?;
            let settings_json = serde_json::to_string(&settings).map_err(|error| {
                NodeProblem::new(
                    NodeProblemCode::Internal,
                    format!("failed to serialize Android tunnel settings: {error}"),
                )
            })?;

            let (device, endpoint) = ExternalPacketDevice::new(
                request.config.tun.name.clone(),
                settings.mtu,
                PACKET_QUEUE_CAPACITY,
            )
            .map_err(|error| {
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("failed to allocate Android packet device: {error}"),
                )
            })?;

            let host = self.host.clone();
            let network_name = settings.network.clone();
            let json_for_host = settings_json.clone();
            let tun_fd = tokio::task::spawn_blocking(move || host.open_network(&json_for_host))
                .await
                .map_err(|error| {
                    NodeProblem::new(
                        NodeProblemCode::Internal,
                        format!("Android open_network task panicked: {error}"),
                    )
                })??;

            #[cfg(unix)]
            let pump_slot = {
                let pump = match TunPump::start(
                    tun_fd,
                    settings.mtu,
                    endpoint,
                    self.counters.clone(),
                    self.handle.clone(),
                ) {
                    Ok(pump) => pump,
                    Err(error) => {
                        let rollback_host = self.host.clone();
                        let rollback_net = network_name.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = rollback_host.close_network(&rollback_net);
                        })
                        .await;
                        return Err(NodeProblem::new(
                            NodeProblemCode::Unavailable,
                            format!("failed to start Android TUN pump: {error}"),
                        ));
                    }
                };
                let slot = Arc::new(Mutex::new(Some(pump)));
                let previous = self
                    .active_pumps
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(network_name.clone(), slot.clone());
                if let Some(prev_slot) = previous {
                    let prev_pump = prev_slot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(prev_pump) = prev_pump {
                        prev_pump.stop().await;
                    }
                }
                slot
            };

            #[cfg(not(unix))]
            let _ = (tun_fd, endpoint);

            self.active_sessions.fetch_add(1, Ordering::AcqRel);

            let lease = AndroidNetworkLease {
                network_name,
                host: self.host.clone(),
                handle: self.handle.clone(),
                active_sessions: self.active_sessions.clone(),
                #[cfg(unix)]
                pump_slot,
                #[cfg(unix)]
                active_pumps: self.active_pumps.clone(),
                released: AtomicBool::new(false),
            };
            Ok(EmbeddedNetwork::new(device, Box::new(lease)))
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async move {
            self.closed.store(true, Ordering::Release);
            #[cfg(unix)]
            {
                let entries: Vec<(String, Arc<Mutex<Option<TunPump>>>)> = {
                    let mut guard = self
                        .active_pumps
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.drain().collect()
                };
                for (network_name, slot) in entries {
                    let pump = slot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(pump) = pump {
                        pump.stop().await;
                    }
                    let host = self.host.clone();
                    let _ = tokio::task::spawn_blocking(move || host.close_network(&network_name))
                        .await;
                }
            }
            self.active_sessions.store(0, Ordering::Release);
            Ok(())
        })
    }
}

struct AndroidNetworkLease {
    network_name: String,
    host: Arc<dyn AndroidHostCallbacks>,
    handle: Handle,
    active_sessions: Arc<AtomicUsize>,
    #[cfg(unix)]
    pump_slot: PumpSlot,
    #[cfg(unix)]
    active_pumps: ActivePumps,
    released: AtomicBool,
}

impl EmbeddedNetworkLease for AndroidNetworkLease {
    fn close(&mut self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        let already = self.released.swap(true, Ordering::SeqCst);
        let network_name = self.network_name.clone();
        let host = self.host.clone();
        let active_sessions = self.active_sessions.clone();
        #[cfg(unix)]
        let pump_slot = self.pump_slot.clone();
        #[cfg(unix)]
        let active_pumps = self.active_pumps.clone();

        Box::pin(async move {
            if already {
                return Ok(());
            }
            #[cfg(unix)]
            {
                let pump = pump_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(pump) = pump {
                    pump.stop().await;
                }
                let mut map = active_pumps
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if map
                    .get(&network_name)
                    .is_some_and(|existing| Arc::ptr_eq(existing, &pump_slot))
                {
                    map.remove(&network_name);
                }
            }
            decrement_saturating(&active_sessions);
            tokio::task::spawn_blocking(move || host.close_network(&network_name))
                .await
                .map_err(|error| {
                    NodeProblem::new(
                        NodeProblemCode::Internal,
                        format!("Android close_network task panicked: {error}"),
                    )
                })?
        })
    }

    fn cancel(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        #[cfg(unix)]
        {
            if let Ok(guard) = self.pump_slot.lock() {
                if let Some(pump) = guard.as_ref() {
                    pump.signal_stop();
                }
            }
        }
        let host = self.host.clone();
        let network_name = self.network_name.clone();
        let active_sessions = self.active_sessions.clone();
        #[cfg(unix)]
        let pump_slot = self.pump_slot.clone();
        self.handle.spawn(async move {
            #[cfg(unix)]
            {
                let pump = pump_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(pump) = pump {
                    pump.stop().await;
                }
            }
            decrement_saturating(&active_sessions);
            let _ = tokio::task::spawn_blocking(move || host.close_network(&network_name)).await;
        });
    }
}

fn decrement_saturating(counter: &AtomicUsize) {
    let mut current = counter.load(Ordering::Acquire);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}
