//! Platform-neutral, packet-preserving L3 I/O. No interface or route is created
//! by this module. Mobile hosts own their VPN session and supply raw IP bytes.

mod external;

pub use external::{ExternalPacketDevice, ExternalPacketEndpoint};

use anyhow::Result;
use futures::future::BoxFuture;

/// One handle to a full-duplex packet device. Clones share a device lifetime;
/// reads compete for packets, while a reader and writer may run concurrently.
/// Buffers are borrowed only until the returned future completes or is dropped.
/// Successful writes accept exactly one whole packet, never a byte stream.
pub trait PacketDevice: Send + Sync {
    fn name(&self) -> &str;
    fn mtu(&self) -> u16;
    fn clone_box(&self) -> Box<dyn PacketDevice>;

    /// Returns a whole packet's length, or zero for EOF. Implementations must
    /// document cancellation and undersized-buffer behavior.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, Result<usize>>;
    fn write<'a>(&'a mut self, packet: &'a [u8]) -> BoxFuture<'a, Result<()>>;

    /// Wait for the first packet, then optionally drain immediately available
    /// packets. Lengths describe a successful prefix of the supplied buffers.
    fn read_batch<'a>(
        &'a mut self,
        buffers: &'a mut [&mut [u8]],
    ) -> BoxFuture<'a, Result<Vec<usize>>> {
        Box::pin(async move {
            match buffers.first_mut() {
                Some(first) => Ok(vec![self.read(first).await?]),
                None => Ok(Vec::new()),
            }
        })
    }

    /// Each packet has its own result; one failure must not skip later packets.
    /// Cancellation may leave a successfully written prefix; batches are not
    /// transactions and must not be blindly retried.
    fn write_batch<'a>(&'a mut self, packets: &'a [&[u8]]) -> BoxFuture<'a, Vec<Result<()>>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(packets.len());
            for packet in packets {
                results.push(self.write(packet).await);
            }
            results
        })
    }

    /// Release the device using its platform's lifecycle. Runtime owners stop
    /// and join readers first. External devices close all clones immediately;
    /// native TUN adapters retain their existing delete/Drop behavior.
    fn close(&mut self) -> BoxFuture<'_, Result<()>>;
}
