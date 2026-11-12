//! Isolated end-to-end proof that marked traffic can reach the public Internet
//! through the selected Exit before host-wide routing is enabled.

use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpSocket, UdpSocket};
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

const PROBE_HOST: &str = "www.youtube.com";
const PROBE_PORT: u16 = 443;
const DNS_TIMEOUT: Duration = Duration::from_millis(2_500);
const TCP_TIMEOUT: Duration = Duration::from_millis(2_500);
const TLS_TIMEOUT: Duration = Duration::from_millis(3_500);
const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_AAAA: u16 = 28;
const DNS_CLASS_IN: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeFamily {
    V4,
    V6,
}

impl ProbeFamily {
    fn dns_server(self) -> SocketAddr {
        match self {
            Self::V4 => SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),
            Self::V6 => SocketAddr::new(
                IpAddr::V6("2606:4700:4700::1111".parse().expect("valid DNS IPv6")),
                53,
            ),
        }
    }

    fn any_bind(self) -> SocketAddr {
        match self {
            Self::V4 => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            Self::V6 => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        }
    }

    fn query_type(self) -> u16 {
        match self {
            Self::V4 => DNS_TYPE_A,
            Self::V6 => DNS_TYPE_AAAA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExitPathProbeReport {
    pub(crate) ipv4: bool,
    pub(crate) ipv6: bool,
}

/// Resolve and establish a verified TLS connection to YouTube through sockets
/// carrying `mark`. No unmarked application traffic is used by this probe.
pub(crate) async fn probe_exit_path(
    families: &[ProbeFamily],
    mark: u32,
) -> Result<ExitPathProbeReport> {
    let probe_v4 = families.contains(&ProbeFamily::V4);
    let probe_v6 = families.contains(&ProbeFamily::V6);
    if !probe_v4 && !probe_v6 {
        anyhow::bail!("Exit 数据路径探针没有被选中的地址族");
    }

    let result = match (probe_v4, probe_v6) {
        (true, true) => {
            let (v4, v6) = tokio::join!(
                probe_family(ProbeFamily::V4, mark),
                probe_family(ProbeFamily::V6, mark)
            );
            if let Err(error) = v4 {
                anyhow::bail!("IPv4 Exit 数据路径探针失败: {error:#}");
            }
            if let Err(error) = v6 {
                anyhow::bail!("IPv6 Exit 数据路径探针失败: {error:#}");
            }
            ExitPathProbeReport {
                ipv4: true,
                ipv6: true,
            }
        }
        (true, false) => {
            probe_family(ProbeFamily::V4, mark)
                .await
                .context("IPv4 Exit 数据路径探针失败")?;
            ExitPathProbeReport {
                ipv4: true,
                ipv6: false,
            }
        }
        (false, true) => {
            probe_family(ProbeFamily::V6, mark)
                .await
                .context("IPv6 Exit 数据路径探针失败")?;
            ExitPathProbeReport {
                ipv4: false,
                ipv6: true,
            }
        }
        (false, false) => unreachable!("empty family set rejected above"),
    };
    Ok(result)
}

async fn probe_family(family: ProbeFamily, mark: u32) -> Result<()> {
    let address = query_youtube_address(family, mark).await?;
    let socket = match family {
        ProbeFamily::V4 => TcpSocket::new_v4(),
        ProbeFamily::V6 => TcpSocket::new_v6(),
    }
    .context("无法创建 Exit 数据路径 TCP socket")?;
    set_socket_mark(socket.as_raw_fd(), mark).context("无法为 Exit TCP 探针设置 SO_MARK")?;
    let destination = SocketAddr::new(address, PROBE_PORT);
    let tcp = timeout(TCP_TIMEOUT, socket.connect(destination))
        .await
        .with_context(|| format!("经 Exit 连接 {destination} 超时"))?
        .with_context(|| format!("经 Exit 无法连接 {destination}"))?;

    let connector = youtube_tls_connector()?;
    let server_name = ServerName::try_from(PROBE_HOST)
        .context("YouTube TLS SNI 无效")?
        .to_owned();
    timeout(TLS_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .with_context(|| format!("经 Exit 与 {PROBE_HOST} 完成 TLS 握手超时"))?
        .context("经 Exit 与 YouTube TLS 握手或证书校验失败")?;
    Ok(())
}

async fn query_youtube_address(family: ProbeFamily, mark: u32) -> Result<IpAddr> {
    let (transaction_id, query) = build_dns_query(family)?;
    let socket = StdUdpSocket::bind(family.any_bind())
        .with_context(|| format!("无法绑定 {family:?} Exit DNS 探针 socket"))?;
    set_socket_mark(socket.as_raw_fd(), mark)
        .with_context(|| format!("无法为 {family:?} Exit DNS 探针设置 SO_MARK"))?;
    socket
        .set_nonblocking(true)
        .context("无法设置 Exit DNS 探针 socket 非阻塞")?;
    let socket = UdpSocket::from_std(socket).context("无法接入异步 Exit DNS 探针 socket")?;
    socket
        .connect(family.dns_server())
        .await
        .context("无法连接 Exit DNS 探针目标")?;

    timeout(DNS_TIMEOUT, socket.send(&query))
        .await
        .with_context(|| format!("经 Exit 发送 {family:?} DNS 查询超时"))?
        .context("经 Exit 发送 DNS 查询失败")?;

    let mut response = [0u8; 2_048];
    let len = timeout(DNS_TIMEOUT, socket.recv(&mut response))
        .await
        .with_context(|| format!("经 Exit 等待 {family:?} DNS 响应超时"))?
        .context("经 Exit 接收 DNS 响应失败")?;
    parse_dns_address(&response[..len], transaction_id, family.query_type())
        .with_context(|| format!("经 Exit 的 {family:?} DNS 响应无效"))
}

fn build_dns_query(family: ProbeFamily) -> Result<([u8; 2], Vec<u8>)> {
    let mut transaction_id = [0u8; 2];
    getrandom::fill(&mut transaction_id).context("无法生成 Exit DNS 事务 ID")?;
    let mut query = Vec::with_capacity(64);
    query.extend_from_slice(&transaction_id);
    query.extend_from_slice(&[0x01, 0x00]);
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    for label in PROBE_HOST.split('.') {
        if label.is_empty() || label.len() > 63 {
            anyhow::bail!("YouTube 探针域名标签无效");
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&family.query_type().to_be_bytes());
    query.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
    Ok((transaction_id, query))
}

fn parse_dns_address(packet: &[u8], transaction_id: [u8; 2], query_type: u16) -> Result<IpAddr> {
    if packet.len() < 12 {
        anyhow::bail!("DNS 响应头不完整");
    }
    if packet[0..2] != transaction_id {
        anyhow::bail!("DNS 事务 ID 不匹配");
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 == 0 {
        anyhow::bail!("DNS 响应缺少响应标志");
    }
    if flags & 0x000f != 0 {
        anyhow::bail!("DNS 响应返回错误码 {}", flags & 0x000f);
    }
    if flags & 0x0200 != 0 {
        anyhow::bail!("DNS 响应被截断");
    }

    let question_count = u16::from_be_bytes([packet[4], packet[5]]);
    let answer_count = u16::from_be_bytes([packet[6], packet[7]]);
    let mut offset = 12usize;
    for _ in 0..question_count {
        skip_dns_name(packet, &mut offset)?;
        if packet.len().saturating_sub(offset) < 4 {
            anyhow::bail!("DNS 问题区不完整");
        }
        offset += 4;
    }

    for _ in 0..answer_count {
        skip_dns_name(packet, &mut offset)?;
        if packet.len().saturating_sub(offset) < 10 {
            anyhow::bail!("DNS 答案区不完整");
        }
        let record_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let record_class = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
        let data_len = u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]) as usize;
        offset += 10;
        let end = offset
            .checked_add(data_len)
            .filter(|end| *end <= packet.len())
            .context("DNS 答案数据长度越界")?;
        if record_type == query_type && record_class == DNS_CLASS_IN {
            let data = &packet[offset..end];
            return match (query_type, data.len()) {
                (DNS_TYPE_A, 4) => Ok(IpAddr::V4(Ipv4Addr::new(
                    data[0], data[1], data[2], data[3],
                ))),
                (DNS_TYPE_AAAA, 16) => {
                    let octets: [u8; 16] = data
                        .try_into()
                        .expect("IPv6 DNS data length checked immediately above");
                    Ok(IpAddr::V6(Ipv6Addr::from(octets)))
                }
                _ => anyhow::bail!("DNS 地址记录长度无效"),
            };
        }
        offset = end;
    }
    anyhow::bail!("DNS 响应中没有请求类型的地址记录")
}

fn skip_dns_name(packet: &[u8], offset: &mut usize) -> Result<()> {
    let mut cursor = *offset;
    let mut labels = 0usize;
    loop {
        let length = *packet.get(cursor).context("DNS 名称越过响应边界")?;
        if length & 0xc0 == 0xc0 {
            packet.get(cursor + 1).context("DNS 压缩指针不完整")?;
            *offset = cursor + 2;
            return Ok(());
        }
        if length & 0xc0 != 0 {
            anyhow::bail!("DNS 名称使用了未知标签编码");
        }
        cursor += 1;
        if length == 0 {
            *offset = cursor;
            return Ok(());
        }
        let end = cursor
            .checked_add(length as usize)
            .filter(|end| *end <= packet.len())
            .context("DNS 名称标签越过响应边界")?;
        cursor = end;
        labels += 1;
        if labels > 127 {
            anyhow::bail!("DNS 名称标签过多");
        }
    }
}

fn set_socket_mark(fd: RawFd, mark: u32) -> io::Result<()> {
    let value = mark as libc::c_int;
    // SAFETY: `fd` belongs to a live socket and `value` remains valid for the
    // duration of the setsockopt call.
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_MARK,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn youtube_tls_connector() -> Result<TlsConnector> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("无法启用 YouTube 探针默认 TLS 版本")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_packet(transaction_id: [u8; 2], query_type: u16, data: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(&transaction_id);
        packet.extend_from_slice(&[0x81, 0x80]);
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.push(3);
        packet.extend_from_slice(b"www");
        packet.push(7);
        packet.extend_from_slice(b"youtube");
        packet.push(3);
        packet.extend_from_slice(b"com");
        packet.push(0);
        packet.extend_from_slice(&query_type.to_be_bytes());
        packet.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        packet.extend_from_slice(&[0xc0, 0x0c]);
        packet.extend_from_slice(&query_type.to_be_bytes());
        packet.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(data);
        packet
    }

    #[test]
    fn extracts_compressed_ipv4_answer() {
        let packet = answer_packet([0x12, 0x34], DNS_TYPE_A, &[142, 250, 74, 46]);
        assert_eq!(
            parse_dns_address(&packet, [0x12, 0x34], DNS_TYPE_A).unwrap(),
            "142.250.74.46".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn extracts_compressed_ipv6_answer() {
        let address: Ipv6Addr = "2607:f8b0:4005:80b::200e".parse().unwrap();
        let packet = answer_packet([0xab, 0xcd], DNS_TYPE_AAAA, &address.octets());
        assert_eq!(
            parse_dns_address(&packet, [0xab, 0xcd], DNS_TYPE_AAAA).unwrap(),
            IpAddr::V6(address)
        );
    }

    #[test]
    fn rejects_wrong_dns_transaction_id() {
        let packet = answer_packet([0x12, 0x34], DNS_TYPE_A, &[142, 250, 74, 46]);
        assert!(parse_dns_address(&packet, [0x12, 0x35], DNS_TYPE_A).is_err());
    }

    #[test]
    fn rejects_truncated_answer_data() {
        let packet = answer_packet([0x12, 0x34], DNS_TYPE_A, &[142, 250, 74]);
        assert!(parse_dns_address(&packet, [0x12, 0x34], DNS_TYPE_A).is_err());
    }

    #[test]
    fn dns_query_has_one_question_and_requested_family() {
        let (transaction_id, query) = build_dns_query(ProbeFamily::V4).unwrap();
        assert_eq!(&query[0..2], &transaction_id);
        assert_eq!(u16::from_be_bytes([query[4], query[5]]), 1);
        assert!(query.ends_with(&[0, 0, 1, 0, 1]));
    }
}
