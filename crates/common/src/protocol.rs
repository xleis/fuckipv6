use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use crate::types::{ClientId, NatType, PeerInfo, RouteEntry, VirtualIps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    BadRequest = 400,
    Unauthorized = 401,
    Forbidden = 403,
    NotFound = 404,
    RateLimited = 429,
    BadGateway = 502,
    Internal = 500,
    Overloaded = 503,
}

impl ErrorCode {
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    pub const fn default_message(self) -> &'static str {
        match self {
            Self::BadRequest => "Bad request",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not found",
            Self::RateLimited => "Rate limit exceeded",
            Self::BadGateway => "Upstream relay failure",
            Self::Internal => "Internal server error",
            Self::Overloaded => "Service overloaded",
        }
    }
}

/// 信令消息类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalingMessage {
    /// 客户端注册到协调服务器
    Register {
        client_id: ClientId,
        public_key: Vec<u8>,
        virtual_ip: Option<Ipv6Addr>,
        p2p_port: u16,
        #[serde(default)]
        nat_type: NatType,
        #[serde(default)]
        candidate_endpoints: Vec<SocketAddr>,
        timestamp: u64,
        signature: Vec<u8>,
    },

    /// 服务器确认注册
    RegisterAck {
        assigned_ip: Ipv6Addr,
        #[serde(default)]
        assigned_ips: Option<VirtualIps>,
        network_prefix: u8,
        #[serde(default)]
        ipv4_prefix: Option<u8>,
        peers: Vec<PeerInfo>,
        #[serde(default)]
        relay_endpoint: Option<RelayEndpoint>,
        #[serde(default)]
        relay_endpoints: Vec<RelayEndpoint>,
    },

    /// 请求与某个节点建立连接
    ConnectRequest { target_id: ClientId },

    /// 服务器返回目标节点信息
    ConnectInfo { peer: PeerInfo },

    /// 节点连接准备通知 (发送给目标节点)
    ConnectNotify { peer: PeerInfo },

    /// 节点上线通知 (广播给所有在线节点)
    PeerOnline { peer: PeerInfo },

    /// 节点下线通知
    PeerOffline { client_id: ClientId },

    /// 通过协调服务器中继的数据包 (支持 IPv4 / IPv6)
    RelayPacket {
        src_ip: IpAddr,
        dst_ip: IpAddr,
        #[serde(with = "serde_bytes")]
        payload: Vec<u8>,
    },

    /// 心跳
    Heartbeat { timestamp: u64 },

    /// 心跳响应
    HeartbeatAck { timestamp: u64 },

    /// 错误响应
    Error { code: u16, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEndpoint {
    pub addr: SocketAddr,
    pub server_name: String,
    pub public_key_pin: [u8; 32],
}

impl RelayEndpoint {
    pub fn for_signaling_connection(mut self, local_addr: Option<SocketAddr>) -> Self {
        if self.addr.ip().is_unspecified() {
            if let Some(local_addr) = local_addr {
                self.addr = SocketAddr::new(local_addr.ip(), self.addr.port());
            }
        }

        self
    }

    pub fn combine_relay_endpoints(
        primary: Option<&RelayEndpoint>,
        multi: &[RelayEndpoint],
    ) -> Vec<RelayEndpoint> {
        let mut result = Vec::new();
        if let Some(p) = primary {
            result.push(p.clone());
        }
        for ep in multi {
            if !result.contains(ep) {
                result.push(ep.clone());
            }
        }
        result
    }
}

impl SignalingMessage {
    /// 创建注册消息
    pub fn register(
        client_id: ClientId,
        public_key: Vec<u8>,
        p2p_port: u16,
        nat_type: NatType,
        timestamp: u64,
        signature: Vec<u8>,
    ) -> Self {
        Self::register_with_candidates(
            client_id,
            public_key,
            p2p_port,
            nat_type,
            Vec::new(),
            timestamp,
            signature,
        )
    }

    /// 创建包含 ICE 候选端点列表的注册消息
    pub fn register_with_candidates(
        client_id: ClientId,
        public_key: Vec<u8>,
        p2p_port: u16,
        nat_type: NatType,
        candidate_endpoints: Vec<SocketAddr>,
        timestamp: u64,
        signature: Vec<u8>,
    ) -> Self {
        Self::Register {
            client_id,
            public_key,
            virtual_ip: None,
            p2p_port,
            nat_type,
            candidate_endpoints,
            timestamp,
            signature,
        }
    }

    /// 创建包含双栈期望虚拟 IP 与 ICE 候选端点列表的注册消息
    #[allow(clippy::too_many_arguments)]
    pub fn register_with_ips(
        client_id: ClientId,
        public_key: Vec<u8>,
        virtual_ips: Option<VirtualIps>,
        p2p_port: u16,
        nat_type: NatType,
        candidate_endpoints: Vec<SocketAddr>,
        timestamp: u64,
        signature: Vec<u8>,
    ) -> Self {
        let virtual_ip = virtual_ips.as_ref().and_then(|ips| ips.ipv6);
        Self::Register {
            client_id,
            public_key,
            virtual_ip,
            p2p_port,
            nat_type,
            candidate_endpoints,
            timestamp,
            signature,
        }
    }

    /// 构造注册签名的规范字节串。
    pub fn register_challenge(
        client_id: &str,
        public_key: &[u8],
        p2p_port: u16,
        timestamp: u64,
    ) -> Vec<u8> {
        let mut challenge = Vec::with_capacity(client_id.len() + public_key.len() + 32);
        challenge.extend_from_slice(b"fuckipv6-register-v1");
        challenge.extend_from_slice(client_id.as_bytes());
        challenge.extend_from_slice(public_key);
        challenge.extend_from_slice(&p2p_port.to_be_bytes());
        challenge.extend_from_slice(&timestamp.to_be_bytes());
        challenge
    }

    /// 创建中继数据包消息
    pub fn relay_packet(
        src_ip: impl Into<IpAddr>,
        dst_ip: impl Into<IpAddr>,
        payload: Vec<u8>,
    ) -> Self {
        Self::RelayPacket {
            src_ip: src_ip.into(),
            dst_ip: dst_ip.into(),
            payload,
        }
    }

    /// 创建心跳消息
    pub fn heartbeat() -> Self {
        Self::Heartbeat {
            timestamp: now_secs(),
        }
    }

    /// 创建错误消息
    pub fn error(code: u16, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }

    pub fn protocol_error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::error(code.as_u16(), message)
    }

    pub fn default_error(code: ErrorCode) -> Self {
        Self::error(code.as_u16(), code.default_message())
    }
}

