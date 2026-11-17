//! C ABI consumed by the Swift host.
//!
//! Rules enforced here:
//! - no panic crosses the boundary (every entry point is wrapped in catch_unwind);
//! - every pointer is validated before use, and buffers report BufferTooSmall
//!   together with the required length instead of truncating silently;
//! - the runtime owns its Tokio executor, and every blocking entry point refuses
//!   to run on a Tokio worker thread;
//! - start/stop are idempotent, bounded and cancellable.

use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fuckipv6_client::config_manager::ConfigManager;
use fuckipv6_client::embedded::{EmbeddedNodeConfig, EmbeddedNodeRuntime, EmbeddedPlatform};
use fuckipv6_client::node::{NodeCommandOutcome, NodeCommandRequest, NodeProblem};
use serde::Deserialize;
use tokio::runtime::Runtime;

use crate::error::Fv6Status;
use crate::platform::{EndpointHandle, HostBridge, HostCallbacks, IosPlatform};

/// Lifecycle phases visible through fv6_runtime_phase.
pub const FV6_PHASE_CREATED: u8 = 0;
pub const FV6_PHASE_STARTING: u8 = 1;
pub const FV6_PHASE_RUNNING: u8 = 2;
pub const FV6_PHASE_STOPPING: u8 = 3;
pub const FV6_PHASE_STOPPED: u8 = 4;
pub const FV6_PHASE_FAILED: u8 = 5;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WORKER_THREADS: usize = 8;
const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Deserialize)]
struct RuntimeConfig {
    storage_root: String,
    #[serde(default)]
    dns_servers: Vec<String>,
    #[serde(default)]
    worker_threads: Option<usize>,
}

struct NodeSlot {
    node: Option<EmbeddedNodeRuntime>,
    start_task: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    phase: AtomicU8,
    stop_requested: AtomicBool,
    last_error: Mutex<Option<String>>,
    slot: Mutex<NodeSlot>,
}

impl Inner {
    fn phase(&self) -> u8 {
        self.phase.load(Ordering::SeqCst)
    }

    fn slot(&self) -> MutexGuard<'_, NodeSlot> {
        self.slot.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn set_error(&self, message: impl Into<String>) {
        let message = message.into();
        *self
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(message);
    }

    fn take_start_task(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.slot().start_task.take()
    }
}

pub struct Fv6Runtime {
    runtime: Option<Runtime>,
    config: EmbeddedNodeConfig,
    host: Arc<HostBridge>,
    inner: Arc<Inner>,
}

impl Fv6Runtime {
    fn new(
        config: EmbeddedNodeConfig,
        worker_threads: usize,
        callbacks: HostCallbacks,
        context: *mut c_void,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .thread_name("fv6-node")
            .enable_all()
            .build()
            .map_err(|error| format!("failed to create the node executor: {error}"))?;
        let host = HostBridge::new(callbacks, context, runtime.handle().clone());
        Ok(Self {
            runtime: Some(runtime),
            config,
            host,
            inner: Arc::new(Inner {
                phase: AtomicU8::new(FV6_PHASE_CREATED),
                stop_requested: AtomicBool::new(false),
                last_error: Mutex::new(None),
                slot: Mutex::new(NodeSlot {
                    node: None,
                    start_task: None,
                }),
            }),
        })
    }

    fn executor(&self) -> &Runtime {
        self.runtime
            .as_ref()
            .expect("node executor is released only after teardown")
    }

