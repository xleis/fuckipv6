//! Host adapter for the embedded runtime.
//!
//! IosPlatform is the only place where Rust calls back into the host. The host
//! applies the derived tunnel settings to its VPN session and keeps the packet
//! endpoint token; Rust keeps the device side. Every callback is invoked from a
//! blocking thread so no Tokio worker is ever stalled by host code.

use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuckipv6_client::embedded::{
    EmbeddedNetwork, EmbeddedNetworkLease, EmbeddedNetworkRequest, EmbeddedPlatform,
};
use fuckipv6_client::node::{NodeProblem, NodeProblemCode};
use fuckipv6_client::packet_io::{ExternalPacketDevice, ExternalPacketEndpoint};
use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::error::{status_from_io, Fv6Status};
use crate::settings::TunnelSettings;

/// Packets buffered per direction inside the P1 device. Matches the bounded
/// queue used by desktop adapters; excess pressure surfaces as
/// Fv6Status::QueueFull instead of unbounded memory growth.
pub const PACKET_QUEUE_CAPACITY: usize = 256;

const ENDPOINT_MAGIC: u64 = 0x4656_365f_454e_4450; // "FV6_ENDP"

/// Applies tunnel settings and stores the endpoint token.
///
/// Returns 0 on success. Any other value is reported to the node as a platform
/// failure, which aborts network startup.
pub type OpenNetworkCallback = unsafe extern "C" fn(
    context: *mut c_void,
    settings_json: *const u8,
    settings_len: usize,
    endpoint_token: usize,
) -> i32;

/// Tears the tunnel session down. Must be idempotent and must not block
/// indefinitely: the runtime calls it from its bounded shutdown path.
pub type CloseNetworkCallback = unsafe extern "C" fn(context: *mut c_void) -> i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HostCallbacks {
    pub open_network: Option<OpenNetworkCallback>,
    pub close_network: Option<CloseNetworkCallback>,
}

/// Shared host bridge. The context pointer is owned by the host and must stay
/// valid until fv6_runtime_destroy returns; the host serialises callback entry
/// from its own side.
pub struct HostBridge {
    callbacks: HostCallbacks,
    context: usize,
    handle: Handle,
    sessions: AtomicUsize,
    closed: AtomicBool,
}

// SAFETY: the host contract requires the context pointer and callbacks to be
// callable from any thread (Swift dispatches onto its own serial queue).
unsafe impl Send for HostBridge {}
unsafe impl Sync for HostBridge {}

impl HostBridge {
    pub fn new(callbacks: HostCallbacks, context: *mut c_void, handle: Handle) -> Arc<Self> {
        Arc::new(Self {
            callbacks,
            context: context as usize,
            handle,
            sessions: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        })
    }

