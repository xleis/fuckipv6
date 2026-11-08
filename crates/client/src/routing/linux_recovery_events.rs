//! Linux suspend/resume and network-change event sources.
//!
//! The listeners only translate OS notifications into [`RecoveryTrigger`]
//! values. Recovery sequencing and resource mutation stay in the coordinator
//! and its platform backend. Missing desktop services are reported as
//! unavailable and retried instead of being treated as a successful listener.

use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use zbus::{Connection, Proxy};

use super::recovery::RecoveryTrigger;

const LOGIND_SERVICE: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIND_SLEEP_SIGNAL: &str = "PrepareForSleep";

const NETWORK_MANAGER_SERVICE: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
const NETWORK_MANAGER_INTERFACE: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_STATE_SIGNAL: &str = "StateChanged";

const LISTENER_RETRY_DELAY: Duration = Duration::from_secs(5);

/// A platform event source whose availability matters for honest recovery
/// diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxRecoveryEventSource {
    Logind,
    NetworkManager,
}

impl LinuxRecoveryEventSource {
    fn label(self) -> &'static str {
        match self {
            Self::Logind => "systemd-logind",
            Self::NetworkManager => "NetworkManager",
        }
    }
}

/// Events emitted by the Linux recovery listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxRecoveryEvent {
    Trigger(RecoveryTrigger),
    SourceAvailable(LinuxRecoveryEventSource),
    SourceUnavailable(LinuxRecoveryEventSource),
}

/// Owns the background Linux D-Bus listeners and their bounded event channel.
pub struct LinuxRecoveryEventWatcher {
    events: mpsc::UnboundedReceiver<LinuxRecoveryEvent>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl LinuxRecoveryEventWatcher {
    /// Start logind and NetworkManager listeners without blocking runtime
    /// startup. Each source reconnects independently if its D-Bus service or
    /// the system bus becomes unavailable.
    pub fn spawn() -> Self {
        let (events_tx, events) = mpsc::unbounded_channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let tasks = vec![
            spawn_source_supervisor(
                LinuxRecoveryEventSource::Logind,
                events_tx.clone(),
                shutdown_rx.clone(),
            ),
            spawn_source_supervisor(
                LinuxRecoveryEventSource::NetworkManager,
                events_tx,
                shutdown_rx,
            ),
        ];
        Self {
            events,
            shutdown,
            tasks,
        }
    }

    pub async fn recv(&mut self) -> Option<LinuxRecoveryEvent> {
        self.events.recv().await
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for LinuxRecoveryEventWatcher {
    fn drop(&mut self) {
        self.shutdown();
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn spawn_source_supervisor(
    source: LinuxRecoveryEventSource,
    events: mpsc::UnboundedSender<LinuxRecoveryEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                return;
            }

            let result = match source {
                LinuxRecoveryEventSource::Logind => {
                    listen_for_sleep_wake(&events, &mut shutdown).await
                }
                LinuxRecoveryEventSource::NetworkManager => {
                    listen_for_network_changes(&events, &mut shutdown).await
                }
            };

            if let Err(error) = result {
                let _ = events.send(LinuxRecoveryEvent::SourceUnavailable(source));
                tracing::warn!(
                    "Linux 恢复事件源 {} 不可用，将在 {:?} 后重试: {error:#}",
                    source.label(),
                    LISTENER_RETRY_DELAY
                );
            }

            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                _ = sleep(LISTENER_RETRY_DELAY) => {}
            }
        }
    })
}

async fn listen_for_sleep_wake(
    events: &mpsc::UnboundedSender<LinuxRecoveryEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    let connection = Connection::system()
        .await
        .context("无法连接 system D-Bus")?;
    let proxy = Proxy::new(&connection, LOGIND_SERVICE, LOGIND_PATH, LOGIND_INTERFACE)
        .await
        .context("无法创建 systemd-logind D-Bus 代理")?;
    let mut signals = proxy
        .receive_signal(LOGIND_SLEEP_SIGNAL)
        .await
        .context("无法订阅 systemd-logind PrepareForSleep 信号")?;
    let _ = events.send(LinuxRecoveryEvent::SourceAvailable(
        LinuxRecoveryEventSource::Logind,
    ));

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            message = signals.next() => {
                let message = message.context("systemd-logind PrepareForSleep 信号流已结束")?;
                let sleeping = message
                    .body()
                    .deserialize::<bool>()
                    .context("systemd-logind PrepareForSleep 参数无效")?;
                if let Some(trigger) = trigger_for_sleep_transition(sleeping) {
                    let _ = events.send(LinuxRecoveryEvent::Trigger(trigger));
                }
            }
        }
    }
}

async fn listen_for_network_changes(
    events: &mpsc::UnboundedSender<LinuxRecoveryEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    let connection = Connection::system()
        .await
        .context("无法连接 system D-Bus")?;
    let proxy = Proxy::new(
        &connection,
        NETWORK_MANAGER_SERVICE,
        NETWORK_MANAGER_PATH,
        NETWORK_MANAGER_INTERFACE,
    )
    .await
    .context("无法创建 NetworkManager D-Bus 代理")?;
    let mut signals = proxy
        .receive_signal(NETWORK_MANAGER_STATE_SIGNAL)
        .await
        .context("无法订阅 NetworkManager StateChanged 信号")?;
    let _ = events.send(LinuxRecoveryEvent::SourceAvailable(
        LinuxRecoveryEventSource::NetworkManager,
    ));

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            message = signals.next() => {
                let message = message.context("NetworkManager StateChanged 信号流已结束")?;
                let state = message
                    .body()
                    .deserialize::<u32>()
                    .context("NetworkManager StateChanged 参数无效")?;
                if let Some(trigger) = trigger_for_network_state(state) {
                    let _ = events.send(LinuxRecoveryEvent::Trigger(trigger));
                }
            }
        }
    }
}

fn trigger_for_sleep_transition(sleeping: bool) -> Option<RecoveryTrigger> {
    (!sleeping).then_some(RecoveryTrigger::SleepWake)
}

fn trigger_for_network_state(state: u32) -> Option<RecoveryTrigger> {
    // NetworkManager: local/site/global/VPN connected. Intermediate connecting
    // and disconnected states are not stable enough to reconcile against.
    matches!(state, 50 | 60 | 70 | 90).then_some(RecoveryTrigger::NetworkChanged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_resume_transition_requests_recovery() {
        assert_eq!(trigger_for_sleep_transition(true), None);
        assert_eq!(
            trigger_for_sleep_transition(false),
            Some(RecoveryTrigger::SleepWake)
        );
    }

    #[test]
    fn stable_connected_network_states_request_recovery() {
        for state in [50, 60, 70, 90] {
            assert_eq!(
                trigger_for_network_state(state),
                Some(RecoveryTrigger::NetworkChanged),
                "state {state} should reconcile"
            );
        }
        for state in [0, 10, 20, 30, 40, 80, 100, 110, 120, u32::MAX] {
            assert_eq!(
                trigger_for_network_state(state),
                None,
                "state {state} must not preempt a stable reconciliation"
            );
        }
    }
}
