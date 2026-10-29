use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use std::fmt;

/// fuckipv6 虚拟对等网络邀请票据 (Ticket)
///
/// 格式规范:
/// `fk://<base64_pubkey>@<host:port>?name=<network_name>&v6=<ipv6_prefix>&v4=<ipv4_prefix>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkTicket {
    /// 协调服务或创建者公钥 (Ed25519, 32字节)
    pub pubkey: [u8; 32],
    /// 接入端点 host:port (例如 relay.xlei.cc:6666 或 185.217.110.15:6666)
    pub endpoint: String,
    /// 网络标识名称
    pub network_name: String,
    /// 分配的虚拟 IPv6 地址段 (如 fd00::1000/120)
    pub ipv6_prefix: Option<String>,
    /// 分配的虚拟 IPv4 地址段 (如 10.8.0.0/24)
    pub ipv4_prefix: Option<String>,
}

impl NetworkTicket {
    pub const URI_SCHEME: &'static str = "fk://";

    pub fn new(pubkey: [u8; 32], endpoint: impl Into<String>, network_name: impl Into<String>) -> Self {
        Self {
            pubkey,
            endpoint: endpoint.into(),
            network_name: network_name.into(),
            ipv6_prefix: None,
            ipv4_prefix: None,
        }
    }

    pub fn with_ipv6(mut self, prefix: impl Into<String>) -> Self {
        self.ipv6_prefix = Some(prefix.into());
        self
    }

    pub fn with_ipv4(mut self, prefix: impl Into<String>) -> Self {
        self.ipv4_prefix = Some(prefix.into());
        self
    }

    /// 序列化为规范的 `fk://...` 邀请码
    pub fn encode(&self) -> String {
        let b64_pubkey = URL_SAFE_NO_PAD.encode(self.pubkey);
        let mut uri = format!("{}{b64_pubkey}@{}", Self::URI_SCHEME, self.endpoint);

        let mut query_params = Vec::new();
        if !self.network_name.is_empty() {
            query_params.push(format!("name={}", urlencoding::encode(&self.network_name)));
        }
        if let Some(v6) = &self.ipv6_prefix {
            query_params.push(format!("v6={}", urlencoding::encode(v6)));
        }
        if let Some(v4) = &self.ipv4_prefix {
            query_params.push(format!("v4={}", urlencoding::encode(v4)));
        }

        if !query_params.is_empty() {
            uri.push('?');
            uri.push_str(&query_params.join("&"));
        }

        uri
    }

    /// 从 `fk://...` 字符串解析网络邀请凭证
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if !input.starts_with(Self::URI_SCHEME) {
            anyhow::bail!("无效的网络凭证格式: 必须以 '{}' 开头", Self::URI_SCHEME);
        }

        let body = &input[Self::URI_SCHEME.len()..];
        let (main_part, query_part) = match body.find('?') {
            Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
            None => (body, None),
        };

        let at_idx = main_part
            .find('@')
            .context("无效凭证: 缺少 '@' 分隔公钥与端点")?;
        let pubkey_str = &main_part[..at_idx];
        let endpoint_str = &main_part[at_idx + 1..];

        if endpoint_str.is_empty() {
            anyhow::bail!("无效凭证: 接入端点为空");
        }

        let pubkey_bytes = URL_SAFE_NO_PAD
            .decode(pubkey_str)
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(pubkey_str))
            .context("公钥 Base64 解码失败")?;

        if pubkey_bytes.len() != 32 {
            anyhow::bail!("公钥长度错误: 预期 32 字节, 实际 {} 字节", pubkey_bytes.len());
        }

        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&pubkey_bytes);

        let mut network_name = "default".to_string();
        let mut ipv6_prefix = None;
        let mut ipv4_prefix = None;

        if let Some(queries) = query_part {
            for item in queries.split('&') {
                if let Some((k, v)) = item.split_once('=') {
                    let decoded_val = urlencoding::decode(v).unwrap_or_else(|_| v.into()).to_string();
                    match k {
                        "name" => network_name = decoded_val,
                        "v6" => ipv6_prefix = Some(decoded_val),
                        "v4" => ipv4_prefix = Some(decoded_val),
                        _ => {}
                    }
                }
            }
        }

        Ok(Self {
            pubkey,
            endpoint: endpoint_str.to_string(),
            network_name,
            ipv6_prefix,
            ipv4_prefix,
        })
    }

    /// 获取 WebSocket 协调服务器 URL
    pub fn websocket_url(&self) -> String {
        if self.endpoint.starts_with("ws://") || self.endpoint.starts_with("wss://") {
            self.endpoint.clone()
        } else {
            format!("ws://{}", self.endpoint)
        }
    }

    /// 获取公钥 Base64 编码
    pub fn pubkey_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pubkey)
    }
}

impl fmt::Display for NetworkTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ticket_roundtrip() {
        let pubkey = [42u8; 32];
        let ticket = NetworkTicket::new(pubkey, "relay.xlei.cc:6666", "team-vpn")
            .with_ipv6("fd00::1000/120")
            .with_ipv4("10.8.0.0/24");

        let encoded = ticket.encode();
        assert!(encoded.starts_with("fk://"));
        assert!(encoded.contains("@relay.xlei.cc:6666"));
        assert!(encoded.contains("name=team-vpn"));
        assert!(encoded.contains("v6=fd00%3A%3A1000%2F120") || encoded.contains("v6=fd00::1000/120"));

        let parsed = NetworkTicket::parse(&encoded).expect("parse ticket failed");
        assert_eq!(parsed.pubkey, pubkey);
        assert_eq!(parsed.endpoint, "relay.xlei.cc:6666");
        assert_eq!(parsed.network_name, "team-vpn");
        assert_eq!(parsed.ipv6_prefix.as_deref(), Some("fd00::1000/120"));
        assert_eq!(parsed.ipv4_prefix.as_deref(), Some("10.8.0.0/24"));
        assert_eq!(parsed.websocket_url(), "ws://relay.xlei.cc:6666");
    }

    #[test]
    fn test_ticket_invalid_scheme() {
        assert!(NetworkTicket::parse("http://example.com").is_err());
        assert!(NetworkTicket::parse("fk://").is_err());
        assert!(NetworkTicket::parse("fk://invalid_no_at").is_err());
    }

    #[test]
    fn test_ticket_defaults() {
        let pubkey = [7u8; 32];
        let b64 = URL_SAFE_NO_PAD.encode(pubkey);
        let raw = format!("fk://{}@1.2.3.4:6666", b64);
        let parsed = NetworkTicket::parse(&raw).expect("parse simple ticket");
        assert_eq!(parsed.pubkey, pubkey);
        assert_eq!(parsed.endpoint, "1.2.3.4:6666");
        assert_eq!(parsed.network_name, "default");
        assert!(parsed.ipv6_prefix.is_none());
    }
}
