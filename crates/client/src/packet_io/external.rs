use super::PacketDevice;
use anyhow::Result;
use futures::future::BoxFuture;
use std::collections::VecDeque;
use std::io::{self, ErrorKind};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Copy)]
enum Direction {
    ToKernel = 0,
    FromKernel = 1,
}

struct Queues {
    packets: [VecDeque<Vec<u8>>; 2],
    closed: bool,
}

struct SharedDevice {
    name: String,
    mtu: u16,
    capacity: usize,
    queues: Mutex<Queues>,
    changed: watch::Sender<()>,
}

impl SharedDevice {
    fn close(&self) {
        let mut queues = self.queues.lock().expect("packet queues poisoned");
        queues.closed = true;
        for queue in &mut queues.packets {
            queue.clear();
        }
        self.changed.send_replace(());
    }

    fn try_send(&self, direction: Direction, packet: &[u8]) -> io::Result<()> {
        validate_packet(packet, self.mtu)?;
        let mut queues = self.queues.lock().expect("packet queues poisoned");
        if queues.closed {
            return Err(ErrorKind::BrokenPipe.into());
        }
        let queue = &mut queues.packets[direction as usize];
        if queue.len() == self.capacity {
            return Err(ErrorKind::WouldBlock.into());
        }
        // Copy only after admission: cancellation while waiting retains the
        // caller's bytes and never allocates another queued packet.
        queue.push_back(packet.to_vec());
        self.changed.send_replace(());
        Ok(())
    }

    async fn send(&self, direction: Direction, packet: &[u8]) -> io::Result<()> {
        let mut changed = self.changed.subscribe();
        loop {
            match self.try_send(direction, packet) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    let _ = changed.changed().await;
                }
                result => return result,
            }
        }
    }

    fn try_recv(&self, direction: Direction) -> io::Result<Vec<u8>> {
        let mut queues = self.queues.lock().expect("packet queues poisoned");
        if queues.closed {
            return Err(ErrorKind::BrokenPipe.into());
        }
        let packet = queues.packets[direction as usize]
            .pop_front()
            .ok_or(ErrorKind::WouldBlock)?;
        self.changed.send_replace(());
        Ok(packet)
    }

    async fn recv(&self, direction: Direction) -> io::Result<Vec<u8>> {
        // Subscribe before checking the queue, so a concurrent state change
        // between the check and await cannot be lost.
        let mut changed = self.changed.subscribe();
        loop {
            match self.try_recv(direction) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    let _ = changed.changed().await;
                }
                result => return result,
            }
        }
    }
}

// Each side has its own lifetime shared by its clones. Dropping the final
// handle on either side closes both directions without spawning cleanup work.
struct SideLifetime(Arc<SharedDevice>);

impl Drop for SideLifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Rust-facing half of an externally managed VPN session. Each direction holds
/// at most `capacity` owned packets of at most `mtu` bytes (plus active callers).
/// No Ethernet, utun family header, or host-specific metadata crosses this API.
///
/// Close is idempotent, discards queued packets, wakes every waiter with
/// `BrokenPipe`, and affects all clones. Dropping the last handle on either
/// side also closes. A closed pair cannot reopen: the host creates a fresh pair
/// and explicitly replaces the runtime device after restoring its VPN session.
#[derive(Clone)]
pub struct ExternalPacketDevice {
    lifetime: Arc<SideLifetime>,
}

/// Host-facing adapter for both NEPacketTunnelFlow and VpnService. `send`
/// feeds Rust; `recv` takes packets Rust wrote back toward the operating system.
/// The host must keep draining output while active. Without runtime-owned I/O
/// cancellation, close this endpoint before awaiting shutdown. EmbeddedNodeRuntime
/// cancels blocked I/O and closes the device itself. An invalid OS VPN session
/// must always close its endpoint.
#[derive(Clone)]
pub struct ExternalPacketEndpoint {
    lifetime: Arc<SideLifetime>,
}

impl ExternalPacketDevice {
    /// Dual-stack MTUs must be at least 1280. Capacity is per direction and
    /// must be nonzero. Construction never opens a device or modifies routes.
    pub fn new(
        name: impl Into<String>,
        mtu: u16,
        capacity: usize,
    ) -> io::Result<(Self, ExternalPacketEndpoint)> {
        let name = name.into();
        if name.trim().is_empty() || mtu < 1280 || capacity == 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "packet device requires a name, MTU >= 1280 and nonzero capacity",
            ));
        }
        let shared = Arc::new(SharedDevice {
            name,
            mtu,
            capacity,
            queues: Mutex::new(Queues {
                packets: Default::default(),
                closed: false,
            }),
            changed: watch::channel(()).0,
        });
        Ok((
            Self {
                lifetime: Arc::new(SideLifetime(shared.clone())),
            },
            ExternalPacketEndpoint {
                lifetime: Arc::new(SideLifetime(shared)),
            },
        ))
    }
}

