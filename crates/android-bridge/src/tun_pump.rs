//! Bidirectional packet pump between an Android `VpnService` TUN file descriptor
//! and [`ExternalPacketEndpoint`].
//!
//! Uses a dedicated reader thread driven by `poll(2)` on both the TUN fd and a
//! self-pipe so shutdown is immediate and never leaks blocked descriptors.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Default)]
pub struct TunnelCounters {
    pub rx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_packets: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub dropped_packets: AtomicU64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TunnelCountersSnapshot {
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub dropped_packets: u64,
}

impl TunnelCounters {
    pub fn snapshot(&self) -> TunnelCountersSnapshot {
        TunnelCountersSnapshot {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            dropped_packets: self.dropped_packets.load(Ordering::Relaxed),
        }
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::thread::JoinHandle;

    use fuckipv6_client::packet_io::ExternalPacketEndpoint;
    use tokio::runtime::Handle;

    pub struct TunPump {
        stopped: Arc<AtomicBool>,
        wake_write: OwnedFd,
        endpoint: ExternalPacketEndpoint,
        reader_thread: Option<JoinHandle<()>>,
        writer_task: Option<tokio::task::JoinHandle<()>>,
        tun_fd: Option<Arc<OwnedFd>>,
    }

    impl TunPump {
        /// Takes ownership of `raw_tun_fd` (e.g. from Android `ParcelFileDescriptor.detachFd()`).
        /// On error, closes `raw_tun_fd` before returning so no descriptor leaks.
        pub fn start(
            raw_tun_fd: RawFd,
            mtu: u16,
            endpoint: ExternalPacketEndpoint,
            counters: Arc<TunnelCounters>,
            runtime: Handle,
        ) -> io::Result<Self> {
            if raw_tun_fd < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "TUN file descriptor must be non-negative",
                ));
            }
            // SAFETY: caller transfers exclusive ownership of the detached fd.
            let tun_fd = Arc::new(unsafe { OwnedFd::from_raw_fd(raw_tun_fd) });

