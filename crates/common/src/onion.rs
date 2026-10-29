use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::codec;
use crate::crypto::{decrypt_payload, encrypt_payload, Identity};

/// 洋葱电路节点元数据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnionHopNode {
    /// 节点客户端唯一 ID
    pub client_id: String,
    /// 节点虚拟 IP 地址
    pub virtual_ip: IpAddr,
    /// 节点的 X25519 公钥字节数组 (用于 Diffie-Hellman 协商)
    pub public_key: [u8; 32],
    /// 节点物理端点 (可选)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<SocketAddr>,
}

impl OnionHopNode {
    pub fn new(
        client_id: impl Into<String>,
        virtual_ip: IpAddr,
        public_key: [u8; 32],
        endpoint: Option<SocketAddr>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            virtual_ip,
            public_key,
            endpoint,
        }
    }

    /// 从客户端 Identity 构造 Hop 节点元数据
    pub fn from_identity(
        identity: &Identity,
        virtual_ip: IpAddr,
        endpoint: Option<SocketAddr>,
    ) -> Self {
        Self {
            client_id: identity.client_id().to_string(),
            virtual_ip,
            public_key: identity.x25519_public().to_bytes(),
            endpoint,
        }
    }
}

/// 中继节点剥开一层洋葱后获得的路由控制指令
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnionNextHop {
    /// 继续转发至下一跳中继节点
    Forward {
        /// 下一跳节点信息
        next_hop: OnionHopNode,
        /// 电路标识符
        circuit_id: u64,
        /// 剥开后剩余的内层洋葱载荷
        inner_payload: Vec<u8>,
    },
    /// 已到达最后一跳（出口中继），解封最终数据并交付给目标虚拟 IP
    Deliver {
        /// 最终接收方的虚拟 IP
        destination_ip: IpAddr,
        /// 解密出的应用层明文数据包
        payload: Vec<u8>,
    },
}

/// 3-Hop 客户端洋葱电路
pub struct OnionCircuit {
    pub circuit_id: u64,
    pub hops: [OnionHopNode; 3],
    /// 与 Hop 1 (入口守护节点) 的共享对称密钥
    pub key1: [u8; 32],
    /// 与 Hop 2 (中间中继节点) 的共享对称密钥
    pub key2: [u8; 32],
    /// 与 Hop 3 (出口中继节点) 的共享对称密钥
    pub key3: [u8; 32],
}

impl OnionCircuit {
    /// 使用预先协商好的 3 组密钥构建电路
    pub fn new(
        circuit_id: u64,
        hop1: OnionHopNode,
        hop2: OnionHopNode,
        hop3: OnionHopNode,
        key1: [u8; 32],
        key2: [u8; 32],
        key3: [u8; 32],
    ) -> Self {
        Self {
            circuit_id,
            hops: [hop1, hop2, hop3],
            key1,
            key2,
            key3,
        }
    }

    /// 基于发起端的私钥与 3 个中继节点的公钥，使用 X25519 Diffie-Hellman + Blake3 KDF
    /// 自动派生 3 层互不相同的对称密钥
    pub fn build(
        circuit_id: u64,
        client_secret: &StaticSecret,
        hop1: OnionHopNode,
        hop2: OnionHopNode,
        hop3: OnionHopNode,
    ) -> Result<Self> {
        let key1 = derive_hop_key(client_secret, &hop1.public_key, 1)?;
        let key2 = derive_hop_key(client_secret, &hop2.public_key, 2)?;
        let key3 = derive_hop_key(client_secret, &hop3.public_key, 3)?;

        Ok(Self::new(circuit_id, hop1, hop2, hop3, key1, key2, key3))
    }

    /// 基于客户端 Identity 快速构建 3 跳电路
    pub fn build_from_identity(
        circuit_id: u64,
        identity: &Identity,
        hop1: OnionHopNode,
        hop2: OnionHopNode,
        hop3: OnionHopNode,
    ) -> Result<Self> {
        let secret = identity.x25519_secret();
        Self::build(circuit_id, &secret, hop1, hop2, hop3)
    }

