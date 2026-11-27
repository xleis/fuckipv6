//! Android embedded node host and JNI bridge.
//!
//! Owns a Tokio runtime, an authoritative [`EmbeddedNodeRuntime`], an
//! [`AndroidEmbeddedPlatform`] wired to `VpnService`, and a bearer-authenticated
//! loopback HTTP Control + File API server so the Android Compose UI speaks the
//! exact shared node protocol directly to the in-process Rust node.

pub mod http_server;
#[cfg(target_os = "android")]
pub mod jni_api;
pub mod platform;
pub mod settings;
pub mod tun_pump;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fuckipv6_client::embedded::{EmbeddedNodeConfig, EmbeddedNodeRuntime};
use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

use crate::http_server::{AndroidTelemetry, LoopbackHttpServer};
use crate::platform::{AndroidEmbeddedPlatform, AndroidHostCallbacks};

pub struct AndroidNodeHandle {
    tokio: Runtime,
    runtime: Arc<RwLock<Option<EmbeddedNodeRuntime>>>,
    platform: Arc<AndroidEmbeddedPlatform>,
    server: Option<LoopbackHttpServer>,
    api_url: String,
    api_token: String,
    node_id: String,
    started_at: Instant,
}

impl AndroidNodeHandle {
    pub fn start(
        storage_root: PathBuf,
        callbacks: Arc<dyn AndroidHostCallbacks>,
    ) -> Result<Self, NodeProblem> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();

        if !storage_root.is_absolute() {
            return Err(NodeProblem::new(
                NodeProblemCode::InvalidRequest,
                "Android storage_root must be absolute",
            ));
        }
        std::fs::create_dir_all(&storage_root).map_err(|error| {
            NodeProblem::new(
                NodeProblemCode::Unavailable,
                format!("failed to create Android node storage dir: {error}"),
            )
        })?;

        let tokio = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("fv6-android")
            .build()
            .map_err(|error| {
                NodeProblem::new(
                    NodeProblemCode::Internal,
                    format!("failed to build Android Tokio runtime: {error}"),
                )
            })?;

        let platform = AndroidEmbeddedPlatform::new(callbacks, tokio.handle().clone());
        let config = EmbeddedNodeConfig {
            storage_root,
            dns_servers: Vec::new(),
        };
        let started_at = Instant::now();

        let (embedded, node_id) = tokio.block_on(async {
            let embedded = EmbeddedNodeRuntime::start(config, platform.clone()).await?;
            let snapshot = embedded.snapshot().await?;
            let node_id = snapshot
                .identity
                .as_ref()
                .map(|id| id.node_id.clone())
                .unwrap_or_default();
            Ok::<_, NodeProblem>((embedded, node_id))
        })?;