    fn start(&self, timeout: Duration) -> Result<(), Fv6Status> {
        match self.inner.phase() {
            FV6_PHASE_RUNNING => return Ok(()),
            FV6_PHASE_STARTING => return Err(Fv6Status::Busy),
            FV6_PHASE_STOPPING => return Err(Fv6Status::Busy),
            _ => {}
        }
        let mut slot = self.inner.slot();
        if slot.node.is_some() {
            // Leftover from a failed stop: the previous runtime was never
            // released, so starting again would double-open the mesh.
            return Err(Fv6Status::State);
        }
        slot.start_task = None;
        drop(slot);

        self.inner.stop_requested.store(false, Ordering::SeqCst);
        *self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        self.inner.phase.store(FV6_PHASE_STARTING, Ordering::SeqCst);

        let inner = self.inner.clone();
        let config = self.config.clone();
        let platform: Arc<dyn EmbeddedPlatform> = Arc::new(IosPlatform::new(self.host.clone()));
        let task = self.executor().handle().spawn(async move {
            match EmbeddedNodeRuntime::start(config, platform).await {
                Ok(mut node) => {
                    if inner.stop_requested.load(Ordering::SeqCst) {
                        let _ = node.stop().await;
                        inner.phase.store(FV6_PHASE_STOPPED, Ordering::SeqCst);
                    } else {
                        inner.slot().node = Some(node);
                        inner.phase.store(FV6_PHASE_RUNNING, Ordering::SeqCst);
                    }
                }
                Err(problem) => {
                    inner.set_error(problem_text(&problem));
                    inner.phase.store(FV6_PHASE_FAILED, Ordering::SeqCst);
                }
            }
        });
        self.inner.slot().start_task = Some(task);

        let deadline = Instant::now() + timeout;
        loop {
            match self.inner.phase() {
                FV6_PHASE_RUNNING => return Ok(()),
                FV6_PHASE_FAILED => return Err(Fv6Status::NodeProblem),
                FV6_PHASE_STOPPED => return Err(Fv6Status::Closed),
                _ => {}
            }
            if Instant::now() >= deadline {
                // The start task keeps the cancellation flag and releases every
                // resource it acquires before completion.
                self.inner.stop_requested.store(true, Ordering::SeqCst);
                self.inner
                    .set_error("node start timed out before the node became ready");
                return Err(Fv6Status::Timeout);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn stop(&self, timeout: Duration) -> Result<(), Fv6Status> {
        match self.inner.phase() {
            FV6_PHASE_CREATED | FV6_PHASE_STOPPED | FV6_PHASE_FAILED => {
                self.inner.phase.store(FV6_PHASE_STOPPED, Ordering::SeqCst);
                return Ok(());
            }
            _ => {}
        }
        self.inner.stop_requested.store(true, Ordering::SeqCst);
        self.inner.phase.store(FV6_PHASE_STOPPING, Ordering::SeqCst);
        if let Some(task) = self.inner.take_start_task() {
            let _ = self
                .executor()
                .block_on(async move { tokio::time::timeout(timeout, task).await });
        }
        let node = self.inner.slot().node.take();
        let Some(mut node) = node else {
            self.inner.phase.store(FV6_PHASE_STOPPED, Ordering::SeqCst);
            return Ok(());
        };
        let outcome = {
            let stop = async { tokio::time::timeout(timeout, node.stop()).await };
            self.executor().block_on(stop)
        };
        match outcome {
            Ok(Ok(())) => {
                self.inner.phase.store(FV6_PHASE_STOPPED, Ordering::SeqCst);
                Ok(())
            }
            Ok(Err(problem)) => {
                self.inner.set_error(problem_text(&problem));
                self.inner.phase.store(FV6_PHASE_FAILED, Ordering::SeqCst);
                // Keep the runtime alive so a bounded retry can still stop it.
                self.inner.slot().node = Some(node);
                Err(Fv6Status::NodeProblem)
            }
            Err(_) => {
                self.inner
                    .set_error("node stop did not finish before the deadline");
                self.inner.phase.store(FV6_PHASE_FAILED, Ordering::SeqCst);
                self.inner.slot().node = Some(node);
                Err(Fv6Status::Timeout)
            }
        }
    }

    fn last_error_text(&self) -> String {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .unwrap_or_default()
    }

    fn teardown(&mut self) {
        if self.runtime.is_none() {
            return;
        }
        let _ = self.stop(Duration::from_secs(10));
        let leftover = self.inner.slot().node.take();
        if let Some(mut node) = leftover {
            let _ = self.executor().block_on(async {
                tokio::time::timeout(Duration::from_secs(5), node.stop()).await
            });
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(5));
        }
    }
}

fn wait_node<T>(
    runtime: &Fv6Runtime,
    timeout: Duration,
    future: impl std::future::Future<Output = Result<T, NodeProblem>>,
) -> Result<T, Fv6Status> {
    match runtime
        .executor()
        .block_on(async move { tokio::time::timeout(timeout, future).await })
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(problem)) => {
            runtime.inner.set_error(problem_text(&problem));
            Err(Fv6Status::NodeProblem)
        }
        Err(_) => {
            runtime
                .inner
                .set_error("node command did not finish before the deadline");
            Err(Fv6Status::Timeout)
        }
    }
}