    /// 获取第一跳（入口节点）信息
    pub fn entry_hop(&self) -> &OnionHopNode {
        &self.hops[0]
    }

    /// 获取第二跳（中间节点）信息
    pub fn middle_hop(&self) -> &OnionHopNode {
        &self.hops[1]
    }

    /// 获取第三跳（出口节点）信息
    pub fn exit_hop(&self) -> &OnionHopNode {
        &self.hops[2]
    }

    /// 将明文载荷从内到外进行 3 层洋葱加密封包
    ///
    /// - 第 3 层 (出口节点): 加密 Deliver 指令 -> 包含真实目的 IP 与 明文数据
    /// - 第 2 层 (中间节点): 加密 Forward 指令 -> 包含出口节点地址与第 3 层密文
    /// - 第 1 层 (入口节点): 加密 Forward 指令 -> 包含中间节点地址与第 2 层密文
    pub fn pack_onion(&self, destination_ip: IpAddr, payload: &[u8]) -> Result<Vec<u8>> {
        // 1. 构建第 3 层（出口节点 Layer 3）
        let hop3_action = OnionNextHop::Deliver {
            destination_ip,
            payload: payload.to_vec(),
        };
        let hop3_encoded = codec::encode(&hop3_action).context("序列化出口节点洋葱层失败")?;
        let layer3_ciphertext = encrypt_payload(&self.key3, &hop3_encoded)
            .context("加密出口节点 (Layer 3) 失败")?;

        // 2. 构建第 2 层（中间节点 Layer 2）
        let hop2_action = OnionNextHop::Forward {
            next_hop: self.hops[2].clone(),
            circuit_id: self.circuit_id,
            inner_payload: layer3_ciphertext,
        };
        let hop2_encoded = codec::encode(&hop2_action).context("序列化中间节点洋葱层失败")?;
        let layer2_ciphertext = encrypt_payload(&self.key2, &hop2_encoded)
            .context("加密中间节点 (Layer 2) 失败")?;

        // 3. 构建第 1 层（入口守护节点 Layer 1）
        let hop1_action = OnionNextHop::Forward {
            next_hop: self.hops[1].clone(),
            circuit_id: self.circuit_id,
            inner_payload: layer2_ciphertext,
        };
        let hop1_encoded = codec::encode(&hop1_action).context("序列化入口节点洋葱层失败")?;
        let layer1_ciphertext = encrypt_payload(&self.key1, &hop1_encoded)
            .context("加密入口节点 (Layer 1) 失败")?;

        Ok(layer1_ciphertext)
    }

    /// 接收逆向洋葱回复（来自目标节点通过 Hop 3 -> Hop 2 -> Hop 1 返回的数据）
    /// 依次剥离 Layer 1 -> Layer 2 -> Layer 3 得到最终明文
    pub fn unpack_reverse(&self, reverse_payload: &[u8]) -> Result<Vec<u8>> {
        let dec1 = decrypt_payload(&self.key1, reverse_payload)
            .context("逆向洋葱解密 Layer 1 失败")?;
        let dec2 = decrypt_payload(&self.key2, &dec1)
            .context("逆向洋葱解密 Layer 2 失败")?;
        let dec3 = decrypt_payload(&self.key3, &dec2)
            .context("逆向洋葱解密 Layer 3 失败")?;
        Ok(dec3)
    }
}

