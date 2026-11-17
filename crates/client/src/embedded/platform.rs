use std::net::SocketAddr;

use fuckipv6_common::{ClientConfig, VirtualIps};
use futures::future::BoxFuture;

use crate::node::NodeProblem;
use crate::packet_io::{ExternalPacketDevice, PacketDevice};

#[derive(Debug, thiserror::Error)]
#[error("{}", .0.message)]
pub(crate) struct PlatformError(pub(crate) NodeProblem);

/// Configuration resolved by the real Coordinator, not by the platform adapter.
/// `config` includes the requested MTU, routing and DNS policy. The host must
/// apply these to its VPN session before returning a packet device.
#[derive(Debug, Clone)]
pub struct EmbeddedNetworkRequest {
    pub network_name: String,
    pub assigned_ips: VirtualIps,
    pub config: ClientConfig,
    pub transport_addr: SocketAddr,
    pub dns_servers: Vec<std::net::IpAddr>,
}

/// Packet-device factory and system-network configuration boundary. iOS and
/// Android implement this using their own VPN APIs, never desktop commands.
/// An interrupted open must roll back its partial platform work using RAII.
pub trait EmbeddedPlatform: Send + Sync + 'static {
    fn open_network(
        &self,
        request: EmbeddedNetworkRequest,
    ) -> BoxFuture<'_, Result<EmbeddedNetwork, NodeProblem>>;

    /// Join adapter-owned rollback/callback work, including cancelled opens.
    /// This must be idempotent and must not leave detached platform tasks.
    fn shutdown(&self) -> BoxFuture<'_, Result<(), NodeProblem>>;
}

/// Ownership of host IP/routes/DNS/MTU configuration for one network session.
/// Both operations must be idempotent. `cancel` is a synchronous fallback for
/// startup cancellation or panic; `close` confirms cleanup before stop returns.
/// Asynchronous rollback started by cancel stays owned by EmbeddedPlatform and
/// must finish before its shutdown hook returns.
pub trait EmbeddedNetworkLease: Send + Sync + 'static {
    fn close(&mut self) -> BoxFuture<'_, Result<(), NodeProblem>>;
    fn cancel(&self);
}

/// A factory result transfers the packet device and its host network lifetime
/// together. Closing a device never silently recreates an OS VPN session.
pub struct EmbeddedNetwork {
    pub(crate) device: Box<dyn PacketDevice>,
    pub(crate) lease: NetworkLease,
}

impl EmbeddedNetwork {
    pub fn new(device: ExternalPacketDevice, lease: Box<dyn EmbeddedNetworkLease>) -> Self {
        Self {
            device: Box::new(device),
            lease: NetworkLease {
                inner: lease,
                closed: false,
            },
        }
    }
}

pub(crate) struct NetworkLease {
    inner: Box<dyn EmbeddedNetworkLease>,
    closed: bool,
}

impl NetworkLease {
    pub(crate) async fn close(&mut self) -> Result<(), NodeProblem> {
        if !self.closed {
            self.inner.close().await?;
            self.closed = true;
        }
        Ok(())
    }
}

impl Drop for NetworkLease {
    fn drop(&mut self) {
        if !self.closed {
            self.inner.cancel();
        }
    }
}