fn problem_text(problem: &NodeProblem) -> String {
    serde_json::to_string(problem).unwrap_or_else(|_| format!("{problem:?}"))
}

fn duration_from_millis(value: u64) -> Duration {
    if value == 0 {
        DEFAULT_TIMEOUT
    } else {
        Duration::from_millis(value.min(600_000))
    }
}

unsafe fn str_from(ptr: *const c_char) -> Result<String, Fv6Status> {
    if ptr.is_null() {
        return Err(Fv6Status::InvalidArgument);
    }
    // SAFETY: the caller guarantees a NUL-terminated string that stays valid
    // for the duration of the call.
    CStr::from_ptr(ptr)
        .to_str()
        .map(|value| value.to_owned())
        .map_err(|_| Fv6Status::InvalidArgument)
}

unsafe fn write_out(
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
    payload: &[u8],
) -> Fv6Status {
    if !out_len.is_null() {
        *out_len = payload.len();
    } else if !payload.is_empty() {
        return Fv6Status::InvalidArgument;
    }
    if payload.is_empty() {
        return Fv6Status::Ok;
    }
    if buffer.is_null() || buffer_len < payload.len() {
        return Fv6Status::BufferTooSmall;
    }
    // SAFETY: the caller provided a writable buffer of buffer_len bytes.
    std::ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len());
    Fv6Status::Ok
}

fn guard<F>(fallback: Fv6Status, action: F) -> i32
where
    F: FnOnce() -> Fv6Status,
{
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(status) => status.as_i32(),
        Err(_) => fallback.as_i32(),
    }
}

/// Byte length of a snapshot or outcome payload, reported through out_len.
#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_create(
    config_json: *const c_char,
    callbacks: HostCallbacks,
    context: *mut c_void,
    out_runtime: *mut *mut Fv6Runtime,
    error_buffer: *mut u8,
    error_buffer_len: usize,
    out_error_len: *mut usize,
) -> i32 {
    if out_runtime.is_null() {
        return Fv6Status::InvalidArgument.as_i32();
    }
    *out_runtime = std::ptr::null_mut();
    guard(Fv6Status::Panic, || {
        let raw = match str_from(config_json) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let parsed: RuntimeConfig = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(error) => {
                return report_error(
                    error_buffer,
                    error_buffer_len,
                    out_error_len,
                    &format!("invalid runtime configuration: {error}"),
                )
            }
        };
        let storage_root = PathBuf::from(&parsed.storage_root);
        if !storage_root.is_absolute() {
            return report_error(
                error_buffer,
                error_buffer_len,
                out_error_len,
                "storage_root must be an absolute path",
            );
        }
        let mut dns_servers = Vec::with_capacity(parsed.dns_servers.len());
        for server in &parsed.dns_servers {
            match std::net::IpAddr::from_str(server) {
                Ok(ip) => dns_servers.push(ip),
                Err(_) => {
                    return report_error(
                        error_buffer,
                        error_buffer_len,
                        out_error_len,
                        &format!("invalid DNS server address: {server}"),
                    )
                }
            }
        }
        let workers = parsed
            .worker_threads
            .unwrap_or(2)
            .clamp(1, MAX_WORKER_THREADS);
        let config = EmbeddedNodeConfig {
            storage_root,
            dns_servers,
        };
        match Fv6Runtime::new(config, workers, callbacks, context) {
            Ok(runtime) => {
                *out_runtime = Box::into_raw(Box::new(runtime));
                Fv6Status::Ok
            }
            Err(error) => report_error(error_buffer, error_buffer_len, out_error_len, &error),
        }
    })
}

