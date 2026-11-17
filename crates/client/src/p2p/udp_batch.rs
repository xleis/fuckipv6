//! Linux UDP message-batch syscalls with an explicit bounded fallback surface.
//!
//! `recvmmsg`/`sendmmsg` operate on independent packet buffers. The helpers in
//! this module therefore preserve one allocation per packet while reducing the
//! number of kernel crossings for a group of datagrams owned by one peer.
//!
//! Only Linux exposes the batch syscalls. Darwin and the other non-Linux hosts
//! loop over one `send_to` per datagram: datagram boundaries, ciphertext and the
//! success-prefix contract stay identical, while the syscall counters report the
//! per-packet crossings that the kernel actually performs.

#[cfg(not(target_os = "linux"))]
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const DEFAULT_UDP_BATCH_SIZE: usize = 32;
pub(crate) const MAX_UDP_BATCH_SIZE: usize = 128;

/// Datagrams sent by the non-Linux loop before it yields to the executor.
#[cfg(not(target_os = "linux"))]
const SEND_YIELD_INTERVAL: usize = 16;

pub(crate) fn validate_udp_batch_size(size: usize) -> anyhow::Result<()> {
    if size == 0 || size > MAX_UDP_BATCH_SIZE {
        anyhow::bail!("UDP batch size must be in 1..={MAX_UDP_BATCH_SIZE}, got {size}");
    }
    Ok(())
}

/// Aggregate syscall and packet counters for the UDP batch path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpBatchMetrics {
    pub recv_syscalls: u64,
    pub send_syscalls: u64,
    pub recv_datagrams: u64,
    pub send_datagrams: u64,
    pub recv_batches: u64,
    pub send_batches: u64,
    pub max_recv_batch: u64,
    pub max_send_batch: u64,
    pub fallback_packets: u64,
}

impl UdpBatchMetrics {
    pub fn recv_packets_per_syscall(self) -> f64 {
        if self.recv_syscalls == 0 {
            0.0
        } else {
            self.recv_datagrams as f64 / self.recv_syscalls as f64
        }
    }