    /// Applies settings through the host callback. Runs on a blocking thread.
    fn apply(&self, settings: &[u8], token: usize) -> Result<(), NodeProblem> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(problem(
                NodeProblemCode::Unavailable,
                "host adapter was shut down before the tunnel was applied",
            ));
        }
        let Some(callback) = self.callbacks.open_network else {
            return Err(problem(
                NodeProblemCode::Unsupported,
                "host does not implement a tunnel configuration callback",
            ));
        };
        let status = unsafe {
            callback(
                self.context as *mut c_void,
                settings.as_ptr(),
                settings.len(),
                token,
            )
        };
        if status == 0 {
            self.sessions.fetch_add(1, Ordering::SeqCst);
            Ok(())
        } else {
            Err(problem(
                NodeProblemCode::Unavailable,
                format!("host rejected the tunnel configuration (status {status})"),
            ))
        }
    }

    /// Closes the host-side session. Runs on a blocking thread; idempotent.
    fn close_session(&self) -> Result<(), NodeProblem> {
        if self.sessions.swap(0, Ordering::SeqCst) == 0 {
            return Ok(());
        }
        let Some(callback) = self.callbacks.close_network else {
            return Err(problem(
                NodeProblemCode::Unsupported,
                "host does not implement a tunnel teardown callback",
            ));
        };
        let status = unsafe { callback(self.context as *mut c_void) };
        if status == 0 {
            Ok(())
        } else {
            Err(problem(
                NodeProblemCode::Unavailable,
                format!("host tunnel teardown failed (status {status})"),
            ))
        }
    }

    /// Final teardown used by EmbeddedPlatform::shutdown.
    fn shutdown(&self) -> Result<(), NodeProblem> {
        let result = self.close_session();
        self.closed.store(true, Ordering::SeqCst);
        result
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

/// Host-side packet endpoint. Allocated by Rust, owned by the host after a
/// successful open_network callback, released exactly once through
/// fv6_endpoint_release.
pub struct EndpointHandle {
    magic: u64,
    pub(crate) endpoint: ExternalPacketEndpoint,
    pub(crate) handle: Handle,
    pub(crate) mtu: u16,
}

impl EndpointHandle {
    pub(crate) fn new(endpoint: ExternalPacketEndpoint, handle: Handle, mtu: u16) -> Self {
        Self {
            magic: ENDPOINT_MAGIC,
            endpoint,
            handle,
            mtu,
        }
    }

    pub fn mtu(&self) -> u16 {
        self.mtu
    }

    pub fn send(&self, packet: &[u8]) -> Result<(), Fv6Status> {
        match self.endpoint.try_send(packet) {
            Ok(()) => Ok(()),
            Err(error) => Err(status_from_io(error.kind())),
        }
    }

    /// Blocking receive with a bounded wait. Never called from inside a Tokio
    /// runtime context; the FFI layer rejects that case beforehand.
    pub fn recv(&self, timeout: Duration) -> Result<Vec<u8>, Fv6Status> {
        if timeout.is_zero() {
            return self
                .endpoint
                .try_recv()
                .map_err(|error| status_from_io(error.kind()));
        }
        let endpoint = &self.endpoint;
        self.handle.block_on(async move {
            match tokio::time::timeout(timeout, endpoint.recv()).await {
                Ok(Ok(packet)) => Ok(packet),
                Ok(Err(error)) => Err(status_from_io(error.kind())),
                Err(_) => Err(Fv6Status::Timeout),
            }
        })
    }
}

impl Drop for EndpointHandle {
    fn drop(&mut self) {
        self.magic = 0;
        self.endpoint.close();
    }
}

/// EmbeddedPlatform implementation used by the iOS host.
pub struct IosPlatform {
    host: Arc<HostBridge>,
}

impl IosPlatform {
    pub fn new(host: Arc<HostBridge>) -> Self {
        Self { host }
    }
}

impl EmbeddedPlatform for IosPlatform {
    fn open_network(
        &self,
        request: EmbeddedNetworkRequest,
    ) -> BoxFuture<'_, Result<EmbeddedNetwork, NodeProblem>> {
        Box::pin(async move {
            let settings = TunnelSettings::derive(&request)?;
            let settings_json = serde_json::to_vec(&settings).map_err(|error| {
                problem(
                    NodeProblemCode::Internal,
                    format!("failed to encode tunnel settings: {error}"),
                )
            })?;
            let name = request.config.tun.name.clone();
            let (device, endpoint) =
                ExternalPacketDevice::new(name, settings.mtu, PACKET_QUEUE_CAPACITY).map_err(
                    |error| {
                        problem(
                            NodeProblemCode::Unavailable,
                            format!("failed to create the host packet device: {error}"),
                        )
                    },
                )?;
            let token = Box::into_raw(Box::new(EndpointHandle::new(
                endpoint,
                self.host.handle(),
                settings.mtu,
            ))) as usize;
            let host = self.host.clone();
            let applied = tokio::task::spawn_blocking(move || host.apply(&settings_json, token))
                .await
                .map_err(|error| {
                    // SAFETY: the token was never handed to the host.
                    unsafe { drop(Box::from_raw(token as *mut EndpointHandle)) };
                    problem(
                        NodeProblemCode::Internal,
                        format!("host configuration task failed: {error}"),
                    )
                })?;
            if applied.is_err() {
                // SAFETY: the host refused the configuration, so it holds no
                // ownership of the token.
                unsafe { drop(Box::from_raw(token as *mut EndpointHandle)) };
                applied?;
            }
            Ok(EmbeddedNetwork::new(
                device,
                Box::new(IosLease {
                    host: self.host.clone(),
                    released: AtomicBool::new(false),
                }),
            ))
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async move {
            let host = self.host.clone();
            tokio::task::spawn_blocking(move || host.shutdown())
                .await
                .map_err(|error| {
                    problem(
                        NodeProblemCode::Internal,
                        format!("host teardown task failed: {error}"),
                    )
                })?
        })
    }
}

/// Host session lifetime owned by the embedded runtime.
struct IosLease {
    host: Arc<HostBridge>,
    released: AtomicBool,
}

impl EmbeddedNetworkLease for IosLease {
    fn close(&mut self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        let already = self.released.swap(true, Ordering::SeqCst);
        let host = self.host.clone();
        Box::pin(async move {
            if already {
                return Ok(());
            }
            tokio::task::spawn_blocking(move || host.close_session())
                .await
                .map_err(|error| {
                    problem(
                        NodeProblemCode::Internal,
                        format!("host teardown task failed: {error}"),
                    )
                })?
        })
    }

    fn cancel(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        let host = self.host.clone();
        // Best-effort rollback from a synchronous context. The host callback is
        // required to be idempotent, and shutdown re-checks the session counter
        // before returning.
        self.host.handle().spawn_blocking(move || {
            let _ = host.close_session();
        });
    }
}

fn problem(code: NodeProblemCode, message: impl Into<String>) -> NodeProblem {
    NodeProblem::new(code, message)
}
