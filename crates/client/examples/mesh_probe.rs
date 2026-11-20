//! Development-only mesh data-plane probe.
//!
//! Joins a running developer network as an ordinary member through the
//! embedded node runtime with an external packet device instead of a TUN
//! interface, then injects ICMP echo requests addressed to peer virtual IPs
//! and measures round-trip time and burst throughput.
//!
//! Usage:
//!   cargo run -p fuckipv6-client --example mesh_probe -- \
//!     --coordinator ws://127.0.0.1:6666 --network mesh

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use fuckipv6_client::config_manager::ConfigManager;
use fuckipv6_client::embedded::{
    EmbeddedNetwork, EmbeddedNetworkLease, EmbeddedNetworkRequest, EmbeddedNodeConfig,
    EmbeddedNodeRuntime, EmbeddedPlatform,
};
use fuckipv6_client::node::{NodeProblem, NodeProblemCode, NodeSnapshot};
use fuckipv6_client::packet_io::{ExternalPacketDevice, ExternalPacketEndpoint};
use fuckipv6_common::VirtualIps;
use futures::future::BoxFuture;
use tokio::sync::{mpsc, Notify};

const DEFAULT_STORAGE_ROOT: &str = ".tmp/mesh-probe";
const DEFAULT_NETWORK: &str = "mesh";
const DEFAULT_COORDINATOR: &str = "ws://127.0.0.1:6666";
const DEVICE_CAPACITY: usize = 64;

struct Options {
    storage_root: PathBuf,
    network: String,
    coordinator: String,
    count: u16,
    burst: u16,
    payload: usize,
    burst_payload: usize,
    timeout: Duration,
    warmup: u16,
    burst_rounds: u16,
    burst_batch: u16,
    burst_pause: Duration,
    peer_wait: Duration,
}

impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            storage_root: PathBuf::from(DEFAULT_STORAGE_ROOT),
            network: DEFAULT_NETWORK.to_string(),
            coordinator: DEFAULT_COORDINATOR.to_string(),
            count: 10,
            burst: 64,
            payload: 56,
            burst_payload: 1200,
            timeout: Duration::from_millis(3_000),
            warmup: 5,
            burst_rounds: 3,
            burst_batch: 16,
            burst_pause: Duration::from_micros(500),
            peer_wait: Duration::from_secs(45),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| anyhow!("{flag} requires a value"))?;
            match flag.as_str() {
                "--storage-root" => options.storage_root = PathBuf::from(value),
                "--network" => options.network = value,
                "--coordinator" => options.coordinator = value,
                "--count" => options.count = value.parse()?,
                "--burst" => options.burst = value.parse()?,
                "--payload" => options.payload = value.parse()?,
                "--burst-payload" => options.burst_payload = value.parse()?,
                "--timeout-ms" => options.timeout = Duration::from_millis(value.parse()?),
                "--warmup" => options.warmup = value.parse()?,
                "--burst-rounds" => options.burst_rounds = value.parse()?,
                "--burst-batch" => options.burst_batch = value.parse()?,
                "--burst-pause-us" => options.burst_pause = Duration::from_micros(value.parse()?),
                "--peer-wait-secs" => options.peer_wait = Duration::from_secs(value.parse()?),
                "--help" | "-h" => {
                    println!(
                        "usage: mesh_probe [--coordinator {DEFAULT_COORDINATOR}] [--network {DEFAULT_NETWORK}]"
                    );
                    std::process::exit(0);
                }
                other => return Err(anyhow!("unknown argument: {other}")),
            }
        }
        Ok(options)
    }
}

struct Session {
    network: String,
    mtu: u16,
    assigned: VirtualIps,
    endpoint: ExternalPacketEndpoint,
}

#[derive(Default)]
struct DevicePlatform {
    session: Mutex<Option<Arc<Session>>>,
    opened: Notify,
    opens: AtomicUsize,
    closes: Arc<AtomicUsize>,
}

impl DevicePlatform {
    async fn session(&self, timeout: Duration) -> Result<Arc<Session>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(session) = self.session.lock().unwrap().clone() {
                return Ok(session);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!("the embedded node never opened a network"));
            }
            let _ = tokio::time::timeout(remaining, self.opened.notified()).await;
        }
    }
}

struct DeviceLease {
    session: Arc<Session>,
    closes: Arc<AtomicUsize>,
}