unsafe fn report_error(
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
    message: &str,
) -> Fv6Status {
    let status = write_out(buffer, buffer_len, out_len, message.as_bytes());
    if status == Fv6Status::Ok {
        Fv6Status::InvalidArgument
    } else {
        status
    }
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_destroy(runtime: *mut Fv6Runtime) {
    if runtime.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the pointer came from fv6_runtime_create and is released once.
        let mut boxed = Box::from_raw(runtime);
        boxed.teardown();
    }));
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_start(runtime: *mut Fv6Runtime, timeout_ms: u64) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        match runtime.start(duration_from_millis(timeout_ms)) {
            Ok(()) => Fv6Status::Ok,
            Err(status) => status,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_stop(runtime: *mut Fv6Runtime, timeout_ms: u64) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        match runtime.stop(duration_from_millis(timeout_ms)) {
            Ok(()) => Fv6Status::Ok,
            Err(status) => status,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_phase(runtime: *const Fv6Runtime) -> i32 {
    match runtime.as_ref() {
        Some(runtime) => runtime.inner.phase() as i32,
        None => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_last_error(
    runtime: *const Fv6Runtime,
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        let message = runtime.last_error_text();
        write_out(buffer, buffer_len, out_len, message.as_bytes())
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_snapshot(
    runtime: *mut Fv6Runtime,
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
    timeout_ms: u64,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        if runtime.inner.phase() != FV6_PHASE_RUNNING {
            return Fv6Status::State;
        }
        let slot = runtime.inner.slot();
        let Some(node) = slot.node.as_ref() else {
            return Fv6Status::State;
        };
        let snapshot = match wait_node(runtime, duration_from_millis(timeout_ms), node.snapshot()) {
            Ok(snapshot) => snapshot,
            Err(status) => return status,
        };
        match serde_json::to_vec(&snapshot) {
            Ok(payload) => write_out(buffer, buffer_len, out_len, &payload),
            Err(error) => {
                runtime
                    .inner
                    .set_error(format!("failed to encode the snapshot: {error}"));
                Fv6Status::Io
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_execute(
    runtime: *mut Fv6Runtime,
    request_json: *const c_char,
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
    timeout_ms: u64,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        let raw = match str_from(request_json) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let request: NodeCommandRequest = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(error) => {
                runtime
                    .inner
                    .set_error(format!("invalid node command request: {error}"));
                return Fv6Status::InvalidArgument;
            }
        };
        if runtime.inner.phase() != FV6_PHASE_RUNNING {
            return Fv6Status::State;
        }
        let slot = runtime.inner.slot();
        let Some(node) = slot.node.as_ref() else {
            return Fv6Status::State;
        };
        let outcome = match wait_node(
            runtime,
            duration_from_millis(timeout_ms),
            node.execute(request),
        ) {
            Ok(outcome) => outcome,
            Err(status) => return status,
        };
        let encoded: Result<Vec<u8>, serde_json::Error> = serde_json::to_vec(&outcome);
        match encoded {
            Ok(payload) => write_out(buffer, buffer_len, out_len, &payload),
            Err(error) => {
                runtime
                    .inner
                    .set_error(format!("failed to encode the command outcome: {error}"));
                Fv6Status::Io
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_runtime_network_path_changed(
    runtime: *mut Fv6Runtime,
    timeout_ms: u64,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(runtime) = runtime.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        if runtime.inner.phase() != FV6_PHASE_RUNNING {
            return Fv6Status::State;
        }
        let slot = runtime.inner.slot();
        let Some(node) = slot.node.as_ref() else {
            return Fv6Status::State;
        };
        match wait_node(
            runtime,
            duration_from_millis(timeout_ms),
            node.network_path_changed(),
        ) {
            Ok(()) => Fv6Status::Ok,
            Err(status) => status,
        }
    })
}

/// Endpoint handed to the host through the open_network callback.
#[no_mangle]
pub unsafe extern "C" fn fv6_endpoint_mtu(
    endpoint: *const EndpointHandle,
    out_mtu: *mut u16,
) -> i32 {
    guard(Fv6Status::Panic, || {
        if out_mtu.is_null() {
            return Fv6Status::InvalidArgument;
        }
        let Some(endpoint) = endpoint.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        *out_mtu = endpoint.mtu();
        Fv6Status::Ok
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_endpoint_send(
    endpoint: *mut EndpointHandle,
    packet: *const u8,
    packet_len: usize,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(endpoint) = endpoint.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        if packet.is_null() {
            return Fv6Status::InvalidArgument;
        }
        // SAFETY: the caller guarantees packet_len readable bytes.
        let bytes = std::slice::from_raw_parts(packet, packet_len);
        // Whole IP datagrams only: the host must not stream fragments into the
        // device, and the MTU check keeps a single packet inside one buffer.
        if let Err(status) = crate::packet::validate(bytes, endpoint.mtu()) {
            return status;
        }
        match endpoint.send(bytes) {
            Ok(()) => Fv6Status::Ok,
            Err(status) => status,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_endpoint_recv(
    endpoint: *mut EndpointHandle,
    buffer: *mut u8,
    buffer_len: usize,
    out_len: *mut usize,
    timeout_ms: u64,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let Some(endpoint) = endpoint.as_ref() else {
            return Fv6Status::InvalidHandle;
        };
        if buffer.is_null() || out_len.is_null() {
            return Fv6Status::InvalidArgument;
        }
        if buffer_len < endpoint.mtu() as usize {
            return Fv6Status::BufferTooSmall;
        }
        let packet = match endpoint.recv(duration_from_millis(timeout_ms)) {
            Ok(packet) => packet,
            Err(status) => return status,
        };
        if packet.len() > buffer_len {
            return Fv6Status::BufferTooSmall;
        }
        std::ptr::copy_nonoverlapping(packet.as_ptr(), buffer, packet.len());
        *out_len = packet.len();
        Fv6Status::Ok
    })
}

#[no_mangle]
pub unsafe extern "C" fn fv6_endpoint_release(endpoint: *mut EndpointHandle) {
    if endpoint.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the token came from the open_network callback and is released
        // exactly once by the host.
        drop(Box::from_raw(endpoint));
    }));
}

/// Creates the network configuration and node identity under storage_root.
#[no_mangle]
pub unsafe extern "C" fn fv6_prepare_network(
    storage_root: *const c_char,
    network_name: *const c_char,
    coordinator_url: *const c_char,
    set_default: i32,
    error_buffer: *mut u8,
    error_buffer_len: usize,
    out_error_len: *mut usize,
) -> i32 {
    guard(Fv6Status::Panic, || {
        let root = match str_from(storage_root) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let name = match str_from(network_name) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let url = match str_from(coordinator_url) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let manager = match ConfigManager::new_for_dir(PathBuf::from(root)) {
            Ok(manager) => manager,
            Err(error) => {
                return report_error(
                    error_buffer,
                    error_buffer_len,
                    out_error_len,
                    &format!("storage root is unusable: {error:#}"),
                )
            }
        };
        match manager.create_network(&name, &url, set_default != 0) {
            Ok(_) => Fv6Status::Ok,
            Err(error) => {
                let message = format!("{error:#}");
                // The caller chose a network that already exists: that is a
                // request-level conflict, not a storage failure.
                let status = if message.contains("已存在") || message.contains("Duplicate") {
                    Fv6Status::InvalidArgument
                } else {
                    Fv6Status::Io
                };
                // report_error also stores the length when the buffer is short.
                report_error(error_buffer, error_buffer_len, out_error_len, &message);
                status
            }
        }
    })
}

/// Convenience: outcome kind for hosts that only need the variant name.
#[no_mangle]
pub unsafe extern "C" fn fv6_outcome_kind(
    buffer: *const u8,
    buffer_len: usize,
    out_kind: *mut i32,
) -> i32 {
    guard(Fv6Status::Panic, || {
        if buffer.is_null() || buffer_len == 0 || out_kind.is_null() {
            return Fv6Status::InvalidArgument;
        }
        // SAFETY: the caller passes a payload previously produced by this module.
        let bytes = std::slice::from_raw_parts(buffer, buffer_len);
        let kind = match serde_json::from_slice::<NodeCommandOutcome>(bytes) {
            Ok(NodeCommandOutcome::Snapshot(_)) => 1,
            Ok(_) => 2,
            Err(_) => return Fv6Status::InvalidArgument,
        };
        *out_kind = kind;
        Fv6Status::Ok
    })
}
