use anyhow::{Context, Result};
use fuckipv6_common::{NatDetectionConfig, NatType};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::{lookup_host, UdpSocket};
use tokio::time::{timeout, Duration};

const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_SUCCESS: u16 = 0x0101;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const STUN_ATTR_CHANGE_REQUEST: u16 = 0x0003;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatDetectionReport {
    pub nat_type: NatType,
    pub mapped_addresses: Vec<SocketAddr>,
    pub filtering_allows_changed_addr: bool,
}

pub async fn detect_nat_type(
    config: &NatDetectionConfig,
    local_addr: SocketAddr,
) -> Result<NatType> {
    if !config.enabled {
        return Ok(NatType::Unknown);
    }

    let report = detect_nat_type_report(config, local_addr).await?;
    Ok(report.nat_type)
}

/// Probe NAT behavior on a temporary socket bound to the same local address.
///
/// This preserves the original API for callers that only know the engine's
/// local address. New callers should use [`detect_nat_type_report_on_socket`]
/// so STUN observes the exact socket advertised by the P2P transport.
pub async fn detect_nat_type_report(
    config: &NatDetectionConfig,
    local_addr: SocketAddr,
) -> Result<NatDetectionReport> {
    if !config.enabled {
        return Ok(NatDetectionReport {
            nat_type: NatType::Unknown,
            mapped_addresses: Vec::new(),
            filtering_allows_changed_addr: false,
        });
    }

    let socket = bind_detection_socket(local_addr).await?;
    detect_nat_type_report_on_socket(config, &socket).await
}

/// Probe NAT behavior using the supplied transport socket.
pub async fn detect_nat_type_report_on_socket(
    config: &NatDetectionConfig,
    socket: &UdpSocket,
) -> Result<NatDetectionReport> {
    if !config.enabled {
        return Ok(NatDetectionReport {
            nat_type: NatType::Unknown,
            mapped_addresses: Vec::new(),
            filtering_allows_changed_addr: false,
        });
    }

    let servers = resolve_stun_servers(&config.stun_servers).await?;
    let timeout_duration = Duration::from_millis(config.timeout_ms);
    let local_probe_addr = socket
        .local_addr()
        .context("无法读取 NAT 检测 UDP socket 地址")?;

    let mut mapped_addresses = Vec::new();
    let mut filtering_allows_changed_addr = false;

    for server in servers {
        if let Ok(response) = stun_binding_request(socket, server, false, timeout_duration).await {
            if !mapped_addresses.contains(&response.mapped_addr) {
                mapped_addresses.push(response.mapped_addr);
            }
        }

        if !filtering_allows_changed_addr {
            filtering_allows_changed_addr =
                stun_binding_request(socket, server, true, timeout_duration)
                    .await
                    .is_ok();
        }
    }

    let nat_type = classify_nat_type(
        local_probe_addr,
        &mapped_addresses,
        filtering_allows_changed_addr,
    );

    Ok(NatDetectionReport {
        nat_type,
        mapped_addresses,
        filtering_allows_changed_addr,
    })
}

async fn bind_detection_socket(local_addr: SocketAddr) -> Result<UdpSocket> {
    let (domain, bind_addr) = match local_addr {
        SocketAddr::V4(addr) => (
            socket2::Domain::IPV4,
            SocketAddr::new(IpAddr::V4(*addr.ip()), 0),
        ),
        SocketAddr::V6(addr) => (
            socket2::Domain::IPV6,
            SocketAddr::new(IpAddr::V6(*addr.ip()), 0),
        ),
    };

    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))
        .context("无法创建 NAT 检测 UDP socket")?;
    if local_addr.is_ipv6() {
        socket
            .set_only_v6(false)
            .context("无法启用 NAT 检测 UDP 双栈模式")?;
    }
    socket
        .set_nonblocking(true)
        .context("无法设置 NAT 检测 UDP socket 为非阻塞")?;
    socket
        .bind(&bind_addr.into())
        .context("无法绑定 NAT 检测 UDP socket")?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket).context("无法创建 Tokio NAT 检测 UDP socket")
}