impl ExternalPacketEndpoint {
    /// Copy a whole packet after capacity becomes available. Dropping this
    /// future before completion sends nothing and leaves the caller's bytes
    /// untouched. No lock is held across an await, and no worker is spawned.
    pub async fn send(&self, packet: &[u8]) -> io::Result<()> {
        self.lifetime.0.send(Direction::ToKernel, packet).await
    }

    /// Nonblocking admission, returning WouldBlock without copying when full.
    pub fn try_send(&self, packet: &[u8]) -> io::Result<()> {
        self.lifetime.0.try_send(Direction::ToKernel, packet)
    }

    /// Transfer ownership of one output packet to the host. Cancellation while
    /// waiting consumes nothing. Concurrent receivers compete for whole packets.
    pub async fn recv(&self) -> io::Result<Vec<u8>> {
        self.lifetime.0.recv(Direction::FromKernel).await
    }

    pub fn try_recv(&self) -> io::Result<Vec<u8>> {
        self.lifetime.0.try_recv(Direction::FromKernel)
    }

    pub fn close(&self) {
        self.lifetime.0.close();
    }
}

impl PacketDevice for ExternalPacketDevice {
    fn name(&self) -> &str {
        &self.lifetime.0.name
    }

    fn mtu(&self) -> u16 {
        self.lifetime.0.mtu
    }

    fn clone_box(&self) -> Box<dyn PacketDevice> {
        Box::new(self.clone())
    }

    /// Every receive buffer must hold the configured MTU. Reject a small buffer
    /// before dequeuing anything; packets are never truncated or lost on error.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let lengths = self.read_batch(&mut [buffer]).await?;
            Ok(lengths[0])
        })
    }

    fn read_batch<'a>(
        &'a mut self,
        buffers: &'a mut [&mut [u8]],
    ) -> BoxFuture<'a, Result<Vec<usize>>> {
        Box::pin(async move {
            if buffers
                .iter()
                .any(|buffer| buffer.len() < self.mtu().into())
            {
                return Err(
                    io::Error::new(ErrorKind::InvalidInput, "buffer smaller than MTU").into(),
                );
            }
            let Some((first, rest)) = buffers.split_first_mut() else {
                return Ok(Vec::new());
            };
            let shared = &self.lifetime.0;
            let packet = shared.recv(Direction::ToKernel).await?;
            first[..packet.len()].copy_from_slice(&packet);
            let mut lengths = vec![packet.len()];
            for buffer in rest {
                // A close after the first packet returns the successful prefix.
                match shared.try_recv(Direction::ToKernel) {
                    Ok(packet) => {
                        buffer[..packet.len()].copy_from_slice(&packet);
                        lengths.push(packet.len());
                    }
                    Err(_) => break,
                }
            }
            Ok(lengths)
        })
    }

    fn write<'a>(&'a mut self, packet: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.lifetime.0.send(Direction::FromKernel, packet).await?;
            Ok(())
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.lifetime.0.close();
            Ok(())
        })
    }
}