impl EmbeddedNetworkLease for DeviceLease {
    fn close(&mut self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async move {
            self.session.endpoint.close();
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn cancel(&self) {
        self.session.endpoint.close();
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

impl EmbeddedPlatform for DevicePlatform {
    fn open_network(
        &self,
        request: EmbeddedNetworkRequest,
    ) -> BoxFuture<'_, Result<EmbeddedNetwork, NodeProblem>> {
        Box::pin(async move {
            let mtu = request.config.tun.mtu;
            let (device, endpoint) =
                ExternalPacketDevice::new(&request.network_name, mtu, DEVICE_CAPACITY).map_err(
                    |error| {
                        NodeProblem::new(
                            NodeProblemCode::Unavailable,
                            format!("cannot create the external packet device: {error}"),
                        )
                    },
                )?;
            let session = Arc::new(Session {
                network: request.network_name.clone(),
                mtu,
                assigned: request.assigned_ips,
                endpoint,
            });
            *self.session.lock().unwrap() = Some(session.clone());
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened.notify_waiters();
            Ok(EmbeddedNetwork::new(
                device,
                Box::new(DeviceLease {
                    session,
                    closes: self.closes.clone(),
                }),
            ))
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), NodeProblem>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Family {
    V4,
    V6,
}

struct Reply {
    family: Family,
    seq: u16,
    rtt: Duration,
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let (chunks, remainder) = data.as_chunks::<2>();
    for chunk in chunks {
        sum += u16::from_be_bytes(*chunk) as u32;
    }
    if let [last] = remainder {
        sum += (*last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn icmp_echo(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut icmp = Vec::with_capacity(8 + payload.len());
    icmp.push(8);
    icmp.push(0);
    icmp.extend_from_slice(&[0, 0]);
    icmp.extend_from_slice(&id.to_be_bytes());
    icmp.extend_from_slice(&seq.to_be_bytes());
    icmp.extend_from_slice(payload);
    let sum = checksum(&icmp);
    icmp[2..4].copy_from_slice(&sum.to_be_bytes());
    icmp
}

fn icmpv6_echo(id: u16, seq: u16, payload: &[u8], src: Ipv6Addr, dst: Ipv6Addr) -> Vec<u8> {
    let mut icmp = Vec::with_capacity(8 + payload.len());
    icmp.push(128);
    icmp.push(0);
    icmp.extend_from_slice(&[0, 0]);
    icmp.extend_from_slice(&id.to_be_bytes());
    icmp.extend_from_slice(&seq.to_be_bytes());
    icmp.extend_from_slice(payload);
    let mut pseudo = Vec::with_capacity(40 + icmp.len());
    pseudo.extend_from_slice(&src.octets());
    pseudo.extend_from_slice(&dst.octets());
    pseudo.extend_from_slice(&(icmp.len() as u32).to_be_bytes());
    pseudo.extend_from_slice(&[0, 0, 0, 58]);
    pseudo.extend_from_slice(&icmp);
    let sum = checksum(&pseudo);
    icmp[2..4].copy_from_slice(&sum.to_be_bytes());
    icmp
}

fn ipv4_packet(src: Ipv4Addr, dst: Ipv4Addr, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let icmp = icmp_echo(id, seq, payload);
    let total = 20 + icmp.len();
    let mut packet = vec![0u8; 20];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    packet[4..6].copy_from_slice(&id.to_be_bytes());
    packet[8] = 64;
    packet[9] = 1;
    packet[12..16].copy_from_slice(&src.octets());
    packet[16..20].copy_from_slice(&dst.octets());
    let header = checksum(&packet);
    packet[10..12].copy_from_slice(&header.to_be_bytes());
    packet.extend_from_slice(&icmp);
    packet
}

fn ipv6_packet(src: Ipv6Addr, dst: Ipv6Addr, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let icmp = icmpv6_echo(id, seq, payload, src, dst);
    let mut packet = vec![0u8; 40];
    packet[0] = 0x60;
    packet[4..6].copy_from_slice(&(icmp.len() as u16).to_be_bytes());
    packet[6] = 58;
    packet[7] = 64;
    packet[8..24].copy_from_slice(&src.octets());
    packet[24..40].copy_from_slice(&dst.octets());
    packet.extend_from_slice(&icmp);
    packet
}

fn parse_echo_reply(packet: &[u8]) -> Option<(Family, u16)> {
    match packet.first().map(|byte| byte >> 4)? {
        4 => {
            let header = (*packet.first()? as usize & 0x0f) * 4;
            if *packet.get(9)? != 1 {
                return None;
            }
            let icmp = packet.get(header..)?;
            if *icmp.first()? != 0 {
                return None;
            }
            Some((
                Family::V4,
                u16::from_be_bytes([*icmp.get(6)?, *icmp.get(7)?]),
            ))
        }
        6 => {
            if *packet.get(6)? != 58 {
                return None;
            }
            let icmp = packet.get(40..)?;
            if *icmp.first()? != 129 {
                return None;
            }
            Some((
                Family::V6,
                u16::from_be_bytes([*icmp.get(6)?, *icmp.get(7)?]),
            ))
        }
        _ => None,
    }
}

fn describe(problem: &NodeProblem) -> String {
    format!("{:?}: {}", problem.code, problem.message)
}

async fn total_errors(runtime: &EmbeddedNodeRuntime) -> Result<u64> {
    let snapshot = runtime
        .snapshot()
        .await
        .map_err(|problem| anyhow!(describe(&problem)))?;
    Ok(snapshot
        .networks
        .iter()
        .map(|network| network.traffic.error_count)
        .sum())
}

fn print_snapshot(snapshot: &NodeSnapshot, label: &str) {
    println!(
        "{label}: phase={:?} configured={} active={} failed={} peers={}",
        snapshot.lifecycle.phase,
        snapshot.lifecycle.configured_networks,
        snapshot.lifecycle.active_networks,
        snapshot.lifecycle.failed_networks.len(),
        snapshot.peers.len()
    );
    for network in &snapshot.networks {
        println!(
            "  network {} status={:?} ipv4={:?} ipv6={:?} peer_count={} tun={}",
            network.name,
            network.status,
            network.assigned_ipv4,
            network.assigned_ipv6,
            network.peer_count,
            network.tun_name
        );
        let traffic = &network.traffic;
        println!(
            "    traffic tun_rx={}/{}B tun_tx={}/{}B p2p_rx={}B p2p_tx={}B errors={}",
            traffic.tun_rx_packets,
            traffic.tun_rx_bytes,
            traffic.tun_tx_packets,
            traffic.tun_tx_bytes,
            traffic.p2p_rx_bytes,
            traffic.p2p_tx_bytes,
            traffic.error_count
        );
    }
    for peer in &snapshot.peers {
        println!(
            "  peer {} online={} ipv4={:?} ipv6={} p2p_active={} p2p_healthy={} rtt_us={:?} endpoints={:?}",
            peer.peer_id,
            peer.online,
            peer.virtual_ipv4,
            peer.virtual_ipv6,
            peer.p2p_active,
            peer.p2p_healthy,
            peer.rtt_us,
            peer.endpoints
        );
    }
    if let Some(problem) = &snapshot.last_error {
        println!("  last_error: {} ({:?})", problem.message, problem.code);
    }
}

async fn wait_for_peer(runtime: &EmbeddedNodeRuntime, timeout: Duration) -> Result<NodeSnapshot> {
    let deadline = Instant::now() + timeout;
    let mut last = runtime
        .snapshot()
        .await
        .map_err(|problem| anyhow!(describe(&problem)))?;
    loop {
        if last.peers.iter().any(|peer| peer.online) {
            return Ok(last);
        }
        if Instant::now() >= deadline {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        last = runtime
            .snapshot()
            .await
            .map_err(|problem| anyhow!(describe(&problem)))?;
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_echo(
    session: &Session,
    pending: &Mutex<HashMap<(Family, u16), Instant>>,
    replies: &mut mpsc::Receiver<Reply>,
    family: Family,
    peer_v4: Ipv4Addr,
    peer_v6: Ipv6Addr,
    seq: u16,
    payload: &[u8],
    timeout: Duration,
) -> Result<Option<Duration>> {
    let source_v4 = session
        .assigned
        .ipv4
        .ok_or_else(|| anyhow!("this probe has no virtual IPv4 address"))?;
    let source_v6 = session
        .assigned
        .ipv6
        .ok_or_else(|| anyhow!("this probe has no virtual IPv6 address"))?;
    let id = 0x6676;
    let packet = match family {
        Family::V4 => ipv4_packet(source_v4, peer_v4, id, seq, payload),
        Family::V6 => ipv6_packet(source_v6, peer_v6, id, seq, payload),
    };
    pending
        .lock()
        .unwrap()
        .insert((family, seq), Instant::now());
    session.endpoint.send(&packet).await?;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            pending.lock().unwrap().remove(&(family, seq));
            return Ok(None);
        }
        match tokio::time::timeout(remaining, replies.recv()).await {
            Ok(Some(reply)) if reply.family == family && reply.seq == seq => {
                return Ok(Some(reply.rtt));
            }
            Ok(Some(_)) => continue,
            Ok(None) => return Err(anyhow!("the reply channel closed")),
            Err(_) => {
                pending.lock().unwrap().remove(&(family, seq));
                return Ok(None);
            }
        }
    }
}

fn report(label: &str, target: &str, payload: usize, rtts: &[Duration], sent: u16) {
    let mut sorted = rtts.to_vec();
    sorted.sort_unstable();
    let pick = |fraction: f64| -> Duration {
        if sorted.is_empty() {
            return Duration::ZERO;
        }
        let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    };
    let min = sorted.first().copied().unwrap_or_default();
    let max = sorted.last().copied().unwrap_or_default();
    let avg = if sorted.is_empty() {
        Duration::ZERO
    } else {
        sorted.iter().sum::<Duration>() / sorted.len() as u32
    };
    println!(
        "{label} -> {target} ({payload} B payload): {}/{} replies, rtt min/p50/p90/avg/max = {:.2}/{:.2}/{:.2}/{:.2}/{:.2} ms",
        sorted.len(),
        sent,
        min.as_secs_f64() * 1e3,
        pick(0.5).as_secs_f64() * 1e3,
        pick(0.9).as_secs_f64() * 1e3,
        avg.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse()?;
    let _ = fuckipv6_common::init_logging_with_level("warn");
    std::fs::create_dir_all(&options.storage_root)
        .with_context(|| format!("cannot create {}", options.storage_root.display()))?;
    let storage_root =
        std::path::absolute(&options.storage_root).context("cannot resolve the storage root")?;

    let manager = ConfigManager::new_for_dir(storage_root.clone())
        .context("cannot open the probe config directory")?;
    match manager.create_network(&options.network, &options.coordinator, false) {
        Ok(path) => println!(
            "joined network {} through {} ({})",
            options.network,
            options.coordinator,
            path.display()
        ),
        Err(error) if error.to_string().contains("已存在") => println!(
            "network {} already configured in {}",
            options.network,
            storage_root.display()
        ),
        Err(error) => return Err(error).context("cannot configure the probe network"),
    }

    let platform = Arc::new(DevicePlatform::default());
    let mut runtime = EmbeddedNodeRuntime::start(
        EmbeddedNodeConfig {
            storage_root: storage_root.clone(),
            dns_servers: Vec::new(),
        },
        platform.clone(),
    )
    .await
    .map_err(|problem| anyhow!("cannot start the embedded node: {}", describe(&problem)))?;

    let session = platform.session(Duration::from_secs(30)).await?;
    println!(
        "packet device ready: network={} mtu={} assigned ipv4={:?} ipv6={:?}",
        session.network, session.mtu, session.assigned.ipv4, session.assigned.ipv6
    );

    let (sender, mut replies) = mpsc::channel::<Reply>(256);
    let pending: Arc<Mutex<HashMap<(Family, u16), Instant>>> = Arc::default();
    let endpoint = session.endpoint.clone();
    let receiver_pending = pending.clone();
    let receiver = tokio::spawn(async move {
        loop {
            let packet = match endpoint.recv().await {
                Ok(packet) => packet,
                Err(_) => break,
            };
            let Some((family, seq)) = parse_echo_reply(&packet) else {
                continue;
            };
            let started = receiver_pending.lock().unwrap().remove(&(family, seq));
            if let Some(started) = started {
                if sender
                    .send(Reply {
                        family,
                        seq,
                        rtt: started.elapsed(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });

    let snapshot = wait_for_peer(&runtime, options.peer_wait).await?;
    print_snapshot(&snapshot, "before probing");
    let peer = snapshot
        .peers
        .iter()
        .find(|peer| peer.online)
        .ok_or_else(|| anyhow!("no online peer in network {}", options.network))?
        .clone();
    let peer_v4 = peer
        .virtual_ipv4
        .ok_or_else(|| anyhow!("the peer advertises no virtual IPv4 address"))?;
    println!(
        "probing peer {} ({} / {})",
        peer.peer_id, peer_v4, peer.virtual_ipv6
    );

    let body = vec![0x5au8; options.payload];

    // Establish the peer path before measuring: the first packets of a fresh
    // session may travel over the relay until hole punching settles.
    let mut warmup_replies = 0usize;
    let warmup_started = Instant::now();
    for seq in 0..options.warmup {
        let tag = 0x7000u16.wrapping_add(seq);
        if send_echo(
            &session,
            &pending,
            &mut replies,
            Family::V6,
            peer_v4,
            peer.virtual_ipv6,
            tag,
            &body,
            options.timeout,
        )
        .await?
        .is_some()
        {
            warmup_replies += 1;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!(
        "warm-up icmpv6 -> {}: {}/{} replies in {:.2} s (excluded from the figures below)",
        peer.virtual_ipv6,
        warmup_replies,
        options.warmup,
        warmup_started.elapsed().as_secs_f64()
    );

    let mut v4_rtts = Vec::new();
    for seq in 0..options.count {
        if let Some(rtt) = send_echo(
            &session,
            &pending,
            &mut replies,
            Family::V4,
            peer_v4,
            peer.virtual_ipv6,
            seq,
            &body,
            options.timeout,
        )
        .await?
        {
            v4_rtts.push(rtt);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    report(
        "icmp echo",
        &peer_v4.to_string(),
        options.payload,
        &v4_rtts,
        options.count,
    );

    let mut v6_rtts = Vec::new();
    for seq in 0..options.count {
        if let Some(rtt) = send_echo(
            &session,
            &pending,
            &mut replies,
            Family::V6,
            peer_v4,
            peer.virtual_ipv6,
            seq,
            &body,
            options.timeout,
        )
        .await?
        {
            v6_rtts.push(rtt);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    report(
        "icmpv6 echo",
        &peer.virtual_ipv6.to_string(),
        options.payload,
        &v6_rtts,
        options.count,
    );

    let source_v6 = session
        .assigned
        .ipv6
        .ok_or_else(|| anyhow!("this probe has no virtual IPv6 address"))?;
    let burst_body = vec![0x5au8; options.burst_payload];
    let mut rates = Vec::new();
    for round in 0..options.burst_rounds {
        let started = Instant::now();
        let errors_before = total_errors(&runtime).await?;
        let batch = options.burst_batch.max(1);
        for seq in 0..options.burst {
            // Give every round its own sequence window so replies cannot be
            // credited to an earlier round.
            let tag = round.wrapping_mul(options.burst).wrapping_add(seq);
            let packet = ipv6_packet(source_v6, peer.virtual_ipv6, 0x6676, tag, &burst_body);
            pending
                .lock()
                .unwrap()
                .insert((Family::V6, tag), Instant::now());
            session.endpoint.send(&packet).await?;
            // Pace the burst: flooding the node UDP send queue makes every
            // packet fall back to the sealed relay path, which measures the
            // relay instead of the direct tunnel.
            if (seq + 1) % batch == 0 && !options.burst_pause.is_zero() {
                tokio::time::sleep(options.burst_pause).await;
            }
        }
        let mut replied = 0usize;
        let deadline = Instant::now() + options.timeout * 4;
        while replied < options.burst as usize {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, replies.recv()).await {
                Ok(Some(reply)) if reply.family == Family::V6 => replied += 1,
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        let errors_delta = total_errors(&runtime).await?.saturating_sub(errors_before);
        let elapsed = started.elapsed();
        let bytes = (options.burst as usize * options.burst_payload) as f64;
        let rate = bytes * 8.0 / elapsed.as_secs_f64() / 1e6;
        rates.push(rate);
        println!(
            "burst icmpv6 round {} -> {} ({} x {} B payload, {} per batch): {}/{} replies in {:.3} s, round-trip payload {:.2} Mbit/s, errors +{}",
            round + 1,
            peer.virtual_ipv6,
            options.burst,
            options.burst_payload,
            batch,
            replied,
            options.burst,
            elapsed.as_secs_f64(),
            rate,
            errors_delta
        );
    }
    if !rates.is_empty() {
        let mean = rates.iter().sum::<f64>() / rates.len() as f64;
        let best = rates.iter().copied().fold(0.0_f64, f64::max);
        println!(
            "burst icmpv6 summary: mean {:.2} Mbit/s, best {:.2} Mbit/s over {} rounds of {} x {} B",
            mean,
            best,
            rates.len(),
            options.burst,
            options.burst_payload
        );
    }

    let after = runtime
        .snapshot()
        .await
        .map_err(|problem| anyhow!(describe(&problem)))?;
    print_snapshot(&after, "after probing");

    // Close the packet device first: the receive loop holds an endpoint clone,
    // so it can only exit once the device is shut.
    session.endpoint.close();
    let _ = tokio::time::timeout(Duration::from_secs(3), receiver).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    if let Err(error) = runtime.stop().await {
        println!("node stop reported: {}", describe(&error));
    }
    println!(
        "platform sessions opened={} closed={}",
        platform.opens.load(Ordering::SeqCst),
        platform.closes.load(Ordering::SeqCst)
    );
    Ok(())
}