/// P2P UDP 数据报
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2pDatagram {
    /// TUN 读出的原始 IP 数据包
    Data(DataPacket),

    /// P2P 控制消息
    Control {
        src_ip: IpAddr,
        message: ControlMessage,
    },

    /// 连接探测，用于 UDP 打洞和 RTT 测量
    Ping { src_ip: IpAddr, timestamp: u64 },

    /// 连接探测响应
    Pong { src_ip: IpAddr, timestamp: u64 },

    /// 洋葱多跳路由控制与中继信元 (3-Hop Onion Routing)
    Onion(crate::onion::OnionCell),

    /// Kademlia DHT 分布式哈希表 RPC 消息
    Dht(crate::dht::DhtMessage),

    /// P2P 内容分块多源拉取协议
    Transfer(crate::chunk::TransferMessage),

    /// 端到端公钥加密点对点消息
    DirectMessage(crate::chunk::P2pDirectMessage),
}

impl P2pDatagram {
    pub fn ping(src_ip: impl Into<IpAddr>) -> Self {
        Self::Ping {
            src_ip: src_ip.into(),
            timestamp: now_micros(),
        }
    }

    pub fn onion(cell: crate::onion::OnionCell) -> Self {
        Self::Onion(cell)
    }

    pub fn dht(msg: crate::dht::DhtMessage) -> Self {
        Self::Dht(msg)
    }

    pub fn transfer(msg: crate::chunk::TransferMessage) -> Self {
        Self::Transfer(msg)
    }

    pub fn direct_message(msg: crate::chunk::P2pDirectMessage) -> Self {
        Self::DirectMessage(msg)
    }

    pub fn pong(src_ip: impl Into<IpAddr>, timestamp: u64) -> Self {
        Self::Pong {
            src_ip: src_ip.into(),
            timestamp,
        }
    }

    pub fn quality_report(
        src_ip: impl Into<IpAddr>,
        rtt_us: u64,
        loss_rate: f32,
        bandwidth_bps: u64,
    ) -> Self {
        Self::Control {
            src_ip: src_ip.into(),
            message: ControlMessage::quality_report(rtt_us, loss_rate, bandwidth_bps),
        }
    }
}

/// 数据包格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPacket {
    /// 源虚拟IP (IPv4 或 IPv6)
    pub src_ip: IpAddr,

    /// 目标虚拟IP (IPv4 或 IPv6)
    pub dst_ip: IpAddr,

    /// 原始IP数据包 (从TUN设备读取)
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,

    /// 时间戳 (用于延迟测量，微秒)
    pub timestamp: u64,
}

