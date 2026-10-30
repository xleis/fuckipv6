use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

/// Node-global lifecycle. Network sessions may come and go while the node
/// remains in one of these phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePhase {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// Shared lifecycle snapshot consumed by CLI, IPC, WebUI, and (later) GPUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLifecycleSnapshot {
    pub phase: NodePhase,
    pub configured_networks: usize,
    pub active_networks: usize,
    pub failed_networks: Vec<String>,
}

impl Default for NodeLifecycleSnapshot {
    fn default() -> Self {
        Self {
            phase: NodePhase::Stopped,
            configured_networks: 0,
            active_networks: 0,
            failed_networks: Vec::new(),
        }
    }
}

/// Single source of truth for node lifecycle state.
#[derive(Clone)]
pub struct NodeLifecycle {
    snapshot: Arc<RwLock<NodeLifecycleSnapshot>>,
    events: watch::Sender<NodeLifecycleSnapshot>,
}

impl NodeLifecycle {
    pub fn new() -> Self {
        let snapshot = NodeLifecycleSnapshot::default();
        let (events, _) = watch::channel(snapshot.clone());
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            events,
        }
    }

    pub async fn snapshot(&self) -> NodeLifecycleSnapshot {
        self.snapshot.read().await.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<NodeLifecycleSnapshot> {
        self.events.subscribe()
    }

    pub async fn begin_start(
        &self,
        configured_networks: usize,
        failed_networks: Vec<String>,
    ) -> NodeLifecycleSnapshot {
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Starting,
            configured_networks,
            active_networks: 0,
            failed_networks: normalize_failures(failed_networks),
        })
        .await
    }

    pub async fn running(
        &self,
        active_networks: usize,
        failed_networks: Vec<String>,
    ) -> NodeLifecycleSnapshot {
        let configured_networks = self.snapshot.read().await.configured_networks;
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Running,
            configured_networks,
            active_networks,
            failed_networks: normalize_failures(failed_networks),
        })
        .await
    }

    /// Update lifecycle counts while a live node accepts dynamic network
    /// changes. The phase remains `Running`; only the configured/active/failed
    /// projections change.
    pub async fn running_with_configuration(
        &self,
        configured_networks: usize,
        active_networks: usize,
        failed_networks: Vec<String>,
    ) -> NodeLifecycleSnapshot {
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Running,
            configured_networks,
            active_networks,
            failed_networks: normalize_failures(failed_networks),
        })
        .await
    }

    pub async fn begin_stop(&self) -> NodeLifecycleSnapshot {
        let current = self.snapshot.read().await.clone();
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Stopping,
            ..current
        })
        .await
    }

    pub async fn stopped(&self) -> NodeLifecycleSnapshot {
        let current = self.snapshot.read().await.clone();
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Stopped,
            active_networks: 0,
            ..current
        })
        .await
    }

    pub async fn failed(&self) -> NodeLifecycleSnapshot {
        let current = self.snapshot.read().await.clone();
        self.set(NodeLifecycleSnapshot {
            phase: NodePhase::Failed,
            active_networks: 0,
            ..current
        })
        .await
    }

    async fn set(&self, next: NodeLifecycleSnapshot) -> NodeLifecycleSnapshot {
        let mut current = self.snapshot.write().await;
        *current = next;
        let published = current.clone();
        self.events.send_replace(published.clone());
        published
    }
}

impl Default for NodeLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_failures(failed_networks: Vec<String>) -> Vec<String> {
    let mut failures: Vec<_> = failed_networks
        .into_iter()
        .filter(|name| !name.trim().is_empty())
        .collect();
    failures.sort();
    failures.dedup();
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_single_lifecycle_transition_stream() {
        let lifecycle = NodeLifecycle::new();
        let events = lifecycle.subscribe();

        assert_eq!(events.borrow().phase, NodePhase::Stopped);

        lifecycle
            .begin_start(2, vec!["office".into(), "office".into()])
            .await;
        assert_eq!(events.borrow().phase, NodePhase::Starting);
        assert_eq!(events.borrow().configured_networks, 2);
        assert_eq!(events.borrow().failed_networks, vec!["office"]);

        lifecycle.running(1, vec!["office".into()]).await;
        assert_eq!(events.borrow().phase, NodePhase::Running);
        assert_eq!(events.borrow().active_networks, 1);

        lifecycle.begin_stop().await;
        assert_eq!(events.borrow().phase, NodePhase::Stopping);
        assert_eq!(events.borrow().active_networks, 1);

        lifecycle.stopped().await;
        assert_eq!(events.borrow().phase, NodePhase::Stopped);
        assert_eq!(events.borrow().active_networks, 0);
        assert_eq!(events.borrow().configured_networks, 2);
    }

    #[tokio::test]
    async fn failed_phase_preserves_configuration_and_failure_context() {
        let lifecycle = NodeLifecycle::new();
        lifecycle.begin_start(1, Vec::new()).await;
        lifecycle.failed().await;

        let snapshot = lifecycle.snapshot().await;
        assert_eq!(snapshot.phase, NodePhase::Failed);
        assert_eq!(snapshot.configured_networks, 1);
        assert_eq!(snapshot.active_networks, 0);
    }
}