fn classify_nat_type(
    local_addr: SocketAddr,
    mapped_addresses: &[SocketAddr],
    filtering_allows_changed_addr: bool,
) -> NatType {
    if mapped_addresses.is_empty() {
        return NatType::Unknown;
    }

    if mapped_addresses
        .iter()
        .all(|mapped_addr| same_ip_and_port(*mapped_addr, local_addr))
    {
        return NatType::Open;
    }

    if mapped_addresses
        .windows(2)
        .any(|window| window[0] != window[1])
    {
        return NatType::Symmetric;
    }

    if filtering_allows_changed_addr {
        NatType::FullCone
    } else if mapped_addresses.len() >= 2 {
        NatType::Restricted
    } else {
        NatType::PortRestricted
    }
}

fn same_ip_and_port(left: SocketAddr, right: SocketAddr) -> bool {
    left.port() == right.port() && ip_equal(left.ip(), right.ip())
}

fn ip_equal(left: IpAddr, right: IpAddr) -> bool {
    match (left, right) {
        (IpAddr::V4(left), IpAddr::V4(right)) => left == right,
        (IpAddr::V6(left), IpAddr::V6(right)) => left == right,
        (IpAddr::V4(left), IpAddr::V6(right)) => right.to_ipv4_mapped() == Some(left),
        (IpAddr::V6(left), IpAddr::V4(right)) => left.to_ipv4_mapped() == Some(right),
    }
}

