//! Optional Linux LXT UDP-assisted TCP handshake rescue.
//!
//! This module carries the real server-generated SYN-ACK over an authenticated
//! UDP side channel. It does not proxy or tunnel established TCP traffic.

mod linux;
mod packet;
mod protocol;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use zeroize::Zeroizing;

pub use protocol::MAX_UDP_DATAGRAM;

const MIN_KEY_LEN: usize = 32;
const MAX_KEY_LEN: usize = 1024;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub interface: String,
    pub bind: Ipv4Addr,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub key_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub interface: String,
    pub server: Ipv4Addr,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub hello_interval: Duration,
    pub tun_name: String,
    pub key_file: PathBuf,
}

pub async fn run_server(config: ServerConfig) -> Result<()> {
    validate_common(&config.interface, config.tcp_port, config.udp_port)?;
    let key = load_key_file(&config.key_file)?;
    linux::run_server(config, key, MAX_UDP_DATAGRAM).await
}

pub async fn run_client(config: ClientConfig) -> Result<()> {
    validate_common(&config.interface, config.tcp_port, config.udp_port)?;
    if config.server.is_unspecified()
        || config.server.is_multicast()
        || config.server.is_broadcast()
    {
        anyhow::bail!("LXT 救援服务端地址必须是明确的单播 IPv4 地址");
    }
    if config.hello_interval < Duration::from_millis(100)
        || config.hello_interval > Duration::from_secs(60)
    {
        anyhow::bail!("LXT HELLO 间隔必须在 100ms 到 60s 之间");
    }
    validate_tun_name(&config.tun_name)?;
    let key = load_key_file(&config.key_file)?;
    linux::run_client(config, key, MAX_UDP_DATAGRAM).await
}

fn validate_common(interface: &str, tcp_port: u16, udp_port: u16) -> Result<()> {
    if interface.trim().is_empty() {
        anyhow::bail!("LXT 救援必须指定 Linux 抓包网卡");
    }
    if tcp_port == 0 {
        anyhow::bail!("LXT TCP 端口不能为 0");
    }
    if udp_port == 0 {
        anyhow::bail!("LXT UDP 端口不能为 0");
    }
    Ok(())
}

fn validate_tun_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 15 || !name.is_ascii() {
        anyhow::bail!("LXT TUN 名称必须是不超过 15 字节的 ASCII 字符串");
    }
    if name == "." || name == ".." || name.contains('/') || name.contains(char::is_whitespace) {
        anyhow::bail!("LXT TUN 名称包含非法字符");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        anyhow::bail!("LXT TUN 名称只允许 ASCII 字母、数字、'-'、'_' 和 '.'");
    }
    Ok(())
}

fn load_key_file(path: &std::path::Path) -> Result<Zeroizing<Vec<u8>>> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("读取 LXT 救援密钥文件元数据失败: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("LXT 救援密钥路径不是普通文件: {}", path.display());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "LXT 救援密钥文件权限过宽 ({mode:o})；请执行 chmod 600 {}",
                path.display()
            );
        }
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("读取 LXT 救援密钥文件失败: {}", path.display()))?;
    decode_hex_key(&contents)
        .with_context(|| format!("解析 LXT 救援密钥文件失败: {}", path.display()))
}

fn decode_hex_key(value: &str) -> Result<Zeroizing<Vec<u8>>> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("LXT 救援密钥不能为空；请使用 `openssl rand -hex 32` 生成");
    }
    if !value.len().is_multiple_of(2) {
        anyhow::bail!("LXT 救援密钥必须是偶数长度的十六进制字符串");
    }

    let mut key = Zeroizing::new(Vec::with_capacity(value.len() / 2));
    let bytes = value.as_bytes();
    for pair in bytes.as_chunks::<2>().0 {
        let high = decode_hex_digit(pair[0])?;
        let low = decode_hex_digit(pair[1])?;
        key.push((high << 4) | low);
    }

    if !(MIN_KEY_LEN..=MAX_KEY_LEN).contains(&key.len()) {
        anyhow::bail!(
            "LXT 救援密钥长度必须为 {MIN_KEY_LEN}..={MAX_KEY_LEN} 字节，当前为 {} 字节",
            key.len()
        );
    }
    Ok(key)
}

fn decode_hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => anyhow::bail!("LXT 救援密钥包含非十六进制字符"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_hex_key() {
        let key = decode_hex_key(&"a5".repeat(32)).unwrap();
        assert_eq!(key.len(), 32);
        assert!(key.iter().all(|byte| *byte == 0xa5));
    }

    #[test]
    fn rejects_short_invalid_and_odd_keys() {
        assert!(decode_hex_key(&"aa".repeat(31)).is_err());
        assert!(decode_hex_key(&"zz".repeat(32)).is_err());
        assert!(decode_hex_key(&"a".repeat(63)).is_err());
    }

    #[test]
    fn validates_linux_interface_names() {
        assert!(validate_tun_name("lxtrescue0").is_ok());
        assert!(validate_tun_name("").is_err());
        assert!(validate_tun_name("this-name-is-too-long").is_err());
        assert!(validate_tun_name("bad/name").is_err());
        assert!(validate_tun_name("bad name").is_err());
    }
}