    pub fn send_packets_per_syscall(self) -> f64 {
        if self.send_syscalls == 0 {
            0.0
        } else {
            self.send_datagrams as f64 / self.send_syscalls as f64
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct UdpBatchStats {
    recv_syscalls: AtomicU64,
    send_syscalls: AtomicU64,
    recv_datagrams: AtomicU64,
    send_datagrams: AtomicU64,
    recv_batches: AtomicU64,
    send_batches: AtomicU64,
    max_recv_batch: AtomicU64,
    max_send_batch: AtomicU64,
    fallback_packets: AtomicU64,
}

impl UdpBatchStats {
    pub(crate) fn record_recv(&self, datagrams: usize) {
        self.recv_syscalls.fetch_add(1, Ordering::Relaxed);
        self.recv_datagrams
            .fetch_add(datagrams as u64, Ordering::Relaxed);
        if datagrams > 1 {
            self.recv_batches.fetch_add(1, Ordering::Relaxed);
        }
        update_max(&self.max_recv_batch, datagrams);
    }

    /// Records one kernel crossing that accepted `datagrams` packets.
    pub(crate) fn record_send(&self, datagrams: usize) {
        self.record_send_calls(1, datagrams);
    }

    /// Records `syscalls` kernel crossings that accepted `datagrams` packets.
    ///
    /// Linux coalesces a whole batch into one `sendmmsg`; every other platform
    /// issues one `send_to` per datagram, so the two counters must diverge to
    /// stay honest about the work the kernel performed.
    pub(crate) fn record_send_calls(&self, syscalls: usize, datagrams: usize) {
        self.send_syscalls
            .fetch_add(syscalls as u64, Ordering::Relaxed);
        self.send_datagrams
            .fetch_add(datagrams as u64, Ordering::Relaxed);
        if datagrams > 1 {
            self.send_batches.fetch_add(1, Ordering::Relaxed);
        }
        update_max(&self.max_send_batch, datagrams);
    }

    pub(crate) fn record_fallback_packet(&self) {
        self.fallback_packets.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn metrics(&self) -> UdpBatchMetrics {
        UdpBatchMetrics {
            recv_syscalls: self.recv_syscalls.load(Ordering::Relaxed),
            send_syscalls: self.send_syscalls.load(Ordering::Relaxed),
            recv_datagrams: self.recv_datagrams.load(Ordering::Relaxed),
            send_datagrams: self.send_datagrams.load(Ordering::Relaxed),
            recv_batches: self.recv_batches.load(Ordering::Relaxed),
            send_batches: self.send_batches.load(Ordering::Relaxed),
            max_recv_batch: self.max_recv_batch.load(Ordering::Relaxed),
            max_send_batch: self.max_send_batch.load(Ordering::Relaxed),
            fallback_packets: self.fallback_packets.load(Ordering::Relaxed),
        }
    }
}

fn update_max(target: &AtomicU64, candidate: usize) {
    let candidate = candidate as u64;
    let mut current = target.load(Ordering::Relaxed);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

/// Per-receiver metadata reused by Linux `recvmmsg` calls.
///
/// Packet buffers remain owned by the pool; this scratch only retains the
/// fixed-size descriptors that describe those buffers for one syscall.
#[derive(Debug, Default)]
pub(crate) struct UdpReceiveScratch {
    #[cfg(target_os = "linux")]
    iovecs: Vec<libc::iovec>,
    #[cfg(target_os = "linux")]
    addresses: Vec<libc::sockaddr_storage>,
    #[cfg(target_os = "linux")]
    messages: Vec<libc::mmsghdr>,
}

// SAFETY: the receive loop owns this scratch exclusively. Raw pointers in the
// metadata are rebound to caller-owned payload buffers immediately before each
// synchronous syscall and are never dereferenced by safe code between calls.
// Moving the descriptor vectors to another executor thread therefore does not
// move or invalidate the packet allocations they describe.
#[cfg(target_os = "linux")]
unsafe impl Send for UdpReceiveScratch {}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn receive_batch(
    socket: &tokio::net::UdpSocket,
    payloads: &mut [super::buffer_pool::InboundPayload],
    _scratch: &mut UdpReceiveScratch,
) -> io::Result<Vec<(usize, std::net::SocketAddr)>> {
    let Some(payload) = payloads.first_mut() else {
        return Ok(Vec::new());
    };
    let (length, address) = socket.recv_from(payload.as_mut_slice()).await?;
    Ok(vec![(length, address)])
}

/// Number of kernel crossings a `send_batch` of `datagrams` packets performs.
pub(crate) fn send_syscalls(datagrams: usize) -> usize {
    if cfg!(target_os = "linux") {
        usize::from(datagrams > 0)
    } else {
        datagrams
    }
}

/// Records the datagrams a `send_batch` call actually handed to the kernel.
pub(crate) fn record_send_result(stats: &UdpBatchStats, datagrams: usize) {
    stats.record_send_calls(send_syscalls(datagrams), datagrams);
}

/// Send every datagram in `packets` to `endpoint`, in order, on hosts without
/// `sendmmsg`.
///
/// The contract matches the Linux batch call: the return value is the number of
/// datagrams the kernel accepted, so a short return means only the unsent suffix
/// may be retried on another transport. A datagram is never sent twice.
///
/// Long bursts yield to the executor every [`SEND_YIELD_INTERVAL`] datagrams so
/// a 128-packet batch cannot monopolise the reactor thread.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn send_batch(
    socket: &tokio::net::UdpSocket,
    endpoint: std::net::SocketAddr,
    packets: &[&[u8]],
) -> io::Result<usize> {
    let mut sent = 0usize;
    for packet in packets {
        if sent > 0 && sent % SEND_YIELD_INTERVAL == 0 {
            tokio::task::yield_now().await;
        }
        loop {
            match socket.send_to(packet, endpoint).await {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // A failure after a successful prefix keeps the already queued
                // datagrams on the direct path and lets only the suffix fall
                // back. The caller reuses the identical ciphertext, so no
                // nonce is ever sent twice.
                Err(error) if sent > 0 => return Ok(sent),
                Err(error) => return Err(error),
            }
        }
        sent += 1;
    }
    Ok(sent)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::super::buffer_pool::InboundPayload;
    use super::UdpReceiveScratch;
    use std::io::{self, ErrorKind};
    use std::mem::{size_of, zeroed};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
    use std::os::fd::AsRawFd;
    use tokio::io::Interest;
    use tokio::net::UdpSocket;

    /// Receive up to `payloads.len()` datagrams with one `recvmmsg` call.
    ///
    /// If the batch syscall itself is unavailable or denied, this falls back to
    /// one ordinary `recv_from` call. Other errors are returned unchanged so a
    /// permanently broken socket cannot turn into a retry loop.
    pub(crate) async fn receive_batch(
        socket: &UdpSocket,
        payloads: &mut [InboundPayload],
        scratch: &mut UdpReceiveScratch,
    ) -> io::Result<Vec<(usize, SocketAddr)>> {
        receive_batch_with(socket, payloads, scratch, recvmmsg).await
    }

    pub(super) async fn receive_batch_with(
        socket: &UdpSocket,
        payloads: &mut [InboundPayload],
        scratch: &mut UdpReceiveScratch,
        mut batch_receive: impl FnMut(
            std::os::fd::RawFd,
            &mut [InboundPayload],
            &mut UdpReceiveScratch,
        ) -> io::Result<Vec<(usize, SocketAddr)>>,
    ) -> io::Result<Vec<(usize, SocketAddr)>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }

        let fd = socket.as_raw_fd();
        match socket
            .async_io(Interest::READABLE, || batch_receive(fd, payloads, scratch))
            .await
        {
            Ok(datagrams) => Ok(datagrams),
            Err(error) if should_fallback_to_single_recv(&error) => {
                let payload = payloads
                    .first_mut()
                    .expect("non-empty payloads checked before batch receive");
                let (length, address) = socket.recv_from(payload.as_mut_slice()).await?;
                Ok(vec![(length, address)])
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn should_fallback_to_single_recv(error: &io::Error) -> bool {
        error.kind() == ErrorKind::Unsupported
            || matches!(
                error.raw_os_error(),
                Some(code)
                    if code == libc::ENOSYS
                        || code == libc::EPERM
                        || code == libc::EOPNOTSUPP
            )
    }

    /// Send up to `packets.len()` datagrams to one endpoint with one syscall.
    ///
    /// A short return is valid: Linux reports how many messages were accepted,
    /// and the caller must fall back only the unsent suffix.
    pub(crate) async fn send_batch(
        socket: &UdpSocket,
        endpoint: SocketAddr,
        packets: &[&[u8]],
    ) -> io::Result<usize> {
        if packets.is_empty() {
            return Ok(0);
        }
        socket
            .async_io(Interest::WRITABLE, || {
                sendmmsg(socket.as_raw_fd(), endpoint, packets)
            })
            .await
    }

    fn recvmmsg(
        fd: std::os::fd::RawFd,
        payloads: &mut [InboundPayload],
        scratch: &mut UdpReceiveScratch,
    ) -> io::Result<Vec<(usize, SocketAddr)>> {
        let count = payloads.len();
        scratch.iovecs.clear();
        scratch.addresses.clear();
        scratch
            .iovecs
            .reserve(count.saturating_sub(scratch.iovecs.capacity()));
        scratch
            .addresses
            .reserve(count.saturating_sub(scratch.addresses.capacity()));

        for payload in payloads.iter_mut() {
            scratch.iovecs.push(libc::iovec {
                iov_base: payload.as_mut_slice().as_mut_ptr().cast(),
                iov_len: payload.len(),
            });
            scratch.addresses.push(unsafe { zeroed() });
        }
        scratch.messages.resize_with(count, || libc::mmsghdr {
            msg_hdr: unsafe { zeroed() },
            msg_len: 0,
        });
        for index in 0..count {
            let address = &mut scratch.addresses[index];
            let iovec = &mut scratch.iovecs[index];
            let message = &mut scratch.messages[index];
            message.msg_hdr.msg_name = (address as *mut libc::sockaddr_storage).cast();
            message.msg_hdr.msg_namelen = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            message.msg_hdr.msg_iov = iovec;
            message.msg_hdr.msg_iovlen = 1;
            message.msg_hdr.msg_control = std::ptr::null_mut();
            message.msg_hdr.msg_controllen = 0;
            message.msg_hdr.msg_flags = 0;
            message.msg_len = 0;
        }

        // SAFETY: `scratch.messages`, every iovec, and every sockaddr_storage
        // remain valid for the duration of the syscall. The kernel writes at
        // most the supplied iovec length and address capacity.
        let received = loop {
            let received = unsafe {
                libc::recvmmsg(
                    fd,
                    scratch.messages.as_mut_ptr(),
                    scratch.messages.len() as libc::c_uint,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if received >= 0 {
                break received;
            }
            let error = io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(error);
            }
        };

        let received = received as usize;
        let mut datagrams = Vec::with_capacity(received);
        for (index, message) in scratch.messages.iter().take(received).enumerate() {
            let Some(address) =
                sockaddr_to_std(&scratch.addresses[index], message.msg_hdr.msg_namelen)
            else {
                // A datagram with an unsupported address family cannot be
                // safely attributed to a peer. Stop accepting this and the
                // remaining suffix; the caller recycles all supplied buffers.
                break;
            };
            datagrams.push((message.msg_len as usize, address));
        }
        Ok(datagrams)
    }

    fn sendmmsg(
        fd: std::os::fd::RawFd,
        endpoint: SocketAddr,
        packets: &[&[u8]],
    ) -> io::Result<usize> {
        debug_assert!(packets.len() <= super::MAX_UDP_BATCH_SIZE);
        let target = socket2::SockAddr::from(endpoint);
        let mut iovecs: [libc::iovec; super::MAX_UDP_BATCH_SIZE] =
            std::array::from_fn(|_| libc::iovec {
                iov_base: std::ptr::null_mut(),
                iov_len: 0,
            });
        let mut messages: [libc::mmsghdr; super::MAX_UDP_BATCH_SIZE] = unsafe { zeroed() };
        for (index, packet) in packets.iter().enumerate() {
            iovecs[index] = libc::iovec {
                iov_base: packet.as_ptr().cast_mut().cast(),
                iov_len: packet.len(),
            };
            let message = &mut messages[index];
            message.msg_hdr.msg_name = target.as_ptr().cast_mut().cast();
            message.msg_hdr.msg_namelen = target.len();
            message.msg_hdr.msg_iov = &mut iovecs[index];
            message.msg_hdr.msg_iovlen = 1;
            message.msg_hdr.msg_control = std::ptr::null_mut();
            message.msg_hdr.msg_controllen = 0;
            message.msg_hdr.msg_flags = 0;
            message.msg_len = 0;
        }

        retry_interrupted(|| {
            // SAFETY: all message descriptors and packet buffers are live for
            // the syscall and each iovec advertises only its initialized bytes.
            let sent = unsafe {
                libc::sendmmsg(fd, messages.as_mut_ptr(), packets.len() as libc::c_uint, 0)
            };
            if sent < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(sent as usize)
            }
        })
    }

    pub(super) fn retry_interrupted(
        mut operation: impl FnMut() -> io::Result<usize>,
    ) -> io::Result<usize> {
        loop {
            match operation() {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }

    fn sockaddr_to_std(
        address: &libc::sockaddr_storage,
        length: libc::socklen_t,
    ) -> Option<SocketAddr> {
        if (length as usize) < size_of::<libc::sa_family_t>() {
            return None;
        }
        match address.ss_family as libc::c_int {
            libc::AF_INET if length as usize >= size_of::<libc::sockaddr_in>() => {
                // SAFETY: the family and kernel-provided length establish a
                // fully initialized sockaddr_in.
                let address = unsafe { &*(address as *const _ as *const libc::sockaddr_in) };
                Some(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)),
                    u16::from_be(address.sin_port),
                )))
            }
            libc::AF_INET6 if length as usize >= size_of::<libc::sockaddr_in6>() => {
                // SAFETY: the family and kernel-provided length establish a
                // fully initialized sockaddr_in6.
                let address = unsafe { &*(address as *const _ as *const libc::sockaddr_in6) };
                Some(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(address.sin6_addr.s6_addr),
                    u16::from_be(address.sin6_port),
                    address.sin6_flowinfo,
                    address.sin6_scope_id,
                )))
            }
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{receive_batch, send_batch};

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::p2p::buffer_pool::UdpBufferPool;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn recvmmsg_and_sendmmsg_batch_independent_payloads() -> anyhow::Result<()> {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let receiver_addr = receiver.local_addr()?;

        let sent_payloads: Vec<Vec<u8>> =
            (0..8).map(|index| vec![index as u8; 64 + index]).collect();
        let sent_refs: Vec<&[u8]> = sent_payloads.iter().map(Vec::as_slice).collect();
        let sent = send_batch(&sender, receiver_addr, &sent_refs).await?;
        assert_eq!(sent, sent_payloads.len());

        let pool = UdpBufferPool::new(16);
        let mut buffers: Vec<_> = (0..8).map(|_| pool.acquire()).collect();
        let mut scratch = UdpReceiveScratch::default();
        let received = receive_batch(&receiver, &mut buffers, &mut scratch).await?;
        assert_eq!(received.len(), sent_payloads.len());
        for (index, ((length, source), expected)) in received.iter().zip(&sent_payloads).enumerate()
        {
            assert_eq!(*source, sender.local_addr()?);
            assert_eq!(*length, expected.len());
            assert_eq!(&buffers[index][..*length], expected.as_slice());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn recvmmsg_unsupported_falls_back_to_real_single_datagram() -> anyhow::Result<()> {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let receiver_addr = receiver.local_addr()?;
        let sender_addr = sender.local_addr()?;
        let expected = b"single-packet-fallback";
        sender.send_to(expected, receiver_addr).await?;

        let pool = UdpBufferPool::new(2);
        let mut buffers: Vec<_> = (0..2).map(|_| pool.acquire()).collect();
        let mut batch_calls = 0;
        let received = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            linux::receive_batch_with(
                &receiver,
                &mut buffers,
                &mut UdpReceiveScratch::default(),
                |_fd, _payloads, _scratch| {
                    batch_calls += 1;
                    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
                },
            ),
        )
        .await??;

        assert_eq!(batch_calls, 1);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0], (expected.len(), sender_addr));
        assert_eq!(&buffers[0][..expected.len()], expected);
        assert_eq!(
            buffers[0].len(),
            super::super::buffer_pool::UDP_RECEIVE_BUFFER_SIZE
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sendmmsg_retries_eintr_once_then_returns_success() {
        let mut calls = 0;
        let sent = linux::retry_interrupted(|| {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from_raw_os_error(libc::EINTR))
            } else {
                Ok(3)
            }
        })
        .expect("interrupted send should be retried");
        assert_eq!(sent, 3);
        assert_eq!(calls, 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sendmmsg_short_prefix_is_not_retried() {
        let mut calls = 0;
        let sent = linux::retry_interrupted(|| {
            calls += 1;
            Ok(2)
        })
        .expect("short send prefix is a successful syscall");
        assert_eq!(sent, 2);
        assert_eq!(calls, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recvmmsg_fallback_error_contract_is_narrow() {
        use std::io::Error;

        for code in [libc::ENOSYS, libc::EPERM, libc::EOPNOTSUPP] {
            assert!(linux::should_fallback_to_single_recv(
                &Error::from_raw_os_error(code)
            ));
        }
        for code in [libc::EBADF, libc::EINVAL, libc::ECONNRESET] {
            assert!(!linux::should_fallback_to_single_recv(
                &Error::from_raw_os_error(code)
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn empty_receive_batch_skips_recvmmsg_syscall() -> anyhow::Result<()> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let mut payloads = [];
        let mut scratch = UdpReceiveScratch::default();
        let mut batch_calls = 0;
        let received = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            linux::receive_batch_with(
                &socket,
                &mut payloads,
                &mut scratch,
                |_fd, _payloads, _scratch| {
                    batch_calls += 1;
                    panic!("empty receive batch must not issue recvmmsg");
                },
            ),
        )
        .await??;

        assert!(received.is_empty());
        assert_eq!(batch_calls, 0);
        Ok(())
    }

    #[tokio::test]
    async fn empty_send_batch_returns_zero_without_kernel_crossing() -> anyhow::Result<()> {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let endpoint = socket.local_addr()?;
        let sent = send_batch(&socket, endpoint, &[]).await?;
        assert_eq!(sent, 0);
        Ok(())
    }

    #[test]
    fn short_prefix_counts_one_syscall_for_only_sent_datagrams() {
        let stats = UdpBatchStats::default();
        stats.record_send(2);
        stats.record_fallback_packet();
        stats.record_fallback_packet();
        let metrics = stats.metrics();
        assert_eq!(metrics.send_syscalls, 1);
        assert_eq!(metrics.send_datagrams, 2);
        assert_eq!(metrics.send_batches, 1);
        assert_eq!(metrics.max_send_batch, 2);
        assert_eq!(metrics.fallback_packets, 2);
    }

    #[test]
    fn batch_size_is_bounded() {
        assert!(validate_udp_batch_size(1).is_ok());
        assert!(validate_udp_batch_size(MAX_UDP_BATCH_SIZE).is_ok());
        assert!(validate_udp_batch_size(0).is_err());
        assert!(validate_udp_batch_size(MAX_UDP_BATCH_SIZE + 1).is_err());
    }

    #[test]
    fn metrics_report_packets_per_syscall() {
        let stats = UdpBatchStats::default();
        stats.record_recv(4);
        stats.record_recv(2);
        stats.record_send(8);
        let metrics = stats.metrics();
        assert_eq!(metrics.recv_syscalls, 2);
        assert_eq!(metrics.recv_datagrams, 6);
        assert_eq!(metrics.recv_batches, 2);
        assert_eq!(metrics.max_recv_batch, 4);
        assert_eq!(metrics.recv_packets_per_syscall(), 3.0);
        assert_eq!(metrics.send_packets_per_syscall(), 8.0);
    }

    async fn recv_datagrams(
        socket: &tokio::net::UdpSocket,
        count: usize,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut received = Vec::with_capacity(count);
        let mut buffer = vec![0u8; 2048];
        for _ in 0..count {
            let (length, _) = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                socket.recv_from(&mut buffer),
            )
            .await??;
            received.push(buffer[..length].to_vec());
        }
        Ok(received)
    }

    #[tokio::test]
    async fn send_batch_delivers_every_datagram_in_order() -> anyhow::Result<()> {
        // 1, 8, 16, 32 and the 128-datagram ceiling of one batch.
        for size in [1usize, 8, 16, 32, 128] {
            let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
            let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
            let receiver_addr = receiver.local_addr()?;
            let payloads: Vec<Vec<u8>> = (0..size)
                .map(|index| vec![index as u8; 32 + index])
                .collect();
            let refs: Vec<&[u8]> = payloads.iter().map(Vec::as_slice).collect();

            let sent = send_batch(&sender, receiver_addr, &refs).await?;
            assert_eq!(
                sent, size,
                "every datagram of a {size}-packet batch must reach the kernel"
            );

            let received = recv_datagrams(&receiver, size).await?;
            assert_eq!(received.len(), size);
            for (index, (actual, expected)) in received.iter().zip(&payloads).enumerate() {
                assert_eq!(
                    actual, expected,
                    "datagram {index} of a {size}-packet batch lost its boundary or order"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn send_batch_delivers_ipv6_datagrams_without_merging_them() -> anyhow::Result<()> {
        let receiver = tokio::net::UdpSocket::bind("[::1]:0").await?;
        let sender = tokio::net::UdpSocket::bind("[::1]:0").await?;
        let receiver_addr = receiver.local_addr()?;
        let payloads: Vec<Vec<u8>> = (0..8u8).map(|index| vec![index; 64]).collect();
        let refs: Vec<&[u8]> = payloads.iter().map(Vec::as_slice).collect();

        let sent = send_batch(&sender, receiver_addr, &refs).await?;
        assert_eq!(sent, payloads.len());

        let received = recv_datagrams(&receiver, payloads.len()).await?;
        for (actual, expected) in received.iter().zip(&payloads) {
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[tokio::test]
    async fn send_batch_keeps_a_successful_prefix_when_a_later_datagram_fails() -> anyhow::Result<()>
    {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let receiver_addr = receiver.local_addr()?;
        let first = vec![7u8; 64];
        // Larger than any UDP payload, so the kernel rejects this datagram after
        // the first one has already been queued.
        let oversized = vec![0u8; 70_000];
        let last = vec![9u8; 64];

        let sent = send_batch(&sender, receiver_addr, &[&first, &oversized, &last]).await?;
        assert_eq!(
            sent, 1,
            "only the datagrams the kernel accepted may be reported as sent"
        );

        let received = recv_datagrams(&receiver, 1).await?;
        assert_eq!(received, vec![first]);

        let mut buffer = vec![0u8; 2048];
        let stray = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            receiver.recv_from(&mut buffer),
        )
        .await;
        assert!(
            stray.is_err(),
            "a datagram the kernel rejected must never be retried or duplicated"
        );
        Ok(())
    }

    #[tokio::test]
    async fn send_batch_surfaces_the_error_when_the_first_datagram_fails() -> anyhow::Result<()> {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let oversized = vec![0u8; 70_000];

        let error = send_batch(&sender, receiver.local_addr()?, &[&oversized])
            .await
            .expect_err("an unsendable first datagram must be reported");
        assert_ne!(error.kind(), std::io::ErrorKind::WouldBlock);
        Ok(())
    }

    #[test]
    fn send_syscalls_track_kernel_coalescing() {
        assert_eq!(send_syscalls(0), 0);
        let expected = if cfg!(target_os = "linux") { 1 } else { 8 };
        assert_eq!(send_syscalls(8), expected);

        let stats = UdpBatchStats::default();
        record_send_result(&stats, 8);
        let metrics = stats.metrics();
        assert_eq!(metrics.send_syscalls, expected as u64);
        assert_eq!(metrics.send_datagrams, 8);
        assert_eq!(metrics.send_batches, 1);
        assert_eq!(metrics.max_send_batch, 8);
    }
}