            let mut pipe_fds = [-1 as RawFd; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: pipe(2) succeeded and returned two freshly owned fds.
            let wake_read = unsafe { OwnedFd::from_raw_fd(pipe_fds[0]) };
            let wake_write = unsafe { OwnedFd::from_raw_fd(pipe_fds[1]) };
            set_cloexec(wake_read.as_raw_fd());
            set_cloexec(wake_write.as_raw_fd());
            set_nonblocking(wake_read.as_raw_fd());

            let stopped = Arc::new(AtomicBool::new(false));
            let reader_stopped = stopped.clone();
            let reader_fd = tun_fd.clone();
            let reader_endpoint = endpoint.clone();
            let reader_counters = counters.clone();
            let reader_runtime = runtime.clone();
            let buf_cap = usize::from(mtu).max(2048) + 64;

            let reader_thread = std::thread::Builder::new()
                .name("fv6-tun-rx".to_owned())
                .spawn(move || {
                    let mut buf = vec![0u8; buf_cap];
                    let mut pfds = [
                        libc::pollfd {
                            fd: reader_fd.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: wake_read.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    while !reader_stopped.load(Ordering::Acquire) {
                        pfds[0].revents = 0;
                        pfds[1].revents = 0;
                        let ready = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
                        if ready < 0 {
                            let err = io::Error::last_os_error();
                            if err.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            break;
                        }
                        if (pfds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0 {
                            break;
                        }
                        if (pfds[0].revents & (libc::POLLERR | libc::POLLNVAL | libc::POLLHUP)) != 0
                        {
                            break;
                        }
                        if (pfds[0].revents & libc::POLLIN) == 0 {
                            continue;
                        }
                        let read_len = unsafe {
                            libc::read(
                                reader_fd.as_raw_fd(),
                                buf.as_mut_ptr().cast::<libc::c_void>(),
                                buf.len(),
                            )
                        };
                        if read_len < 0 {
                            let err = io::Error::last_os_error();
                            if err.kind() == io::ErrorKind::Interrupted
                                || err.kind() == io::ErrorKind::WouldBlock
                            {
                                continue;
                            }
                            break;
                        }
                        if read_len == 0 {
                            break;
                        }
                        let packet = &buf[..read_len as usize];
                        if !is_valid_ip_packet(packet, mtu) {
                            reader_counters
                                .dropped_packets
                                .fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        match reader_endpoint.try_send(packet) {
                            Ok(()) => {
                                reader_counters.rx_packets.fetch_add(1, Ordering::Relaxed);
                                reader_counters
                                    .rx_bytes
                                    .fetch_add(packet.len() as u64, Ordering::Relaxed);
                            }
                            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                                // Wait briefly on backpressure rather than dropping burst traffic.
                                let sent = reader_runtime.block_on(async {
                                    tokio::time::timeout(
                                        std::time::Duration::from_millis(50),
                                        reader_endpoint.send(packet),
                                    )
                                    .await
                                });
                                match sent {
                                    Ok(Ok(())) => {
                                        reader_counters.rx_packets.fetch_add(1, Ordering::Relaxed);
                                        reader_counters
                                            .rx_bytes
                                            .fetch_add(packet.len() as u64, Ordering::Relaxed);
                                    }
                                    Ok(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe => break,
                                    _ => {
                                        reader_counters
                                            .dropped_packets
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
                            Err(_) => {
                                reader_counters
                                    .dropped_packets
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    reader_endpoint.close();
                })?;

            let writer_stopped = stopped.clone();
            let writer_fd = tun_fd.clone();
            let writer_endpoint = endpoint.clone();
            let writer_counters = counters;

            let writer_task = runtime.spawn(async move {
                while !writer_stopped.load(Ordering::Acquire) {
                    let packet = match writer_endpoint.recv().await {
                        Ok(packet) => packet,
                        Err(_) => break,
                    };
                    if writer_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let fd = writer_fd.as_raw_fd();
                    let written = unsafe {
                        libc::write(fd, packet.as_ptr().cast::<libc::c_void>(), packet.len())
                    };
                    if written < 0 {
                        let err = io::Error::last_os_error();
                        if err.kind() == io::ErrorKind::Interrupted
                            || err.kind() == io::ErrorKind::WouldBlock
                        {
                            continue;
                        }
                        break;
                    }
                    writer_counters.tx_packets.fetch_add(1, Ordering::Relaxed);
                    writer_counters
                        .tx_bytes
                        .fetch_add(written as u64, Ordering::Relaxed);
                }
            });

            Ok(Self {
                stopped,
                wake_write,
                endpoint,
                reader_thread: Some(reader_thread),
                writer_task: Some(writer_task),
                tun_fd: Some(tun_fd),
            })
        }

        pub fn signal_stop(&self) {
            if !self.stopped.swap(true, Ordering::SeqCst) {
                self.endpoint.close();
                let byte = [1u8; 1];
                let _ = unsafe {
                    libc::write(
                        self.wake_write.as_raw_fd(),
                        byte.as_ptr().cast::<libc::c_void>(),
                        1,
                    )
                };
            }
        }

        pub async fn stop(mut self) {
            self.signal_stop();
            if let Some(task) = self.writer_task.take() {
                let _ = task.await;
            }
            if let Some(handle) = self.reader_thread.take() {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = handle.join();
                })
                .await;
            }
            // Drop last Arc<OwnedFd> cleanly after both reader and writer exit.
            self.tun_fd.take();
        }
    }

    impl Drop for TunPump {
        fn drop(&mut self) {
            self.signal_stop();
            if let Some(task) = self.writer_task.take() {
                task.abort();
            }
            if let Some(handle) = self.reader_thread.take() {
                let _ = handle.join();
            }
            self.tun_fd.take();
        }
    }

    fn set_cloexec(fd: RawFd) {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags >= 0 {
            let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        }
    }

    fn set_nonblocking(fd: RawFd) {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags >= 0 {
            let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        }
    }

    fn is_valid_ip_packet(packet: &[u8], mtu: u16) -> bool {
        if packet.is_empty() || packet.len() > usize::from(mtu) {
            return false;
        }
        match packet[0] >> 4 {
            4 if packet.len() >= 20 => {
                let ihl = usize::from(packet[0] & 0x0f) * 4;
                let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
                ihl >= 20 && ihl <= packet.len() && total_len == packet.len()
            }
            6 if packet.len() >= 40 => {
                let payload_len = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
                payload_len + 40 == packet.len()
            }
            _ => false,
        }
    }
}

#[cfg(unix)]
pub use unix_impl::TunPump;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use fuckipv6_client::packet_io::{ExternalPacketDevice, PacketDevice};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    fn sample_ipv4_packet(len: usize) -> Vec<u8> {
        let mut pkt = vec![0u8; len];
        pkt[0] = 0x45;
        pkt[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        pkt
    }

    fn sample_ipv6_packet(len: usize) -> Vec<u8> {
        let mut pkt = vec![0u8; len];
        pkt[0] = 0x60;
        pkt[4..6].copy_from_slice(&((len - 40) as u16).to_be_bytes());
        pkt
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tun_pump_forwards_ipv4_and_ipv6_duplex_and_stops_cleanly() {
        let mut fds = [-1 as RawFd; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) },
            0
        );
        let peer_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let tun_raw_fd = fds[1];

        let (mut device, endpoint) = ExternalPacketDevice::new("android-tun0", 1400, 16).unwrap();
        let counters = Arc::new(TunnelCounters::default());
        let pump = TunPump::start(
            tun_raw_fd,
            1400,
            endpoint,
            counters.clone(),
            tokio::runtime::Handle::current(),
        )
        .expect("start pump");

        let v4 = sample_ipv4_packet(40);
        let v6 = sample_ipv6_packet(64);

        assert_eq!(
            unsafe {
                libc::write(
                    peer_fd.as_raw_fd(),
                    v4.as_ptr().cast::<libc::c_void>(),
                    v4.len(),
                )
            },
            v4.len() as isize
        );
        let mut read_buf = vec![0u8; 1400];
        let n = device.read(&mut read_buf).await.expect("read v4");
        assert_eq!(&read_buf[..n], v4.as_slice());

        device.write(&v6).await.expect("write v6");
        let mut out_buf = vec![0u8; 1400];
        let out_n = unsafe {
            libc::read(
                peer_fd.as_raw_fd(),
                out_buf.as_mut_ptr().cast::<libc::c_void>(),
                out_buf.len(),
            )
        };
        assert_eq!(out_n as usize, v6.len());
        assert_eq!(&out_buf[..out_n as usize], v6.as_slice());

        // Invalid packets are dropped cleanly without closing the pump.
        let bogus = [0x11u8; 8];
        assert_eq!(
            unsafe {
                libc::write(
                    peer_fd.as_raw_fd(),
                    bogus.as_ptr().cast::<libc::c_void>(),
                    bogus.len(),
                )
            },
            bogus.len() as isize
        );
        // Send another valid packet to ensure bogus packet was already processed.
        assert_eq!(
            unsafe {
                libc::write(
                    peer_fd.as_raw_fd(),
                    v6.as_ptr().cast::<libc::c_void>(),
                    v6.len(),
                )
            },
            v6.len() as isize
        );
        let n2 = device.read(&mut read_buf).await.expect("read v6");
        assert_eq!(&read_buf[..n2], v6.as_slice());

        let snap = counters.snapshot();
        assert_eq!(snap.rx_packets, 2);
        assert_eq!(snap.tx_packets, 1);
        assert_eq!(snap.dropped_packets, 1);

        tokio::time::timeout(std::time::Duration::from_secs(2), pump.stop())
            .await
            .expect("pump stop completes promptly");
    }
}