/// 中继节点剥离一层洋葱外皮
///
/// 中继仅使用其与该电路协商的共享密钥解密收到的密文，解密后获得 `OnionNextHop`：
/// - 若为 `Forward`：获得下一跳中继地址及剥离后的密文，转发即可；
/// - 若为 `Deliver`：本节点为出口节点，获得最终目标虚拟 IP 与应用载荷，直接交付给目标。
pub fn peel_onion_layer(shared_key: &[u8; 32], layer_ciphertext: &[u8]) -> Result<OnionNextHop> {
    let plaintext = decrypt_payload(shared_key, layer_ciphertext)
        .context("中继剥离洋葱层解密失败：认证标签不匹配或密文已被篡改")?;
    let next_hop = codec::decode::<OnionNextHop>(&plaintext)
        .context("解析洋葱下一跳路由控制指令失败")?;
    Ok(next_hop)
}

/// 逆向洋葱加密（各中继节点向返回数据加盖一层加密）
pub fn pack_reverse_layer(shared_key: &[u8; 32], payload: &[u8]) -> Result<Vec<u8>> {
    encrypt_payload(shared_key, payload).context("逆向洋葱单层加密失败")
}

/// 客户端基于 X25519 协商指定跳数的会话对称密钥
pub fn derive_hop_key(
    client_secret: &StaticSecret,
    hop_public_key: &[u8; 32],
    hop_index: u8,
) -> Result<[u8; 32]> {
    let hop_pub = X25519PublicKey::from(*hop_public_key);
    let shared = client_secret.diffie_hellman(&hop_pub);
    let context = format!("fuckipv6 onion circuit v1 hop {hop_index}");
    let key = blake3::derive_key(&context, shared.as_bytes());
    Ok(key)
}

/// 中继端基于自身的 X25519 私钥与客户端公钥，计算与客户端相同的指定跳数对称密钥
pub fn derive_relay_key(
    relay_secret: &StaticSecret,
    client_public_key: &[u8; 32],
    hop_index: u8,
) -> Result<[u8; 32]> {
    let client_pub = X25519PublicKey::from(*client_public_key);
    let shared = relay_secret.diffie_hellman(&client_pub);
    let context = format!("fuckipv6 onion circuit v1 hop {hop_index}");
    let key = blake3::derive_key(&context, shared.as_bytes());
    Ok(key)
}