        let runtime_cell = Arc::new(RwLock::new(Some(embedded)));
        let server = tokio
            .block_on(LoopbackHttpServer::start(
                runtime_cell.clone(),
                platform.clone(),
                started_at,
            ))
            .map_err(|error| {
                // Clean up the embedded node if binding the loopback HTTP API failed.
                tokio.block_on(async {
                    if let Some(mut node) = runtime_cell.write().await.take() {
                        let _ = node.stop().await;
                    }
                });
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("failed to start Android loopback HTTP server: {error}"),
                )
            })?;

        let api_url = server.api_url();
        let api_token = server.token.clone();

        Ok(Self {
            tokio,
            runtime: runtime_cell,
            platform,
            server: Some(server),
            api_url,
            api_token,
            node_id,
            started_at,
        })
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub fn api_token(&self) -> &str {
        &self.api_token
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn notify_network_path_changed(&self) -> Result<(), NodeProblem> {
        let cell = self.runtime.clone();
        self.tokio.block_on(async move {
            let guard = cell.read().await;
            let Some(node) = guard.as_ref() else {
                return Err(NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "embedded node is stopped",
                ));
            };
            node.network_path_changed().await
        })
    }

    pub fn telemetry(&self) -> AndroidTelemetry {
        AndroidTelemetry {
            uptime_secs: self.started_at.elapsed().as_secs(),
            active_tunnel_sessions: self.platform.active_session_count(),
            counters: self.platform.counters_snapshot(),
        }
    }

    pub fn stop(mut self) -> Result<(), NodeProblem> {
        let server = self.server.take();
        let runtime_cell = self.runtime.clone();
        let result = self.tokio.block_on(async move {
            if let Some(server) = server {
                server.stop().await;
            }
            if let Some(mut node) = runtime_cell.write().await.take() {
                node.stop().await
            } else {
                Ok(())
            }
        });
        self.tokio.shutdown_timeout(Duration::from_secs(3));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_server::constant_time_token_eq;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DummyCallbacks {
        closes: AtomicUsize,
    }

    impl AndroidHostCallbacks for DummyCallbacks {
        fn open_network(&self, _settings_json: &str) -> Result<i32, NodeProblem> {
            Err(NodeProblem::new(
                NodeProblemCode::Unsupported,
                "no tun fd in unit test",
            ))
        }

        fn close_network(&self, _network_name: &str) -> Result<(), NodeProblem> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn bearer_token_comparison_rejects_mismatches() {
        assert!(constant_time_token_eq("abc123", "abc123"));
        assert!(!constant_time_token_eq("abc123", "abc124"));
        assert!(!constant_time_token_eq("abc", "abcd"));
        assert!(!constant_time_token_eq("", "a"));
    }

    #[test]
    fn embedded_node_starts_loopback_http_api_and_stops_cleanly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let callbacks = Arc::new(DummyCallbacks {
            closes: AtomicUsize::new(0),
        });
        let handle = AndroidNodeHandle::start(temp.path().to_path_buf(), callbacks)
            .expect("start android node handle");
        assert!(handle.api_url().starts_with("http://127.0.0.1:"));
        assert_eq!(handle.api_token().len(), 64);
        assert!(!handle.node_id().is_empty());

        let api_url = handle.api_url().to_owned();
        let token = handle.api_token().to_owned();
        let expected_node_id = handle.node_id().to_owned();
        let test_rt = tokio::runtime::Runtime::new().expect("test tokio runtime");
        test_rt.block_on(async move {
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("http client");

            // Unauthenticated request is rejected with 401.
            let unauth = client
                .get(format!("{api_url}/api/v1/snapshot"))
                .send()
                .await
                .expect("unauth request");
            assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

            // Authenticated request returns the authoritative node snapshot.
            let snap_resp = client
                .get(format!("{api_url}/api/v1/snapshot"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("auth snapshot");
            assert_eq!(snap_resp.status(), reqwest::StatusCode::OK);
            let body = snap_resp.text().await.expect("snapshot body");
            let snap_json: serde_json::Value = serde_json::from_str(&body).expect("json snapshot");
            assert_eq!(
                snap_json["snapshot"]["identity"]["node_id"].as_str(),
                Some(expected_node_id.as_str())
            );

            // Telemetry endpoint responds with live counters.
            let telem_resp = client
                .get(format!("{api_url}/api/v1/android/telemetry"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("telemetry");
            assert_eq!(telem_resp.status(), reqwest::StatusCode::OK);

            // Authoritative UI manifest matches desktop/WebUI contract.
            let manifest_resp = client
                .get(format!("{api_url}/api/v1/manifest"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("manifest");
            assert_eq!(manifest_resp.status(), reqwest::StatusCode::OK);

            // Authoritative traffic-policy read command succeeds synchronously (200 OK).
            let tp_resp = client
                .post(format!("{api_url}/api/v1/commands"))
                .bearer_auth(&token)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(
                    serde_json::json!({
                        "schema_version": 1,
                        "request": {
                            "request_id": 101,
                            "command": { "command": "get_traffic_policy" }
                        }
                    })
                    .to_string(),
                )
                .send()
                .await
                .expect("get_traffic_policy");
            assert_eq!(tp_resp.status(), reqwest::StatusCode::OK);

            // Mutating stop_node returns 202 Accepted with canonical idem_<43> key
            // and leaves the loopback control HTTP API online (W16-02 #4).
            let stop_resp = client
                .post(format!("{api_url}/api/v1/commands"))
                .bearer_auth(&token)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(
                    serde_json::json!({
                        "schema_version": 1,
                        "idempotency_key": "idem_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "request": {
                            "request_id": 102,
                            "command": { "command": "stop_node" }
                        }
                    })
                    .to_string(),
                )
                .send()
                .await
                .expect("stop_node");
            assert_eq!(stop_resp.status(), reqwest::StatusCode::ACCEPTED);

            let snap_after_stop = client
                .get(format!("{api_url}/api/v1/snapshot"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("snapshot after stop_node");
            assert_eq!(snap_after_stop.status(), reqwest::StatusCode::OK);

            // File targets & Android network-changed endpoints respond cleanly.
            let file_targets = client
                .get(format!("{api_url}/api/v1/file/targets"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("file targets");
            assert_eq!(file_targets.status(), reqwest::StatusCode::OK);

            let net_changed = client
                .post(format!("{api_url}/api/v1/android/network-changed"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("network changed");
            assert_eq!(net_changed.status(), reqwest::StatusCode::OK);
        });

        handle.stop().expect("stop handle cleanly");
    }
}
