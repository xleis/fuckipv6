use super::*;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use fuckipv6_common::domain::{
    NetworkCreateProfile, NetworkJoinMode, NetworkVisibility, NodeId, OperationState,
};
use fuckipv6_common::ClientConfig;
use futures::future::BoxFuture;
use tokio::sync::Notify;

use crate::node::{NodeNetworkStatus, NodePhase};
use crate::packet_io::{ExternalPacketDevice, ExternalPacketEndpoint};
use crate::symmetric::EmbeddedCoordinator;

fn temp_root() -> tempfile::TempDir {
    #[cfg(unix)]
    {
        tempfile::tempdir_in("/tmp").unwrap()
    }
    #[cfg(not(unix))]
    {
        tempfile::tempdir().unwrap()
    }
}

#[derive(Default)]
struct TestPlatform {
    opened: Notify,
    sessions: Mutex<VecDeque<(EmbeddedNetworkRequest, ExternalPacketEndpoint)>>,
    addresses: Mutex<Vec<SocketAddr>>,
    opens: AtomicUsize,
    mode: AtomicUsize,
    closes: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    close_failures: Arc<AtomicUsize>,
    cleanup_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    cleanup_done: Arc<AtomicUsize>,
    cleanup_gate: Arc<Notify>,
    pause_cleanup: std::sync::atomic::AtomicBool,
    shutdown_failures: AtomicUsize,
}

struct TestLease {
    endpoint: ExternalPacketEndpoint,
    closes: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    close_failures: Arc<AtomicUsize>,
    closed: std::sync::atomic::AtomicBool,
    cleanup_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    cleanup_done: Arc<AtomicUsize>,
    cleanup_gate: Option<Arc<Notify>>,
}

impl EmbeddedNetworkLease for TestLease {
    fn close(&mut self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async move {
            if self.close_failures.swap(0, Ordering::SeqCst) > 0 {
                return Err(NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "Injected platform cleanup failure",
                ));
            }
            if !self.closed.swap(true, Ordering::SeqCst) {
                self.endpoint.close();
                self.closes.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    }

    fn cancel(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.endpoint.close();
            self.cancels.fetch_add(1, Ordering::SeqCst);
            let done = self.cleanup_done.clone();
            let gate = self.cleanup_gate.clone();
            self.cleanup_tasks
                .lock()
                .unwrap()
                .push(tokio::spawn(async move {
                    if let Some(gate) = gate {
                        gate.notified().await;
                    }
                    done.fetch_add(1, Ordering::SeqCst);
                }));
        }
    }
}

impl EmbeddedPlatform for TestPlatform {
    fn open_network(
        &self,
        request: EmbeddedNetworkRequest,
    ) -> BoxFuture<'_, Result<EmbeddedNetwork, NodeProblem>> {
        Box::pin(async move {
            self.addresses.lock().unwrap().push(request.transport_addr);
            let (device, endpoint) =
                ExternalPacketDevice::new(&request.network_name, request.config.tun.mtu, 4)
                    .unwrap();
            let network = EmbeddedNetwork::new(
                device,
                Box::new(TestLease {
                    endpoint: endpoint.clone(),
                    closes: self.closes.clone(),
                    cancels: self.cancels.clone(),
                    close_failures: self.close_failures.clone(),
                    closed: std::sync::atomic::AtomicBool::new(false),
                    cleanup_tasks: self.cleanup_tasks.clone(),
                    cleanup_done: self.cleanup_done.clone(),
                    cleanup_gate: self
                        .pause_cleanup
                        .load(Ordering::SeqCst)
                        .then(|| self.cleanup_gate.clone()),
                }),
            );
            self.sessions.lock().unwrap().push_back((request, endpoint));
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened.notify_one();
            match self.mode.load(Ordering::SeqCst) {
                1 => std::future::pending::<()>().await,
                2 => {
                    return Err(NodeProblem::new(
                        NodeProblemCode::Unavailable,
                        "Injected platform initialization failure",
                    ))
                }
                3 => {
                    return Err(NodeProblem::new(
                        NodeProblemCode::PermissionDenied,
                        "Injected permission denial",
                    )
                    .with_message_key("embedded.test.permission_denied"))
                }
                _ => {}
            }
            Ok(network)
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async move {
            let tasks = std::mem::take(&mut *self.cleanup_tasks.lock().unwrap());
            for task in tasks {
                task.await.map_err(storage_problem)?;
            }
            if self.shutdown_failures.swap(0, Ordering::SeqCst) > 0 {
                return Err(NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "Injected platform shutdown failure",
                ));
            }
            Ok(())
        })
    }
}