/// 洋葱电路控制信元协议
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnionCell {
    /// 请求在第一跳建立新电路
    Create {
        circuit_id: u64,
        client_public_key: [u8; 32],
    },
    /// 第一跳响应电路已就绪
    Created {
        circuit_id: u64,
        relay_public_key: [u8; 32],
    },
    /// 请求沿现有链路向下一跳扩展电路
    Extend {
        circuit_id: u64,
        next_hop: OnionHopNode,
        client_ephemeral_key: [u8; 32],
    },
    /// 扩展电路成功响应
    Extended {
        circuit_id: u64,
        relay_ephemeral_key: [u8; 32],
    },
    /// 沿电路传输多层洋葱加密的数据帧
    RelayData {
        circuit_id: u64,
        onion_payload: Vec<u8>,
    },
    /// 拆除电路
    Destroy {
        circuit_id: u64,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn test_diffie_hellman_key_symmetry_for_all_hops() {
        let client_id = Identity::generate().unwrap();
        let hop1_id = Identity::generate().unwrap();
        let hop2_id = Identity::generate().unwrap();
        let hop3_id = Identity::generate().unwrap();

        let client_sec = client_id.x25519_secret();
        let client_pub = client_id.x25519_public().to_bytes();

        let hop1_sec = hop1_id.x25519_secret();
        let hop1_pub = hop1_id.x25519_public().to_bytes();

        let hop2_sec = hop2_id.x25519_secret();
        let hop2_pub = hop2_id.x25519_public().to_bytes();

        let hop3_sec = hop3_id.x25519_secret();
        let hop3_pub = hop3_id.x25519_public().to_bytes();

        // 验证三跳秘钥双方推导完全一致
        let k1_client = derive_hop_key(&client_sec, &hop1_pub, 1).unwrap();
        let k1_relay = derive_relay_key(&hop1_sec, &client_pub, 1).unwrap();
        assert_eq!(k1_client, k1_relay);

        let k2_client = derive_hop_key(&client_sec, &hop2_pub, 2).unwrap();
        let k2_relay = derive_relay_key(&hop2_sec, &client_pub, 2).unwrap();
        assert_eq!(k2_client, k2_relay);

        let k3_client = derive_hop_key(&client_sec, &hop3_pub, 3).unwrap();
        let k3_relay = derive_relay_key(&hop3_sec, &client_pub, 3).unwrap();
        assert_eq!(k3_client, k3_relay);

        // 验证不同 Hop 之间密钥完全隔离独立
        assert_ne!(k1_client, k2_client);
        assert_ne!(k2_client, k3_client);
        assert_ne!(k1_client, k3_client);
    }

    #[test]
    fn test_3hop_onion_encapsulation_and_peeling() {
        // 1. 初始化参与方身份
        let alice = Identity::generate().unwrap();
        let hop1 = Identity::generate().unwrap();
        let hop2 = Identity::generate().unwrap();
        let hop3 = Identity::generate().unwrap();

        let hop1_node = OnionHopNode::from_identity(
            &hop1,
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            Some("192.168.1.101:50001".parse().unwrap()),
        );
        let hop2_node = OnionHopNode::from_identity(
            &hop2,
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2)),
            Some("192.168.1.102:50002".parse().unwrap()),
        );
        let hop3_node = OnionHopNode::from_identity(
            &hop3,
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 3)),
            Some("192.168.1.103:50003".parse().unwrap()),
        );

        // 2. Alice 建立 3 跳电路
        let circuit_id = 10086;
        let circuit = OnionCircuit::build_from_identity(
            circuit_id,
            &alice,
            hop1_node.clone(),
            hop2_node.clone(),
            hop3_node.clone(),
        )
        .unwrap();

        let target_dest: IpAddr = "fd00::dead:beef".parse().unwrap();
        let secret_message = b"Top secret classified message routed through 3 hops!";

        // 3. Alice 封包：生成 3 层洋葱加密密文
        let onion_packet = circuit.pack_onion(target_dest, secret_message).unwrap();

        // 4. 第一跳 (Hop 1 / Guard) 接收并解密其自身层
        let hop1_sec = hop1.x25519_secret();
        let k1_hop = derive_relay_key(&hop1_sec, &alice.x25519_public().to_bytes(), 1).unwrap();
        let hop1_action = peel_onion_layer(&k1_hop, &onion_packet).unwrap();

        let (_hop2_forward_target, inner_for_hop2) = match hop1_action {
            OnionNextHop::Forward {
                next_hop,
                circuit_id: cid,
                inner_payload,
            } => {
                assert_eq!(cid, circuit_id);
                assert_eq!(next_hop, hop2_node);
                (next_hop, inner_payload)
            }
            OnionNextHop::Deliver { .. } => panic!("Hop 1 绝不应该直接 Deliver！"),
        };

        // 验证：Hop 1 无法解开 inner_for_hop2
        assert!(peel_onion_layer(&k1_hop, &inner_for_hop2).is_err());

        // 5. 第二跳 (Hop 2 / Middle Relay) 接收并解密其自身层
        let hop2_sec = hop2.x25519_secret();
        let k2_hop = derive_relay_key(&hop2_sec, &alice.x25519_public().to_bytes(), 2).unwrap();
        let hop2_action = peel_onion_layer(&k2_hop, &inner_for_hop2).unwrap();

        let (_hop3_forward_target, inner_for_hop3) = match hop2_action {
            OnionNextHop::Forward {
                next_hop,
                circuit_id: cid,
                inner_payload,
            } => {
                assert_eq!(cid, circuit_id);
                assert_eq!(next_hop, hop3_node);
                (next_hop, inner_payload)
            }
            OnionNextHop::Deliver { .. } => panic!("Hop 2 绝不应该直接 Deliver！"),
        };

        // 验证：Hop 2 无法解开 inner_for_hop3
        assert!(peel_onion_layer(&k2_hop, &inner_for_hop3).is_err());

        // 6. 第三跳 (Hop 3 / Exit Relay) 接收并解密最后一层
        let hop3_sec = hop3.x25519_secret();
        let k3_hop = derive_relay_key(&hop3_sec, &alice.x25519_public().to_bytes(), 3).unwrap();
        let hop3_action = peel_onion_layer(&k3_hop, &inner_for_hop3).unwrap();

        match hop3_action {
            OnionNextHop::Deliver {
                destination_ip,
                payload,
            } => {
                assert_eq!(destination_ip, target_dest);
                assert_eq!(payload, secret_message);
            }
            OnionNextHop::Forward { .. } => panic!("Hop 3 应该是出口节点，必须执行 Deliver！"),
        }
    }

    #[test]
    fn test_reverse_onion_path_encryption() {
        let alice = Identity::generate().unwrap();
        let hop1 = Identity::generate().unwrap();
        let hop2 = Identity::generate().unwrap();
        let hop3 = Identity::generate().unwrap();

        let circuit = OnionCircuit::build_from_identity(
            999,
            &alice,
            OnionHopNode::from_identity(&hop1, "fd00::1".parse().unwrap(), None),
            OnionHopNode::from_identity(&hop2, "fd00::2".parse().unwrap(), None),
            OnionHopNode::from_identity(&hop3, "fd00::3".parse().unwrap(), None),
        )
        .unwrap();

        let bob_response = b"Hello Alice, this response is routed backwards!";

        // 目标 Bob 经由 Hop 3 -> Hop 2 -> Hop 1 逐层回传并加密
        let c3 = pack_reverse_layer(&circuit.key3, bob_response).unwrap();
        let c2 = pack_reverse_layer(&circuit.key2, &c3).unwrap();
        let c1 = pack_reverse_layer(&circuit.key1, &c2).unwrap();

        // Alice 收到外层数据并一次性剥出最终回复
        let unpacked = circuit.unpack_reverse(&c1).unwrap();
        assert_eq!(unpacked, bob_response);
    }

    #[test]
    fn test_tamper_detection_in_any_hop() {
        let alice = Identity::generate().unwrap();
        let hop1 = Identity::generate().unwrap();
        let hop2 = Identity::generate().unwrap();
        let hop3 = Identity::generate().unwrap();

        let circuit = OnionCircuit::build_from_identity(
            777,
            &alice,
            OnionHopNode::from_identity(&hop1, "fd00::1".parse().unwrap(), None),
            OnionHopNode::from_identity(&hop2, "fd00::2".parse().unwrap(), None),
            OnionHopNode::from_identity(&hop3, "fd00::3".parse().unwrap(), None),
        )
        .unwrap();

        let mut packet = circuit
            .pack_onion("fd00::8888".parse().unwrap(), b"Integrity check")
            .unwrap();

        // 篡改第 1 层的任意字节
        let last_idx = packet.len() - 1;
        packet[last_idx] ^= 0x5a;

        // Hop 1 解密应直接失败 (Poly1305 MAC 校验不通过)
        let k1_hop = derive_relay_key(
            &hop1.x25519_secret(),
            &alice.x25519_public().to_bytes(),
            1,
        )
        .unwrap();
        assert!(peel_onion_layer(&k1_hop, &packet).is_err());
    }

    #[test]
    fn test_onion_cell_roundtrip() {
        let cell = OnionCell::Extend {
            circuit_id: 12345,
            next_hop: OnionHopNode::new(
                "peer-id-123",
                "fd00::abc".parse().unwrap(),
                [42u8; 32],
                Some("1.2.3.4:9000".parse().unwrap()),
            ),
            client_ephemeral_key: [7u8; 32],
        };

        let encoded = codec::encode(&cell).unwrap();
        let decoded: OnionCell = codec::decode(&encoded).unwrap();
        assert_eq!(cell, decoded);
    }
}