impl DataPacket {
    /// 创建新的数据包
    pub fn new(src_ip: impl Into<IpAddr>, dst_ip: impl Into<IpAddr>, payload: Vec<u8>) -> Self {
        Self {
            src_ip: src_ip.into(),
            dst_ip: dst_ip.into(),
            payload,
            timestamp: now_micros(),
        }
    }

    /// 从借用 payload 创建新的数据包，只在实际组包时复制一次。
    pub fn from_payload_slice(
        src_ip: impl Into<IpAddr>,
        dst_ip: impl Into<IpAddr>,
        payload: &[u8],
    ) -> Self {
        Self::new(src_ip, dst_ip, payload.to_vec())
    }

    /// 获取延迟 (微秒)
    pub fn latency(&self) -> u64 {
        now_micros().saturating_sub(self.timestamp)
    }
}

fn now_micros() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// serde_bytes模块用于优化 `Vec<u8>` 的序列化
pub(crate) mod serde_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<u8>::deserialize(deserializer)
    }
}

/// 控制消息 (在QUIC数据报中发送，无序传递)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Ping请求
    Ping { seq: u32 },

    /// Pong响应
    Pong { seq: u32 },

    /// 路由更新
    RouteUpdate { routes: Vec<RouteEntry> },

    /// 连接质量报告
    QualityReport {
        rtt_us: u64,        // 往返时延 (微秒)
        loss_rate: f32,     // 丢包率 (0.0-1.0)
        bandwidth_bps: u64, // 带宽估计 (bps)
    },
}

impl ControlMessage {
    pub fn quality_report(rtt_us: u64, loss_rate: f32, bandwidth_bps: u64) -> Self {
        Self::QualityReport {
            rtt_us,
            loss_rate: loss_rate.clamp(0.0, 1.0),
            bandwidth_bps,
        }
    }

    pub fn quality_values(&self) -> Option<(u64, f32, u64)> {
        match self {
            Self::QualityReport {
                rtt_us,
                loss_rate,
                bandwidth_bps,
            } => Some((*rtt_us, *loss_rate, *bandwidth_bps)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Identity;

    use super::*;

    #[test]
    fn test_signaling_message_serialization() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let client_id = identity.client_id().to_string();
        let public_key = identity.public_key();
        let p2p_port = 51820;
        let timestamp = 123;
        let challenge =
            SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
        let signature = identity.sign(&challenge).to_bytes().to_vec();
        let msg = SignalingMessage::register(
            client_id,
            public_key,
            p2p_port,
            NatType::Restricted,
            timestamp,
            signature,
        );

        let serialized = crate::codec::encode(&msg)?;
        let deserialized: SignalingMessage = crate::codec::decode(&serialized)?;

        match deserialized {
            SignalingMessage::Register {
                client_id,
                nat_type,
                ..
            } => {
                assert!(!client_id.is_empty());
                assert_eq!(nat_type, NatType::Restricted);
            }
            _ => panic!("Wrong message type"),
        }
        Ok(())
    }

    #[test]
    fn test_data_packet_latency() {
        let packet = DataPacket::new(
            Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2),
            vec![1, 2, 3],
        );

        std::thread::sleep(std::time::Duration::from_millis(10));
        let latency = packet.latency();
        assert!(latency >= 10_000); // 至少10ms = 10000us
    }

    #[test]
    fn test_quality_report_serialization() -> anyhow::Result<()> {
        let src_ip = Ipv6Addr::LOCALHOST;
        let datagram = P2pDatagram::quality_report(src_ip, 12_000, 1.5, 50_000);

        let serialized = crate::codec::encode(&datagram)?;
        let decoded: P2pDatagram = crate::codec::decode(&serialized)?;

        match decoded {
            P2pDatagram::Control {
                src_ip: got_ip,
                message,
            } => {
                assert_eq!(got_ip, src_ip);
                assert_eq!(message.quality_values(), Some((12_000, 1.0, 50_000)));
            }
            other => panic!("unexpected datagram: {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn relay_endpoint_rewrites_unspecified_address_for_signaling_peer() -> anyhow::Result<()> {
        let endpoint = RelayEndpoint {
            addr: "0.0.0.0:6667".parse()?,
            server_name: "fuckipv6.local".to_string(),
            public_key_pin: [7; 32],
        };

        let rewritten = endpoint.for_signaling_connection(Some("203.0.113.10:6666".parse()?));
        assert_eq!(rewritten.addr, "203.0.113.10:6667".parse()?);
        Ok(())
    }

    #[test]
    fn signaling_error_uses_named_error_codes() {
        let error = SignalingMessage::default_error(ErrorCode::NotFound);
        match error {
            SignalingMessage::Error { code, message } => {
                assert_eq!(code, 404);
                assert_eq!(message, "Not found");
            }
            other => panic!("unexpected message: {:?}", other),
        }
    }
}