fn validate_packet(packet: &[u8], mtu: u16) -> io::Result<()> {
    if packet.len() > usize::from(mtu) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "packet exceeds MTU",
        ));
    }
    let valid = match packet.first().map(|byte| byte >> 4) {
        Some(4) if packet.len() >= 20 => {
            let header_len = usize::from(packet[0] & 0x0f) * 4;
            header_len >= 20
                && header_len <= packet.len()
                && usize::from(u16::from_be_bytes([packet[2], packet[3]])) == packet.len()
        }
        Some(6) if packet.len() >= 40 => {
            usize::from(u16::from_be_bytes([packet[4], packet[5]])) + 40 == packet.len()
        }
        _ => false,
    };
    if !valid {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid raw IPv4/IPv6 packet length or header",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{pin_mut, poll};

    fn packet(version: u8, len: usize) -> Vec<u8> {
        let mut packet = vec![0; len];
        if version == 4 {
            packet[0] = 0x45;
            packet[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        } else {
            packet[0] = 0x60;
            packet[4..6].copy_from_slice(&((len - 40) as u16).to_be_bytes());
        }
        packet
    }

    fn kind(error: anyhow::Error) -> ErrorKind {
        error.downcast::<io::Error>().unwrap().kind()
    }

    #[test]
    fn packet_io_rejects_invalid_configuration() {
        assert!(ExternalPacketDevice::new("", 1280, 1).is_err());
        assert!(ExternalPacketDevice::new("mock", 1279, 1).is_err());
        assert!(ExternalPacketDevice::new("mock", 1280, 0).is_err());
    }

    #[tokio::test]
    async fn packet_io_dual_stack_owned_packets_and_batches() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 4).unwrap();
        let ipv4 = packet(4, 20);
        let ipv6 = packet(6, 48);
        let mut caller_owned = ipv4.clone();
        host.send(&caller_owned).await.unwrap();
        caller_owned.fill(0xff);
        host.send(&ipv6).await.unwrap();

        let mut buffers = [[0u8; 1280]; 3];
        let mut slices: Vec<_> = buffers.iter_mut().map(|buffer| &mut buffer[..]).collect();
        let lengths = device.read_batch(&mut slices).await.unwrap();
        assert_eq!(lengths, [20, 48]);
        assert_eq!(&slices[0][..20], ipv4);
        assert_eq!(&slices[1][..48], ipv6);
        assert!(device.read_batch(&mut []).await.unwrap().is_empty());
        assert!(device.write_batch(&[]).await.is_empty());

        let results = device.write_batch(&[&ipv6, &ipv4]).await;
        assert!(results.into_iter().all(|result| result.is_ok()));
        assert_eq!(host.recv().await.unwrap(), ipv6);
        assert_eq!(host.recv().await.unwrap(), ipv4);
        assert_eq!(host.try_recv().unwrap_err().kind(), ErrorKind::WouldBlock);
    }

    #[tokio::test]
    async fn packet_io_validates_mtu_and_headers_in_both_directions() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 2).unwrap();
        for valid in [packet(4, 1280), packet(6, 1280)] {
            host.try_send(&valid).unwrap();
            let mut buffer = [0; 1280];
            assert_eq!(device.read(&mut buffer).await.unwrap(), 1280);
            assert_eq!(buffer.as_slice(), valid);
            device.write(&valid).await.unwrap();
            assert_eq!(host.recv().await.unwrap(), valid);
        }

        let mut bad_ihl = packet(4, 20);
        bad_ihl[0] = 0x44;
        let mut long_ihl = packet(4, 20);
        long_ihl[0] = 0x4f;
        let mut bad_v4_length = packet(4, 20);
        bad_v4_length[3] = 21;
        let mut bad_v6_length = packet(6, 40);
        bad_v6_length[5] = 1;
        for invalid in [
            vec![],
            vec![0x70; 40],
            vec![0x45; 19],
            vec![0x60; 39],
            bad_ihl,
            long_ihl,
            bad_v4_length,
            bad_v6_length,
        ] {
            assert_eq!(
                host.try_send(&invalid).unwrap_err().kind(),
                ErrorKind::InvalidData
            );
            assert_eq!(
                kind(device.write(&invalid).await.unwrap_err()),
                ErrorKind::InvalidData
            );
        }
        let oversized = packet(6, 1281);
        assert_eq!(
            host.try_send(&oversized).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            kind(device.write(&oversized).await.unwrap_err()),
            ErrorKind::InvalidInput
        );

        let valid = packet(6, 40);
        let results = device.write_batch(&[&[], &valid]).await;
        assert!(results[0].is_err());
        assert!(results[1].is_ok());
        assert_eq!(host.recv().await.unwrap(), valid);
    }

    #[tokio::test]
    async fn packet_io_small_buffers_do_not_consume_packets() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 2).unwrap();
        let ipv4 = packet(4, 20);
        host.try_send(&ipv4).unwrap();
        let mut short = [0; 20];
        assert_eq!(
            kind(device.read(&mut short).await.unwrap_err()),
            ErrorKind::InvalidInput
        );
        let mut full = [0; 1280];
        assert!(device
            .read_batch(&mut [&mut full, &mut short])
            .await
            .is_err());
        assert_eq!(device.read(&mut full).await.unwrap(), ipv4.len());
        assert_eq!(&full[..ipv4.len()], ipv4);
    }

    #[tokio::test]
    async fn packet_io_backpressure_and_cancelled_writes_preserve_ownership() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let first = packet(4, 20);
        let second = packet(6, 40);
        host.try_send(&first).unwrap();
        assert_eq!(
            host.try_send(&second).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        {
            let send = host.send(&second);
            pin_mut!(send);
            assert!(poll!(send.as_mut()).is_pending());
        }
        let mut buffer = [0; 1280];
        assert_eq!(device.read(&mut buffer).await.unwrap(), first.len());
        // The cancelled packet was not secretly admitted after space freed.
        {
            let read = device.read(&mut buffer);
            pin_mut!(read);
            assert!(poll!(read.as_mut()).is_pending());
        }

        device.write(&first).await.unwrap();
        {
            let write = device.write(&second);
            pin_mut!(write);
            assert!(poll!(write.as_mut()).is_pending());
        }
        assert_eq!(host.recv().await.unwrap(), first);
        assert_eq!(host.try_recv().unwrap_err().kind(), ErrorKind::WouldBlock);
        device.write(&second).await.unwrap();
        assert_eq!(host.recv().await.unwrap(), second);
    }

    #[tokio::test]
    async fn packet_io_capacity_release_wakes_both_writers() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let data = packet(6, 40);
        host.try_send(&data).unwrap();
        {
            let send = host.send(&data);
            pin_mut!(send);
            assert!(poll!(send.as_mut()).is_pending());
            device.read(&mut [0; 1280]).await.unwrap();
            send.await.unwrap();
        }
        device.write(&data).await.unwrap();
        let write = device.write(&data);
        pin_mut!(write);
        assert!(poll!(write.as_mut()).is_pending());
        host.recv().await.unwrap();
        write.await.unwrap();
        assert_eq!(host.recv().await.unwrap(), data);
    }

    #[tokio::test]
    async fn packet_io_close_wakes_full_queue_writers_and_discards_packets() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let data = packet(6, 40);
        host.try_send(&data).unwrap();
        device.write(&data).await.unwrap();
        {
            let input = host.send(&data);
            let output = device.write(&data);
            pin_mut!(input, output);
            assert!(poll!(input.as_mut()).is_pending());
            assert!(poll!(output.as_mut()).is_pending());
            host.close();
            host.close();
            assert_eq!(input.await.unwrap_err().kind(), ErrorKind::BrokenPipe);
            assert_eq!(kind(output.await.unwrap_err()), ErrorKind::BrokenPipe);
        }
        device.close().await.unwrap();
        device.close().await.unwrap();
        let shared = &device.lifetime.0;
        assert!(shared
            .queues
            .lock()
            .unwrap()
            .packets
            .iter()
            .all(VecDeque::is_empty));
        assert_eq!(host.recv().await.unwrap_err().kind(), ErrorKind::BrokenPipe);
        assert_eq!(
            kind(device.read(&mut [0; 1280]).await.unwrap_err()),
            ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn packet_io_cancelled_reads_and_close_wake_empty_queue_readers() {
        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let mut buffer = [0; 1280];
        {
            let read = device.read(&mut buffer);
            let recv = host.recv();
            pin_mut!(read, recv);
            assert!(poll!(read.as_mut()).is_pending());
            assert!(poll!(recv.as_mut()).is_pending());
        }
        let data = packet(4, 20);
        host.try_send(&data).unwrap();
        assert_eq!(device.read(&mut buffer).await.unwrap(), data.len());
        device.write(&data).await.unwrap();
        assert_eq!(host.recv().await.unwrap(), data);

        let read = device.read(&mut buffer);
        let recv = host.recv();
        pin_mut!(read, recv);
        assert!(poll!(read.as_mut()).is_pending());
        assert!(poll!(recv.as_mut()).is_pending());
        host.close();
        assert_eq!(kind(read.await.unwrap_err()), ErrorKind::BrokenPipe);
        assert_eq!(recv.await.unwrap_err().kind(), ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn packet_io_last_handle_drop_closes_peer_and_releases_resources() {
        let (device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let weak = Arc::downgrade(&device.lifetime.0);
        let clone = device.clone_box();
        drop(device);
        host.try_send(&packet(4, 20)).unwrap();
        drop(clone);
        assert_eq!(host.try_recv().unwrap_err().kind(), ErrorKind::BrokenPipe);
        assert!(weak
            .upgrade()
            .unwrap()
            .queues
            .lock()
            .unwrap()
            .packets
            .iter()
            .all(VecDeque::is_empty));
        drop(host);
        assert!(weak.upgrade().is_none());

        let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
        let clone = host.clone();
        drop(host);
        device.write(&packet(6, 40)).await.unwrap();
        drop(clone);
        assert_eq!(
            kind(device.write(&packet(6, 40)).await.unwrap_err()),
            ErrorKind::BrokenPipe
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_io_concurrent_duplex_has_no_lost_wakeups() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut device, host) = ExternalPacketDevice::new("mock", 1280, 1).unwrap();
            let kernel = tokio::spawn(async move {
                let mut buffer = [0; 1280];
                for _ in 0..256 {
                    let size = device.read(&mut buffer).await.unwrap();
                    device.write(&buffer[..size]).await.unwrap();
                }
                // Keep this side alive until the host takes the final packet.
                device
            });
            for sequence in 0..256u16 {
                let mut data = packet(6, 42);
                data[40..42].copy_from_slice(&sequence.to_be_bytes());
                host.send(&data).await.unwrap();
                assert_eq!(host.recv().await.unwrap(), data);
            }
            drop(kernel.await.unwrap());
        })
        .await
        .unwrap();
    }
}