async fn resolve_stun_servers(servers: &[String]) -> Result<Vec<SocketAddr>> {
    let mut addresses = Vec::new();

    for server in servers {
        let resolved = lookup_host(server)
            .await
            .with_context(|| format!("无法解析 STUN 服务器 `{server}`"))?;
        for address in resolved {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }

    if addresses.is_empty() {
        anyhow::bail!("没有可用的 STUN 服务器地址");
    }

    Ok(addresses)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StunResponse {
    mapped_addr: SocketAddr,
}

async fn stun_binding_request(
    socket: &UdpSocket,
    server: SocketAddr,
    change_ip_and_port: bool,
    timeout_duration: Duration,
) -> Result<StunResponse> {
    let request = build_stun_binding_request(change_ip_and_port)?;
    let transaction_id = request.transaction_id;
    socket
        .send_to(&request.bytes, server)
        .await
        .with_context(|| format!("无法发送 STUN 请求到 {server}"))?;

    let mut buf = [0u8; 1500];
    loop {
        let (size, _) = timeout(timeout_duration, socket.recv_from(&mut buf))
            .await
            .with_context(|| format!("等待 STUN 响应超时: {server}"))?
            .context("读取 STUN 响应失败")?;

        if let Some(response) = parse_stun_binding_response(&buf[..size], transaction_id)? {
            return Ok(response);
        }
    }
}

struct StunRequest {
    bytes: Vec<u8>,
    transaction_id: [u8; 12],
}

fn build_stun_binding_request(change_ip_and_port: bool) -> Result<StunRequest> {
    let transaction_id = random_transaction_id()?;
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    bytes.extend_from_slice(&transaction_id);

    if change_ip_and_port {
        bytes.extend_from_slice(&STUN_ATTR_CHANGE_REQUEST.to_be_bytes());
        bytes.extend_from_slice(&4u16.to_be_bytes());
        bytes.extend_from_slice(&0x0000_0006u32.to_be_bytes());
        let length = (bytes.len() - 20) as u16;
        bytes[2..4].copy_from_slice(&length.to_be_bytes());
    }

    Ok(StunRequest {
        bytes,
        transaction_id,
    })
}

fn random_transaction_id() -> Result<[u8; 12]> {
    let mut transaction_id = [0u8; 12];
    getrandom::fill(&mut transaction_id).context("无法生成 STUN transaction id")?;
    Ok(transaction_id)
}

fn parse_stun_binding_response(
    data: &[u8],
    transaction_id: [u8; 12],
) -> Result<Option<StunResponse>> {
    if data.len() < 20 {
        return Ok(None);
    }

    let message_type = u16::from_be_bytes([data[0], data[1]]);
    if message_type != STUN_BINDING_SUCCESS {
        return Ok(None);
    }

    let message_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data.len() < 20 + message_len {
        return Ok(None);
    }

    let magic_cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if magic_cookie != STUN_MAGIC_COOKIE || data[8..20] != transaction_id {
        return Ok(None);
    }

    let attrs = &data[20..20 + message_len];
    let mut offset = 0usize;
    let mut mapped_addr = None;

    while offset + 4 <= attrs.len() {
        let attr_type = u16::from_be_bytes([attrs[offset], attrs[offset + 1]]);
        let attr_len = u16::from_be_bytes([attrs[offset + 2], attrs[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > attrs.len() {
            break;
        }

        let value = &attrs[offset..offset + attr_len];
        match attr_type {
            STUN_ATTR_XOR_MAPPED_ADDRESS => {
                mapped_addr = parse_stun_address(value, true, transaction_id)?;
            }
            STUN_ATTR_MAPPED_ADDRESS if mapped_addr.is_none() => {
                mapped_addr = parse_stun_address(value, false, transaction_id)?;
            }
            _ => {}
        }

        offset += padded_attr_len(attr_len);
    }

    Ok(mapped_addr.map(|mapped_addr| StunResponse { mapped_addr }))
}

fn parse_stun_address(
    value: &[u8],
    xor: bool,
    transaction_id: [u8; 12],
) -> Result<Option<SocketAddr>> {
    if value.len() < 8 {
        return Ok(None);
    }

    let family = value[1];
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    if xor {
        port ^= (STUN_MAGIC_COOKIE >> 16) as u16;
    }

    let addr = match family {
        0x01 => {
            if value.len() < 8 {
                return Ok(None);
            }

            let mut octets = [value[4], value[5], value[6], value[7]];
            if xor {
                let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
                for (octet, cookie_octet) in octets.iter_mut().zip(cookie) {
                    *octet ^= cookie_octet;
                }
            }
            IpAddr::V4(Ipv4Addr::from(octets))
        }
        0x02 => {
            if value.len() < 20 {
                return Ok(None);
            }

            let mut octets = [0u8; 16];
            octets.copy_from_slice(&value[4..20]);
            if xor {
                let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
                for (octet, cookie_octet) in octets[..4].iter_mut().zip(cookie) {
                    *octet ^= cookie_octet;
                }
                for (octet, transaction_octet) in octets[4..].iter_mut().zip(transaction_id) {
                    *octet ^= transaction_octet;
                }
            }
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return Ok(None),
    };

    Ok(Some(SocketAddr::new(addr, port)))
}

fn padded_attr_len(len: usize) -> usize {
    (len + 3) & !3
}

/// NAT-PMP (RFC 6886) UDP 端口映射请求构造
pub fn build_nat_pmp_request(
    internal_port: u16,
    external_port: u16,
    lifetime_secs: u32,
) -> [u8; 12] {
    let mut req = [0u8; 12];
    req[0] = 0; // Version 0
    req[1] = 2; // Opcode 2 (UDP mapping)
                // req[2..4] = reserved
    req[4..6].copy_from_slice(&internal_port.to_be_bytes());
    req[6..8].copy_from_slice(&external_port.to_be_bytes());
    req[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());
    req
}

/// 解析 NAT-PMP 响应
pub fn parse_nat_pmp_response(buf: &[u8]) -> Option<(u16, u16, u32)> {
    if buf.len() < 16 {
        return None;
    }
    if buf[0] != 0 || buf[1] != 130 {
        // version 0, opcode 128 + 2
        return None;
    }
    let result_code = u16::from_be_bytes([buf[2], buf[3]]);
    if result_code != 0 {
        return None; // 非 0 表示网关拒绝或不支持
    }
    let internal_port = u16::from_be_bytes([buf[8], buf[9]]);
    let mapped_port = u16::from_be_bytes([buf[10], buf[11]]);
    let lifetime = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
    Some((internal_port, mapped_port, lifetime))
}

/// 构造 UPnP IGD AddPortMapping SOAP 报文
pub fn build_upnp_soap_body(
    service_type: &str,
    local_ip: &str,
    port: u16,
    lease_secs: u32,
) -> String {
    format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:AddPortMapping xmlns:u="{service_type}">
      <NewRemoteHost></NewRemoteHost>
      <NewExternalPort>{port}</NewExternalPort>
      <NewProtocol>UDP</NewProtocol>
      <NewInternalPort>{port}</NewInternalPort>
      <NewInternalClient>{local_ip}</NewInternalClient>
      <NewEnabled>1</NewEnabled>
      <NewPortMappingDescription>fuckipv6</NewPortMappingDescription>
      <NewLeaseDuration>{lease_secs}</NewLeaseDuration>
    </u:AddPortMapping>
  </s:Body>
</s:Envelope>"#
    )
}

/// 尝试通过 NAT-PMP 向本地默认网关申请 UDP 端口映射
pub async fn try_nat_pmp_mapping(gateway_ip: Ipv4Addr, port: u16) -> Option<u16> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    let req = build_nat_pmp_request(port, port, 3600);
    let gateway_addr = SocketAddr::new(IpAddr::V4(gateway_ip), 5351);

    if socket.send_to(&req, gateway_addr).await.is_err() {
        return None;
    }

    let mut buf = [0u8; 32];
    let res = tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await;
    if let Ok(Ok((len, _))) = res {
        if let Some((_, mapped_port, _)) = parse_nat_pmp_response(&buf[..len]) {
            if mapped_port > 0 {
                tracing::info!(
                    "NAT-PMP 成功映射端口: 本地 {} -> 网关外部 {}",
                    port,
                    mapped_port
                );
                return Some(mapped_port);
            }
        }
    }
    None
}

/// 收集本节点的候选端点（包含本地私网 IP、NAT-PMP 映射端口、STUN 探测到的公网映射 IP）
pub async fn gather_candidate_endpoints(
    p2p_port: u16,
    mapped_addresses: &[SocketAddr],
) -> Vec<SocketAddr> {
    let mut candidates = Vec::new();

    // 1. 尝试探测本地 IPv4 出站接口地址并请求网关端口映射
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                let ip = local_addr.ip();
                if !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !candidates.contains(&SocketAddr::new(ip, p2p_port))
                {
                    candidates.push(SocketAddr::new(ip, p2p_port));
                }

                if let IpAddr::V4(v4) = ip {
                    let oct = v4.octets();
                    let common_gateways = [
                        Ipv4Addr::new(oct[0], oct[1], oct[2], 1),
                        Ipv4Addr::new(192, 168, 1, 1),
                        Ipv4Addr::new(192, 168, 0, 1),
                    ];
                    for gw in common_gateways {
                        if let Some(mapped_port) = try_nat_pmp_mapping(gw, p2p_port).await {
                            // 如果 STUN 映射地址已存在，借用 STUN 公网 IP 组合映射端口
                            for stun_addr in mapped_addresses {
                                if let SocketAddr::V4(s_v4) = stun_addr {
                                    let ep = SocketAddr::new(IpAddr::V4(*s_v4.ip()), mapped_port);
                                    if !candidates.contains(&ep) {
                                        candidates.push(ep);
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    // 2. 尝试探测本地 IPv6 出站接口地址
    if let Ok(socket) = std::net::UdpSocket::bind("[::]:0") {
        if socket.connect("[2001:4860:4860::8888]:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                let ip = local_addr.ip();
                if !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !candidates.contains(&SocketAddr::new(ip, p2p_port))
                {
                    candidates.push(SocketAddr::new(ip, p2p_port));
                }
            }
        }
    }

    // 3. 加入 STUN 检测到的公网映射地址
    for mapped in mapped_addresses {
        if !mapped.ip().is_loopback()
            && !mapped.ip().is_unspecified()
            && !candidates.contains(mapped)
        {
            candidates.push(*mapped);
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::connection::P2pEngine;

    #[tokio::test]
    async fn nat_probe_and_engine_reuse_the_same_transport_port() -> Result<()> {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let server_addr = server.local_addr()?;
        let server_task = tokio::spawn(async move {
            let mut request = [0u8; 512];
            let (size, source) = server.recv_from(&mut request).await?;
            let transaction_id: [u8; 12] = request[8..20]
                .try_into()
                .context("STUN request transaction id")?;
            let response = build_test_response(source, transaction_id, true);
            server.send_to(&response, source).await?;
            anyhow::Ok((source, size))
        });

        let config = NatDetectionConfig {
            enabled: true,
            stun_servers: vec![server_addr.to_string()],
            timeout_ms: 100,
        };
        let (engine, report) = P2pEngine::new_with_nat_detection(&config, 0).await?;
        let (observed_source, _) = server_task.await??;

        assert_eq!(observed_source.port(), engine.local_addr().port());
        assert_eq!(report.mapped_addresses, vec![observed_source]);
        Ok(())
    }

    #[test]
    fn classifies_nat_types_from_mapping_and_filtering() -> Result<()> {
        let local = "192.0.2.10:51820".parse()?;

        assert_eq!(
            classify_nat_type(local, &["192.0.2.10:51820".parse()?], false),
            NatType::Open
        );
        assert_eq!(
            classify_nat_type(local, &["198.51.100.1:40000".parse()?], true),
            NatType::FullCone
        );
        assert_eq!(
            classify_nat_type(
                local,
                &["198.51.100.1:40000".parse()?, "198.51.100.1:40000".parse()?],
                false
            ),
            NatType::Restricted
        );
        assert_eq!(
            classify_nat_type(local, &["198.51.100.1:40000".parse()?], false),
            NatType::PortRestricted
        );
        assert_eq!(
            classify_nat_type(
                local,
                &["198.51.100.1:40000".parse()?, "198.51.100.1:40001".parse()?],
                false
            ),
            NatType::Symmetric
        );
        assert_eq!(classify_nat_type(local, &[], false), NatType::Unknown);
        Ok(())
    }

    #[test]
    fn parses_xor_mapped_ipv4_address() -> Result<()> {
        let transaction_id = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mapped: SocketAddr = "203.0.113.10:54321".parse()?;
        let response = build_test_response(mapped, transaction_id, true);

        let parsed = parse_stun_binding_response(&response, transaction_id)
            .context("failed to parse STUN response")?
            .expect("XOR-MAPPED-ADDRESS response should contain mapped address");

        assert_eq!(parsed.mapped_addr, mapped);
        Ok(())
    }

    #[test]
    fn parses_mapped_ipv6_address() -> Result<()> {
        let transaction_id = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21];
        let mapped: SocketAddr = "[2001:db8::5]:40000".parse()?;
        let response = build_test_response(mapped, transaction_id, false);

        let parsed = parse_stun_binding_response(&response, transaction_id)
            .context("failed to parse STUN response")?
            .expect("MAPPED-ADDRESS response should contain mapped address");

        assert_eq!(parsed.mapped_addr, mapped);
        Ok(())
    }

    fn build_test_response(mapped: SocketAddr, transaction_id: [u8; 12], xor: bool) -> Vec<u8> {
        let mut attr = Vec::new();
        attr.extend_from_slice(
            &(if xor {
                STUN_ATTR_XOR_MAPPED_ADDRESS
            } else {
                STUN_ATTR_MAPPED_ADDRESS
            })
            .to_be_bytes(),
        );

        let mut value = Vec::new();
        value.push(0);
        match mapped {
            SocketAddr::V4(addr) => {
                value.push(0x01);
                let mut port = addr.port();
                let mut octets = addr.ip().octets();
                if xor {
                    port ^= (STUN_MAGIC_COOKIE >> 16) as u16;
                    let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
                    for (octet, cookie_octet) in octets.iter_mut().zip(cookie) {
                        *octet ^= cookie_octet;
                    }
                }
                value.extend_from_slice(&port.to_be_bytes());
                value.extend_from_slice(&octets);
            }
            SocketAddr::V6(addr) => {
                value.push(0x02);
                let mut port = addr.port();
                let mut octets = addr.ip().octets();
                if xor {
                    port ^= (STUN_MAGIC_COOKIE >> 16) as u16;
                    let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
                    for (octet, cookie_octet) in octets[..4].iter_mut().zip(cookie) {
                        *octet ^= cookie_octet;
                    }
                    for (octet, transaction_octet) in octets[4..].iter_mut().zip(transaction_id) {
                        *octet ^= transaction_octet;
                    }
                }
                value.extend_from_slice(&port.to_be_bytes());
                value.extend_from_slice(&octets);
            }
        }

        attr.extend_from_slice(&(value.len() as u16).to_be_bytes());
        attr.extend_from_slice(&value);
        while attr.len() % 4 != 0 {
            attr.push(0);
        }

        let mut response = Vec::new();
        response.extend_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
        response.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        response.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&transaction_id);
        response.extend_from_slice(&attr);
        response
    }

    #[tokio::test]
    async fn test_gather_candidate_endpoints() {
        let mapped = vec!["1.2.3.4:51820".parse().unwrap()];
        let candidates = gather_candidate_endpoints(51820, &mapped).await;
        assert!(candidates.contains(&mapped[0]));
    }

    #[test]
    fn test_nat_pmp_request_and_response_parsing() {
        let req = build_nat_pmp_request(51820, 51820, 3600);
        assert_eq!(req[0], 0);
        assert_eq!(req[1], 2);
        assert_eq!(u16::from_be_bytes([req[4], req[5]]), 51820);
        assert_eq!(u16::from_be_bytes([req[6], req[7]]), 51820);
        assert_eq!(u32::from_be_bytes([req[8], req[9], req[10], req[11]]), 3600);

        // 构造模拟网关成功响应 (result_code = 0)
        let mut resp = [0u8; 16];
        resp[0] = 0;
        resp[1] = 130; // 128 + 2
        resp[2..4].copy_from_slice(&0u16.to_be_bytes()); // result code 0
        resp[4..8].copy_from_slice(&123456u32.to_be_bytes()); // epoch
        resp[8..10].copy_from_slice(&51820u16.to_be_bytes()); // internal port
        resp[10..12].copy_from_slice(&51821u16.to_be_bytes()); // mapped external port
        resp[12..16].copy_from_slice(&3600u32.to_be_bytes()); // lifetime

        let parsed = parse_nat_pmp_response(&resp);
        assert_eq!(parsed, Some((51820, 51821, 3600)));

        // 构造模拟网关错误响应 (result_code = 2, Not Authorized/Refused)
        resp[2..4].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(parse_nat_pmp_response(&resp), None);
    }

    #[test]
    fn test_upnp_soap_body_generation() {
        let body = build_upnp_soap_body(
            "urn:schemas-upnp-org:service:WANIPConnection:1",
            "192.168.1.100",
            51820,
            3600,
        );
        assert!(body.contains("<NewInternalClient>192.168.1.100</NewInternalClient>"));
        assert!(body.contains("<NewInternalPort>51820</NewInternalPort>"));
        assert!(body.contains("<NewExternalPort>51820</NewExternalPort>"));
        assert!(body.contains("<NewProtocol>UDP</NewProtocol>"));
        assert!(body.contains("fuckipv6"));
    }
}