impl TestPlatform {
    async fn take_session(&self) -> (EmbeddedNetworkRequest, ExternalPacketEndpoint) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let opened = self.opened.notified();
                if let Some(session) = self.sessions.lock().unwrap().pop_front() {
                    return session;
                }
                opened.await;
            }
        })
        .await
        .expect("platform session was not opened")
    }

    fn assert_sockets_released(&self) {
        for address in self.addresses.lock().unwrap().iter() {
            let _udp = std::net::UdpSocket::bind(address).expect("P2P UDP socket leaked");
            let _tcp = std::net::TcpListener::bind(address).expect("P2P TCP listener leaked");
        }
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    coordinator: EmbeddedCoordinator,
    nodes: Vec<EmbeddedNodeConfig>,
}

impl Fixture {
    async fn new(nodes: usize) -> Self {
        let _ = fuckipv6_common::init_logging_with_level("warn");
        let root = temp_root();
        let authority = ConfigManager::new_for_dir(root.path().join("coordinator")).unwrap();
        let mut record = authority
            .create_owned_network(
                NetworkCreateProfile {
                    name: "mesh".into(),
                    purpose: "Embedded runtime loopback tests".into(),
                    icon: "home".into(),
                    language: "zh-CN".into(),
                    region: "CN".into(),
                    tags: Vec::new(),
                    capability_declarations: Vec::new(),
                    visibility: NetworkVisibility::Unlisted,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: Default::default(),
                    exit_contribution_scope: Default::default(),
                    exit_contribution_mode: Default::default(),
                    exit_selection_policy: Default::default(),
                },
                "ws://127.0.0.1:1",
                true,
            )
            .unwrap()
            .record;
        let mut managers = Vec::new();
        for index in 0..nodes {
            let manager =
                ConfigManager::new_for_dir(root.path().join(format!("node-{index}"))).unwrap();
            let identity = manager.load_node_identity().unwrap();
            record
                .admit_member(
                    NodeId::from_public_key(&identity.public_key_bytes()),
                    record.updated_at_unix_ms,
                )
                .unwrap();
            managers.push(manager);
        }
        // Configure the test Coordinator's real member roster through the domain
        // admission rules. No node state or authorization checks are bypassed.
        std::fs::write(
            authority.network_record_path("mesh").unwrap(),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        let coordinator = EmbeddedCoordinator::start_with_config_dir(
            "mesh",
            0,
            0,
            None,
            Some("127.0.0.1".into()),
            None,
            root.path().join("coordinator"),
        )
        .await
        .unwrap();
        let mut configs = Vec::new();
        for manager in managers {
            let storage_root = manager.config_dir().to_path_buf();
            let path = manager
                .create_network(
                    "mesh",
                    &format!("ws://127.0.0.1:{}", coordinator.signaling_addr.port()),
                    true,
                )
                .unwrap();
            let mut config = ClientConfig::from_file(&path).unwrap();
            config.p2p.listen_port = 0;
            config.p2p.nat_detection.enabled = false;
            config.p2p.heartbeat_interval = 1;
            config.tun.mtu = 1280;
            // Invalid for native macOS TUN creation, valid for packet I/O.
            config.tun.name = "external-test".into();
            config.save_to_file(&path).unwrap();
            configs.push(EmbeddedNodeConfig {
                storage_root,
                dns_servers: Vec::new(),
            });
        }
        Self {
            _root: root,
            coordinator,
            nodes: configs,
        }
    }
}

async fn start(config: EmbeddedNodeConfig, platform: Arc<TestPlatform>) -> EmbeddedNodeRuntime {
    tokio::time::timeout(
        Duration::from_secs(15),
        EmbeddedNodeRuntime::start(config, platform),
    )
    .await
    .expect("embedded startup timed out")
    .unwrap_or_else(|problem| panic!("{}: {}", problem.message_key, problem.message))
}
// The overlay reports a peer as soon as signaling advertises it; the secure
// session key confirmation lands later. Packets queued by the host in between
// are dropped by the data plane, so tests wait for the real data path.
async fn wait_for_secure_session(host: &EmbeddedNodeRuntime, network: &str, peer: IpAddr) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let sessions = host.supervisor.registry().get_all().await;
            if let Some(session) = sessions
                .into_iter()
                .find(|session| session.network_name == network)
            {
                if session.p2p.has_established_session_by_ip(peer).await {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("secure session to peer was not established");
}

async fn wait_for_cleanup(config: &EmbeddedNodeConfig) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if acquire_lock(config.storage_root.join("embedded-node.lock")).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled host retained its task/storage ownership");
}

#[tokio::test]
async fn embedded_start_snapshot_stop_and_duplicate_host() {
    let mut fixture = Fixture::new(1).await;
    fixture.nodes[0].dns_servers = vec!["10.8.0.53".parse().unwrap()];
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    let snapshot = host.snapshot().await.unwrap();
    assert_eq!(snapshot.lifecycle.phase, NodePhase::Running);
    assert_eq!(snapshot.networks.len(), 1);
    assert_eq!(snapshot.networks[0].status, NodeNetworkStatus::Online);
    assert!(snapshot.identity.is_some());
    let (request, endpoint) = platform.take_session().await;
    assert_eq!(
        request.assigned_ips.ipv6,
        snapshot.networks[0].assigned_ipv6
    );
    assert_eq!(request.config.tun.mtu, 1280);
    assert_eq!(request.dns_servers, fixture.nodes[0].dns_servers);
    let duplicate = EmbeddedNodeRuntime::start(fixture.nodes[0].clone(), platform.clone()).await;
    assert_eq!(duplicate.err().unwrap().code, NodeProblemCode::Conflict);
    assert_eq!(platform.opens.load(Ordering::SeqCst), 1);
    host.stop().await.unwrap();
    host.stop().await.unwrap();
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Stopped
    );
    assert_eq!(
        endpoint.try_recv().unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    platform.assert_sockets_released();
    assert_eq!(platform.closes.load(Ordering::SeqCst), 1);
    let mut restarted = start(fixture.nodes[0].clone(), platform.clone()).await;
    assert_eq!(
        restarted.snapshot().await.unwrap().identity,
        snapshot.identity
    );
    restarted.stop().await.unwrap();
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_commands_preserve_authorization_operations_and_conflicts() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    let denied = host
        .execute_authorized(
            NodeCommandContext::untrusted("test"),
            NodeCommandRequest::new(10, NodeCommand::StopNode),
        )
        .await
        .unwrap();
    assert!(matches!(
        denied.outcome,
        NodeCommandOutcome::Rejected(NodeProblem {
            code: NodeProblemCode::PermissionDenied,
            ..
        })
    ));
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Running
    );
    let duplicate = host
        .execute(NodeCommandRequest::new(11, NodeCommand::StartNode))
        .await
        .unwrap();
    assert!(
        !matches!(duplicate, NodeCommandOutcome::Rejected(_)),
        "{duplicate:?}"
    );
    assert_eq!(platform.opens.load(Ordering::SeqCst), 1);
    let invalid = host
        .execute(NodeCommandRequest::new(
            12,
            NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::Named(vec!["../invalid".into()]),
            },
        ))
        .await;
    assert_eq!(invalid.unwrap_err().code, NodeProblemCode::InvalidRequest);

    let owner = NodeCommandContext::local_owner_for_snapshot(
        &host.snapshot().await.unwrap(),
        "embedded-test",
    );
    let request = NodeCommandRequest::new(
        13,
        NodeCommand::StopNetworks {
            networks: vec!["mesh".into()],
            all: false,
        },
    );
    let key = IdempotencyKey::from_public_key(b"embedded-stop-once");
    let (operation, created) = host
        .submit(owner.clone(), request.clone(), key.clone())
        .await
        .unwrap();
    assert!(created);
    let id = operation.operation.operation_id.as_str();
    let terminal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let operation = host.operation(id).unwrap();
            if operation.operation.state.is_terminal() && operation.result.is_some() {
                break operation;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal.operation.state, OperationState::Succeeded);
    assert!(terminal.result.unwrap().is_success());
    assert_eq!(
        host.snapshot().await.unwrap().networks[0].status,
        NodeNetworkStatus::Offline
    );
    let (same, created) = host
        .submit(owner.clone(), request, key.clone())
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(same.operation.operation_id.as_str(), id);
    let conflict = host
        .submit(
            owner,
            NodeCommandRequest::new(
                14,
                NodeCommand::StartNetworks {
                    selection: crate::node::NodeNetworkSelection::Named(vec!["mesh".into()]),
                },
            ),
            key,
        )
        .await;
    assert!(matches!(conflict, Err(OperationSubmitError::Conflict(_))));
    let started = host
        .execute(NodeCommandRequest::new(
            15,
            NodeCommand::StartNetworks {
                selection: crate::node::NodeNetworkSelection::Named(vec!["mesh".into()]),
            },
        ))
        .await
        .unwrap();
    assert!(!matches!(started, NodeCommandOutcome::Rejected(_)));
    assert_eq!(platform.opens.load(Ordering::SeqCst), 2);
    assert!(!host.supervisor.commands().audit_entries().is_empty());
    host.stop().await.unwrap();
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_cancel_start_cleans_partial_platform_and_transports() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    platform.mode.store(1, Ordering::SeqCst);
    platform.pause_cleanup.store(true, Ordering::SeqCst);
    let config = fixture.nodes[0].clone();
    let opening = tokio::spawn(EmbeddedNodeRuntime::start(config.clone(), platform.clone()));
    let (_, endpoint) = platform.take_session().await;
    opening.abort();
    assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while platform.cancels.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(acquire_lock(config.storage_root.join("embedded-node.lock")).is_err());
    platform.cleanup_gate.notify_one();
    wait_for_cleanup(&config).await;
    assert_eq!(
        endpoint.try_recv().unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(platform.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(platform.cleanup_done.load(Ordering::SeqCst), 1);
    platform.assert_sockets_released();
    platform.mode.store(0, Ordering::SeqCst);
    platform.pause_cleanup.store(false, Ordering::SeqCst);
    let mut host = start(config, platform.clone()).await;
    host.stop().await.unwrap();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_platform_failure_cleans_initialized_network_core() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    platform.mode.store(2, Ordering::SeqCst);
    let result = EmbeddedNodeRuntime::start(fixture.nodes[0].clone(), platform.clone()).await;
    assert_eq!(result.err().unwrap().code, NodeProblemCode::Unavailable);
    assert_eq!(platform.cancels.load(Ordering::SeqCst), 1);
    platform.assert_sockets_released();
    wait_for_cleanup(&fixture.nodes[0]).await;
    platform.mode.store(0, Ordering::SeqCst);
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    host.stop().await.unwrap();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_platform_errors_preserve_stable_problem_fields() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    platform.mode.store(3, Ordering::SeqCst);
    let problem = EmbeddedNodeRuntime::start(fixture.nodes[0].clone(), platform.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
    assert_eq!(problem.message_key, "embedded.test.permission_denied");
    assert_eq!(problem.network_name.as_deref(), Some("mesh"));
    assert!(!problem.retryable);
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_failed_cleanup_retains_ownership_and_stop_can_retry() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    platform.close_failures.store(1, Ordering::SeqCst);
    let problem = host.stop().await.unwrap_err();
    assert_eq!(problem.code, NodeProblemCode::Unavailable);
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Failed
    );
    assert!(matches!(
        EmbeddedNodeRuntime::start(fixture.nodes[0].clone(), platform.clone()).await,
        Err(NodeProblem {
            code: NodeProblemCode::Conflict,
            ..
        })
    ));
    host.stop().await.unwrap();
    host.stop().await.unwrap();
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Stopped
    );
    assert_eq!(platform.closes.load(Ordering::SeqCst), 1);
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_platform_shutdown_failure_can_retry_after_core_actors_exit() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    platform.shutdown_failures.store(1, Ordering::SeqCst);
    assert!(host.stop().await.is_err());
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Failed
    );
    assert!(acquire_lock(fixture.nodes[0].storage_root.join("embedded-node.lock")).is_err());
    host.stop().await.unwrap();
    assert_eq!(
        host.snapshot().await.unwrap().lifecycle.phase,
        NodePhase::Stopped
    );
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_conflicting_lifecycle_commands_are_rejected_during_real_startup() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    platform.take_session().await;
    platform.mode.store(1, Ordering::SeqCst);
    let commands = host.supervisor.commands();
    let owner = NodeCommandContext::local_owner_for_snapshot(
        &host.snapshot().await.unwrap(),
        "embedded-test",
    );
    let rebuilding = tokio::spawn(async move {
        commands
            .execute(
                owner,
                NodeCommandRequest::new(
                    25,
                    NodeCommand::RebuildTun {
                        network_name: "mesh".into(),
                    },
                ),
            )
            .await
    });
    platform.take_session().await;
    let conflict = host
        .execute(NodeCommandRequest::new(26, NodeCommand::StopNode))
        .await
        .unwrap();
    assert!(
        matches!(
            conflict,
            NodeCommandOutcome::Rejected(NodeProblem {
                code: NodeProblemCode::Conflict,
                ..
            })
        ),
        "{conflict:?}"
    );
    host.stop().await.unwrap();
    assert!(!rebuilding.await.unwrap().unwrap().is_success());
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_corrupt_operation_store_fails_before_network_start_and_releases_actors() {
    let mut fixture = Fixture::new(1).await;
    let path = fixture.nodes[0]
        .storage_root
        .join("embedded-operations.json");
    std::fs::write(&path, b"invalid operation store").unwrap();
    let platform = Arc::new(TestPlatform::default());
    let result = EmbeddedNodeRuntime::start(fixture.nodes[0].clone(), platform.clone()).await;
    assert!(result.is_err());
    assert_eq!(platform.opens.load(Ordering::SeqCst), 0);
    wait_for_cleanup(&fixture.nodes[0]).await;
    std::fs::remove_file(&path).unwrap();
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    host.stop().await.unwrap();
    fixture.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn embedded_owned_services_join_connections_and_release_listeners() {
    use tokio::io::AsyncReadExt;
    let root = temp_root();
    let config = EmbeddedNodeConfig {
        storage_root: root.path().to_path_buf(),
        dns_servers: Vec::new(),
    };
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(config.clone(), platform.clone()).await;
    let signaling_reservation = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let signaling_port = signaling_reservation.local_addr().unwrap().port();
    let relay_reservation = loop {
        let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).unwrap();
        if socket.local_addr().unwrap().port() != signaling_port {
            break socket;
        }
    };
    let relay_port = relay_reservation.local_addr().unwrap().port();
    let create = serde_json::from_value(serde_json::json!({
        "name": "hosted", "purpose": "Owned embedded service lifecycle test",
        "signaling_port": signaling_port, "relay_port": relay_port, "external_host": "127.0.0.1",
        "auto_connect": false
    }))
    .unwrap();
    drop(signaling_reservation);
    drop(relay_reservation);
    let created = host
        .execute(NodeCommandRequest::new(
            30,
            NodeCommand::CreateNetwork(create),
        ))
        .await
        .unwrap();
    assert!(matches!(created, NodeCommandOutcome::NetworkCreated(_)));
    let snapshot = host.snapshot().await.unwrap();
    let service = snapshot
        .embedded_services
        .iter()
        .find(|service| service.network_name == "hosted")
        .unwrap();
    let signaling_port = service.signaling_addr.unwrap().port();
    let relay_port = service.relay_addr.unwrap().port();
    let configs = ConfigManager::new_for_dir(config.storage_root.clone()).unwrap();
    let client = crate::signaling::client::SignalingClient::connect(
        &format!("ws://127.0.0.1:{signaling_port}"),
        configs.load_node_identity().unwrap(),
        0,
        fuckipv6_common::NatType::Unknown,
    )
    .await
    .unwrap();
    // A half-open WebSocket handshake must not pin the service during stop.
    let mut half_open = tokio::net::TcpStream::connect(("127.0.0.1", signaling_port))
        .await
        .unwrap();
    host.stop().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), half_open.read(&mut [0; 1]))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    client.close().await.unwrap();
    let _listener = tokio::net::TcpListener::bind(("0.0.0.0", signaling_port))
        .await
        .unwrap();
    let _relay = std::net::UdpSocket::bind(("0.0.0.0", relay_port)).unwrap();
    assert_eq!(platform.opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn embedded_packet_task_failure_is_visible_and_can_restart() {
    let mut fixture = Fixture::new(1).await;
    let platform = Arc::new(TestPlatform::default());
    let mut host = start(fixture.nodes[0].clone(), platform.clone()).await;
    let (_, endpoint) = platform.take_session().await;
    endpoint.close();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = host.snapshot().await.unwrap();
            if snapshot.networks[0].status == NodeNetworkStatus::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(host.snapshot().await.unwrap().last_error.is_some());
    let result = host
        .execute(NodeCommandRequest::new(
            21,
            NodeCommand::RestartNetworks {
                networks: vec!["mesh".into()],
                all: false,
            },
        ))
        .await
        .unwrap();
    assert!(
        !matches!(result, NodeCommandOutcome::Rejected(_)),
        "{result:?}"
    );
    assert_eq!(
        host.snapshot().await.unwrap().networks[0].status,
        NodeNetworkStatus::Online
    );
    assert_eq!(platform.opens.load(Ordering::SeqCst), 2);
    host.stop().await.unwrap();
    platform.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

fn ip_packet(source: IpAddr, destination: IpAddr) -> Vec<u8> {
    match (source, destination) {
        (IpAddr::V4(source), IpAddr::V4(destination)) => {
            let mut packet = vec![0; 24];
            packet[0] = 0x45;
            packet[2..4].copy_from_slice(&24u16.to_be_bytes());
            packet[8] = 64;
            packet[9] = 253;
            packet[12..16].copy_from_slice(&source.octets());
            packet[16..20].copy_from_slice(&destination.octets());
            packet[20..].copy_from_slice(b"P2v4");
            let (words, _) = packet[..20].as_chunks::<2>();
            let mut sum: u32 = words
                .iter()
                .map(|word| u32::from(u16::from_be_bytes(*word)))
                .sum();
            while sum > 0xffff {
                sum = (sum & 0xffff) + (sum >> 16);
            }
            packet[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
            packet
        }
        (IpAddr::V6(source), IpAddr::V6(destination)) => {
            let mut packet = vec![0; 44];
            packet[0] = 0x60;
            packet[4..6].copy_from_slice(&4u16.to_be_bytes());
            packet[6] = 253;
            packet[7] = 64;
            packet[8..24].copy_from_slice(&source.octets());
            packet[24..40].copy_from_slice(&destination.octets());
            packet[40..].copy_from_slice(b"P2v6");
            packet
        }
        _ => panic!("packet address families differ"),
    }
}
fn large_ip_packet(source: IpAddr, destination: IpAddr, payload: usize) -> Vec<u8> {
    // IPv6 carries no header checksum, so extending the payload keeps the
    // packet valid without recomputing anything.
    assert!(source.is_ipv6(), "only IPv6 packets are extended here");
    let mut packet = ip_packet(source, destination);
    packet.resize(44 + payload, b'x');
    let length = (packet.len() - 40) as u16;
    packet[4..6].copy_from_slice(&length.to_be_bytes());
    packet
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_real_signaling_secure_p2p_and_external_ip_packets() {
    let mut fixture = Fixture::new(2).await;
    let platform_a = Arc::new(TestPlatform::default());
    let platform_b = Arc::new(TestPlatform::default());
    let mut a = start(fixture.nodes[0].clone(), platform_a.clone()).await;
    let mut b = start(fixture.nodes[1].clone(), platform_b.clone()).await;
    let (request_a, endpoint_a) = platform_a.take_session().await;
    let (request_b, endpoint_b) = platform_b.take_session().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = a.snapshot().await.unwrap();
            if !snapshot.peers.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    wait_for_secure_session(&a, "mesh", IpAddr::V6(request_b.assigned_ips.ipv6.unwrap())).await;
    wait_for_secure_session(&b, "mesh", IpAddr::V6(request_a.assigned_ips.ipv6.unwrap())).await;
    for (source, target) in [
        (
            IpAddr::V6(request_a.assigned_ips.ipv6.unwrap()),
            IpAddr::V6(request_b.assigned_ips.ipv6.unwrap()),
        ),
        (
            IpAddr::V4(request_a.assigned_ips.ipv4.unwrap()),
            IpAddr::V4(request_b.assigned_ips.ipv4.unwrap()),
        ),
    ] {
        let packet = ip_packet(source, target);
        endpoint_a.send(&packet).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(10), endpoint_b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, packet);
        let response = ip_packet(target, source);
        endpoint_b.send(&response).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), endpoint_a.recv())
                .await
                .unwrap()
                .unwrap(),
            response
        );
    }
    let before = a.snapshot().await.unwrap();
    a.network_path_changed().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(platform_a.opens.load(Ordering::SeqCst), 1);
    assert_eq!(a.snapshot().await.unwrap().identity, before.identity);
    let certificate = crate::p2p::quic::generate_node_certificate().unwrap();
    let quic = crate::p2p::quic::run_loopback_self_test(&certificate)
        .await
        .unwrap();
    assert_eq!(quic.echoed.payload, b"quic-self-test");
    let stats_b = b
        .supervisor
        .registry()
        .get_all()
        .await
        .into_iter()
        .find(|session| session.network_name == "mesh")
        .unwrap()
        .stats;
    let baseline = stats_b.get_global_stats().await.p2p_rx_packets;
    for _ in 0..5 {
        endpoint_a
            .send(&ip_packet(
                IpAddr::V6(request_a.assigned_ips.ipv6.unwrap()),
                IpAddr::V6(request_b.assigned_ips.ipv6.unwrap()),
            ))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while stats_b.get_global_stats().await.p2p_rx_packets < baseline + 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Five packets fill the four-packet output queue and block a write. Host
    // cancellation must interrupt the real event loop rather than await it forever.
    tokio::time::timeout(Duration::from_secs(5), b.stop())
        .await
        .unwrap()
        .unwrap();
    a.stop().await.unwrap();
    platform_a.assert_sockets_released();
    platform_b.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

// A long-lived host restarts networks after real traffic already used the
// process-wide crypto worker pool. Restart must reuse that pool instead of
// failing with a second, conflicting configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_restart_after_traffic_reuses_process_crypto_pool() {
    let mut fixture = Fixture::new(2).await;
    let platform_a = Arc::new(TestPlatform::default());
    let platform_b = Arc::new(TestPlatform::default());
    let mut a = start(fixture.nodes[0].clone(), platform_a.clone()).await;
    let mut b = start(fixture.nodes[1].clone(), platform_b.clone()).await;
    let (request_a, endpoint_a) = platform_a.take_session().await;
    let (request_b, endpoint_b) = platform_b.take_session().await;
    let peer_a = IpAddr::V6(request_a.assigned_ips.ipv6.unwrap());
    let peer_b = IpAddr::V6(request_b.assigned_ips.ipv6.unwrap());
    wait_for_secure_session(&a, "mesh", peer_b).await;
    wait_for_secure_session(&b, "mesh", peer_a).await;
    let first = ip_packet(peer_a, peer_b);
    endpoint_a.send(&first).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), endpoint_b.recv())
            .await
            .unwrap()
            .unwrap(),
        first
    );

    // A queued batch of four full-size packets crosses the data plane's
    // parallel crypto threshold, so the process-wide worker pool is live
    // before the host restarts its networks.
    for _ in 0..4 {
        endpoint_a
            .send(&large_ip_packet(peer_a, peer_b, 1_200))
            .await
            .unwrap();
    }
    for _ in 0..4 {
        assert!(
            tokio::time::timeout(Duration::from_secs(10), endpoint_b.recv())
                .await
                .unwrap()
                .is_ok()
        );
    }
    assert!(
        fuckipv6_secure_session::crypto_batch_metrics().worker_threads > 0,
        "the data plane did not use the process-wide crypto worker pool"
    );

    let result = a
        .execute(NodeCommandRequest::new(
            23,
            NodeCommand::RestartNetworks {
                networks: vec!["mesh".into()],
                all: false,
            },
        ))
        .await
        .unwrap();
    assert!(
        !matches!(result, NodeCommandOutcome::Rejected(_)),
        "{result:?}"
    );
    assert_eq!(
        a.snapshot().await.unwrap().networks[0].status,
        NodeNetworkStatus::Online
    );
    assert_eq!(platform_a.opens.load(Ordering::SeqCst), 2);

    // The restarted runtime owns a fresh packet device and a fresh transport.
    let (_, endpoint_a2) = platform_a.take_session().await;
    let second = ip_packet(peer_a, peer_b);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            endpoint_a2.send(&second).await.unwrap();
            if let Ok(Ok(received)) =
                tokio::time::timeout(Duration::from_millis(100), endpoint_b.recv()).await
            {
                assert_eq!(received, second);
                return;
            }
        }
    })
    .await
    .expect("packets did not flow after the runtime restart");

    a.stop().await.unwrap();
    b.stop().await.unwrap();
    platform_a.assert_sockets_released();
    platform_b.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Instrumented load harness (P1b). Not part of the standing suite: run it with
//   cargo test -p fuckipv6-client --no-default-features --lib \
//     embedded::tests::embedded_control_plane_latency_matrix -- --nocapture --ignored
// It floods the real data plane of node A and measures how long node A's (host)
// and node B's (receiver) authoritative snapshots take while the flood runs.
// ---------------------------------------------------------------------------
async fn p2p_rx_packets(host: &EmbeddedNodeRuntime) -> u64 {
    match host
        .supervisor
        .registry()
        .get_all()
        .await
        .into_iter()
        .find(|session| session.network_name == "mesh")
    {
        Some(session) => session.stats.get_global_stats().await.p2p_rx_packets,
        None => 0,
    }
}

struct ProbeReport {
    probes: usize,
    errors: usize,
    samples_ms: Vec<f64>,
}

impl ProbeReport {
    fn quantile(&mut self, fraction: f64) -> f64 {
        if self.samples_ms.is_empty() {
            return 0.0;
        }
        self.samples_ms.sort_by(|left, right| left.total_cmp(right));
        let index = ((self.samples_ms.len() - 1) as f64 * fraction).round() as usize;
        self.samples_ms[index]
    }
}

async fn probe_snapshot(
    runtime: &EmbeddedNodeRuntime,
    report: &mut ProbeReport,
    budget: Duration,
) -> bool {
    let started = std::time::Instant::now();
    report.probes += 1;
    match tokio::time::timeout(budget, runtime.snapshot()).await {
        Ok(Ok(_)) => {
            report
                .samples_ms
                .push(started.elapsed().as_secs_f64() * 1_000.0);
            true
        }
        Ok(Err(problem)) => {
            report.errors += 1;
            println!("    probe error: {:?}: {}", problem.code, problem.message);
            false
        }
        Err(_) => {
            report.errors += 1;
            false
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "instrumented load harness, run explicitly with --ignored"]
async fn embedded_control_plane_latency_matrix() {
    let mut fixture = Fixture::new(2).await;
    let platform_a = Arc::new(TestPlatform::default());
    let platform_b = Arc::new(TestPlatform::default());
    let mut a = start(fixture.nodes[0].clone(), platform_a.clone()).await;
    let mut b = start(fixture.nodes[1].clone(), platform_b.clone()).await;
    let (request_a, endpoint_a) = platform_a.take_session().await;
    let (request_b, endpoint_b) = platform_b.take_session().await;
    let peer_a = IpAddr::V6(request_a.assigned_ips.ipv6.unwrap());
    let peer_b = IpAddr::V6(request_b.assigned_ips.ipv6.unwrap());
    wait_for_secure_session(&a, "mesh", peer_b).await;
    wait_for_secure_session(&b, "mesh", peer_a).await;
    let _ = &endpoint_b;

    let packet = large_ip_packet(peer_a, peer_b, 1_200);
    println!(
        "interval_us count inject_ms max_block_ms a_probes a_err a_p50 a_p90 a_max b_probes b_err b_p50 b_p90 b_max delivered drain_ms"
    );

    let scenarios: [(usize, Duration); 5] = [
        (128, Duration::from_millis(1)),
        (256, Duration::from_micros(500)),
        (512, Duration::from_micros(200)),
        (1_024, Duration::from_millis(1)),
        (1_024, Duration::from_micros(200)),
    ];

    for (count, interval) in scenarios {
        let baseline = p2p_rx_packets(&b).await;
        let endpoint = endpoint_a.clone();
        let payload = packet.clone();
        let injector = tokio::spawn(async move {
            let started = std::time::Instant::now();
            let mut max_block = Duration::ZERO;
            for _ in 0..count {
                let send_started = std::time::Instant::now();
                endpoint.send(&payload).await.unwrap();
                max_block = max_block.max(send_started.elapsed());
                if !interval.is_zero() {
                    tokio::time::sleep(interval).await;
                }
            }
            (started.elapsed(), max_block)
        });

        let mut report_a = ProbeReport {
            probes: 0,
            errors: 0,
            samples_ms: Vec::new(),
        };
        let mut report_b = ProbeReport {
            probes: 0,
            errors: 0,
            samples_ms: Vec::new(),
        };
        while !injector.is_finished() {
            probe_snapshot(&a, &mut report_a, Duration::from_secs(5)).await;
            probe_snapshot(&b, &mut report_b, Duration::from_secs(5)).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (inject_elapsed, max_block) = injector.await.unwrap();
        probe_snapshot(&a, &mut report_a, Duration::from_secs(5)).await;
        probe_snapshot(&b, &mut report_b, Duration::from_secs(5)).await;

        let drain_started = std::time::Instant::now();
        let delivered = loop {
            let received = p2p_rx_packets(&b).await.saturating_sub(baseline);
            if received >= count as u64 || drain_started.elapsed() > Duration::from_secs(20) {
                break received;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let drain = drain_started.elapsed();

        let a50 = report_a.quantile(0.5);
        let a90 = report_a.quantile(0.9);
        let amax = report_a.quantile(1.0);
        let b50 = report_b.quantile(0.5);
        let b90 = report_b.quantile(0.9);
        let bmax = report_b.quantile(1.0);
        println!(
            "{:>11} {:>5} {:>9.1} {:>12.1} {:>8} {:>5} {:>6.1} {:>6.1} {:>6.1} {:>8} {:>5} {:>6.1} {:>6.1} {:>6.1} {:>9} {:>8.1}",
            interval.as_micros(),
            count,
            inject_elapsed.as_secs_f64() * 1_000.0,
            max_block.as_secs_f64() * 1_000.0,
            report_a.probes,
            report_a.errors,
            a50,
            a90,
            amax,
            report_b.probes,
            report_b.errors,
            b50,
            b90,
            bmax,
            delivered,
            drain.as_secs_f64() * 1_000.0,
        );
    }

    let stop_started = std::time::Instant::now();
    a.stop().await.unwrap();
    println!(
        "node A stop took {:.1} ms",
        stop_started.elapsed().as_secs_f64() * 1_000.0
    );
    let stop_started = std::time::Instant::now();
    b.stop().await.unwrap();
    println!(
        "node B stop took {:.1} ms",
        stop_started.elapsed().as_secs_f64() * 1_000.0
    );
    platform_a.assert_sockets_released();
    platform_b.assert_sockets_released();
    fixture.coordinator.shutdown().await.unwrap();
}
