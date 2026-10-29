use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};

use fuckipv6_common::{
    codec, derive_relay_key, peel_onion_layer, ChunkAssembler, DhtContact, DhtMessage,
    DhtRoutingTable, DhtStorage, FileManifest, Identity, NodeId, OnionCell, OnionNextHop,
    P2pDatagram, P2pDirectMessage, TransferMessage, K_BUCKET_SIZE,
};

type IncomingPacket = (IpAddr, Vec<u8>);

/// P2P 覆盖网络统一管理器 (集成了 3-Hop 洋葱多跳中继、Kademlia DHT 路由表与存储、Blake3 分块传输及端到端加密消息)
#[allow(clippy::type_complexity)]
pub struct P2pOverlay {
    /// 本节点网络身份 (如果已加载)
    pub identity: Arc<RwLock<Option<Identity>>>,
    /// 本节点虚拟 IP
    pub local_ip: Arc<RwLock<Option<IpAddr>>>,
    /// 本节点 Kademlia NodeId
    pub local_node_id: NodeId,
    /// 洋葱电路路由状态表：circuit_id -> hop 共享会话密钥
    pub onion_circuits: Arc<RwLock<HashMap<u64, [u8; 32]>>>,
    /// Kademlia DHT 256 桶路由表
    pub dht_routing: Arc<RwLock<DhtRoutingTable>>,
    /// DHT 本地分布式键值存储
    pub dht_storage: Arc<RwLock<DhtStorage>>,
    /// 已发布/缓存的分块文件：file_id -> (元数据清单, 分块字节向量)
    pub published_files: Arc<RwLock<HashMap<[u8; 32], (FileManifest, Vec<Vec<u8>>)>>>,
    /// 正在接收组装中的分块文件组装器：file_id -> ChunkAssembler
    pub active_assemblers: Arc<RwLock<HashMap<[u8; 32], ChunkAssembler>>>,
    /// 收到的端到端加密直接消息队列
    pub received_messages: Arc<RwLock<Vec<(P2pDirectMessage, Vec<u8>)>>>,
}

impl P2pOverlay {
    /// 创建新的覆盖网络引擎
    pub fn new(local_node_id: NodeId) -> Self {
        Self {
            identity: Arc::new(RwLock::new(None)),
            local_ip: Arc::new(RwLock::new(None)),
            local_node_id,
            onion_circuits: Arc::new(RwLock::new(HashMap::new())),
            dht_routing: Arc::new(RwLock::new(DhtRoutingTable::new(local_node_id))),
            dht_storage: Arc::new(RwLock::new(DhtStorage::new())),
            published_files: Arc::new(RwLock::new(HashMap::new())),
            active_assemblers: Arc::new(RwLock::new(HashMap::new())),
            received_messages: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 基于 Identity 初始化覆盖网络
    pub fn from_identity(identity: &Identity) -> Self {
        let node_id = NodeId::from_public_key(&identity.public_key_bytes());
        Self {
            identity: Arc::new(RwLock::new(Some(identity.clone()))),
            local_ip: Arc::new(RwLock::new(None)),
            local_node_id: node_id,
            onion_circuits: Arc::new(RwLock::new(HashMap::new())),
            dht_routing: Arc::new(RwLock::new(DhtRoutingTable::new(node_id))),
            dht_storage: Arc::new(RwLock::new(DhtStorage::new())),
            published_files: Arc::new(RwLock::new(HashMap::new())),
            active_assemblers: Arc::new(RwLock::new(HashMap::new())),
            received_messages: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 设置本地加密身份
    pub async fn set_identity(&self, identity: Identity) {
        *self.identity.write().await = Some(identity);
    }

    /// 设置本地虚拟 IP
    pub async fn set_local_ip(&self, ip: IpAddr) {
        *self.local_ip.write().await = Some(ip);
    }

    /// 获取本节点的 DHT 联系人信息
    pub async fn local_contact(&self, endpoints: Vec<SocketAddr>) -> DhtContact {
        let virtual_ip = self
            .local_ip
            .read()
            .await
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        DhtContact::new(self.local_node_id, virtual_ip, endpoints)
    }

    // -------------------------------------------------------------------------
    // 5.1 洋葱多跳中继处理
    // -------------------------------------------------------------------------

    /// 登记或协商一条洋葱中继跳数密钥
    pub async fn register_onion_circuit(&self, circuit_id: u64, shared_key: [u8; 32]) {
        self.onion_circuits
            .write()
            .await
            .insert(circuit_id, shared_key);
        tracing::info!("洋葱中继电路 #{} 共享密钥已注册就绪", circuit_id);
    }

    /// 处理收到的洋葱协议信元
    pub async fn handle_onion_cell(
        &self,
        cell: OnionCell,
        src_addr: SocketAddr,
        socket: &UdpSocket,
        incoming_tx: &mpsc::UnboundedSender<IncomingPacket>,
    ) {
        match cell {
            OnionCell::RelayData {
                circuit_id,
                onion_payload,
            } => {
                let shared_key = {
                    let map = self.onion_circuits.read().await;
                    map.get(&circuit_id).copied()
                };

                let Some(key) = shared_key else {
                    tracing::warn!("收到未知洋葱电路 #{} 的中继数据，直接丢弃", circuit_id);
                    return;
                };

                match peel_onion_layer(&key, &onion_payload) {
                    Ok(OnionNextHop::Forward {
                        next_hop,
                        circuit_id: next_circuit_id,
                        inner_payload,
                    }) => {
                        if let Some(target_endpoint) = next_hop.endpoint {
                            tracing::debug!(
                                "洋葱中继: 成功剥离一层外皮，向下一跳 {} 转发电路 #{} 的数据",
                                target_endpoint,
                                next_circuit_id
                            );
                            let relay_cell = OnionCell::RelayData {
                                circuit_id: next_circuit_id,
                                onion_payload: inner_payload,
                            };
                            let datagram = P2pDatagram::onion(relay_cell);
                            if let Ok(encoded) = codec::encode(&datagram) {
                                let _ = socket.send_to(&encoded, target_endpoint).await;
                            }
                        } else {
                            tracing::warn!(
                                "洋葱下一跳节点 {} 缺少物理端点，无法转发",
                                next_hop.client_id
                            );
                        }
                    }
                    Ok(OnionNextHop::Deliver {
                        destination_ip,
                        payload,
                    }) => {
                        tracing::info!(
                            "洋葱出口节点: 成功解封最终应用层载荷 ({} 字节)，交付至目标虚拟 IP: {}",
                            payload.len(),
                            destination_ip
                        );
                        let _ = incoming_tx.send((destination_ip, payload));
                    }
                    Err(e) => {
                        tracing::warn!("洋葱中继解密剥离失败 (密文被篡改或私钥不匹配): {:#}", e);
                    }
                }
            }
            OnionCell::Create {
                circuit_id,
                client_public_key,
            } => {
                let identity_opt = self.identity.read().await.clone();
                if let Some(identity) = identity_opt {
                    let relay_secret = identity.x25519_secret();
                    match derive_relay_key(&relay_secret, &client_public_key, 1) {
                        Ok(key) => {
                            self.onion_circuits.write().await.insert(circuit_id, key);
                            let resp = OnionCell::Created {
                                circuit_id,
                                relay_public_key: identity.x25519_public().to_bytes(),
                            };
                            let datagram = P2pDatagram::onion(resp);
                            if let Ok(encoded) = codec::encode(&datagram) {
                                let _ = socket.send_to(&encoded, src_addr).await;
                            }
                            tracing::info!("洋葱电路 #{} 第一跳握手完成", circuit_id);
                        }
                        Err(e) => {
                            tracing::error!("洋葱电路协商密钥推导失败: {:#}", e);
                        }
                    }
                }
            }
            OnionCell::Created {
                circuit_id,
                relay_public_key,
            } => {
                tracing::info!(
                    "洋葱电路 #{} 建立成功，对端公钥: {:02x?}...",
                    circuit_id,
                    &relay_public_key[..4]
                );
            }
            OnionCell::Extend {
                circuit_id,
                next_hop,
                client_ephemeral_key,
            } => {
                let identity_opt = self.identity.read().await.clone();
                if let Some(identity) = identity_opt {
                    let relay_secret = identity.x25519_secret();
                    if let Ok(key) = derive_relay_key(&relay_secret, &client_ephemeral_key, 2) {
                        self.onion_circuits.write().await.insert(circuit_id, key);
                        let resp = OnionCell::Extended {
                            circuit_id,
                            relay_ephemeral_key: identity.x25519_public().to_bytes(),
                        };
                        let datagram = P2pDatagram::onion(resp);
                        if let Ok(encoded) = codec::encode(&datagram) {
                            let _ = socket.send_to(&encoded, src_addr).await;
                        }
                        tracing::info!(
                            "洋葱电路 #{} 扩展至节点 {} 完成",
                            circuit_id,
                            next_hop.client_id
                        );
                    }
                }
            }
            OnionCell::Extended {
                circuit_id,
                relay_ephemeral_key,
            } => {
                tracing::info!(
                    "洋葱电路 #{} 扩展成功，中间中继公钥: {:02x?}...",
                    circuit_id,
                    &relay_ephemeral_key[..4]
                );
            }
            OnionCell::Destroy { circuit_id, reason } => {
                self.onion_circuits.write().await.remove(&circuit_id);
                tracing::info!("洋葱电路 #{} 已拆除: {}", circuit_id, reason);
            }
        }
    }

    // -------------------------------------------------------------------------
    // 5.2 Kademlia 分布式哈希表 (DHT) 处理
    // -------------------------------------------------------------------------

    /// 处理收到的 Kademlia DHT RPC 消息
    pub async fn handle_dht_message(
        &self,
        msg: DhtMessage,
        src_addr: SocketAddr,
        socket: &UdpSocket,
    ) {
        // 自动更新对端联系人进 K-Bucket
        let sender = match &msg {
            DhtMessage::Ping { sender } => sender.clone(),
            DhtMessage::Pong { sender } => sender.clone(),
            DhtMessage::FindNode { sender, .. } => sender.clone(),
            DhtMessage::FoundNodes { sender, .. } => sender.clone(),
            DhtMessage::Store { sender, .. } => sender.clone(),
            DhtMessage::StoreAck { sender, .. } => sender.clone(),
            DhtMessage::FindValue { sender, .. } => sender.clone(),
            DhtMessage::FoundValue { sender, .. } => sender.clone(),
        };

        {
            let mut routing = self.dht_routing.write().await;
            routing.update(sender.clone());
        }

        let local_contact = self
            .local_contact(vec![socket.local_addr().unwrap_or(src_addr)])
            .await;

        match msg {
            DhtMessage::Ping { .. } => {
                let resp = DhtMessage::Pong {
                    sender: local_contact,
                };
                let datagram = P2pDatagram::dht(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            DhtMessage::Pong { .. } => {
                tracing::debug!(
                    "收到来自 {} (NodeId: {}) 的 DHT Pong 响应",
                    src_addr,
                    sender.id
                );
            }
            DhtMessage::FindNode { target_id, .. } => {
                let closest = {
                    let routing = self.dht_routing.read().await;
                    routing.find_closest_nodes(&target_id, K_BUCKET_SIZE)
                };
                let resp = DhtMessage::FoundNodes {
                    sender: local_contact,
                    closest_nodes: closest,
                };
                let datagram = P2pDatagram::dht(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            DhtMessage::FoundNodes { closest_nodes, .. } => {
                let mut routing = self.dht_routing.write().await;
                for contact in closest_nodes {
                    routing.update(contact);
                }
            }
            DhtMessage::Store {
                key,
                value,
                ttl_secs,
                ..
            } => {
                {
                    let mut storage = self.dht_storage.write().await;
                    storage.store(key, value, ttl_secs);
                }
                let resp = DhtMessage::StoreAck {
                    sender: local_contact,
                    key,
                    success: true,
                };
                let datagram = P2pDatagram::dht(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            DhtMessage::StoreAck { key, success, .. } => {
                tracing::debug!("DHT 存储键值 {:02x?} 确认: {}", &key[..4], success);
            }
            DhtMessage::FindValue { key, .. } => {
                let (val, closer) = {
                    let mut storage = self.dht_storage.write().await;
                    if let Some(v) = storage.get(&key) {
                        (Some(v.to_vec()), Vec::new())
                    } else {
                        let routing = self.dht_routing.read().await;
                        (
                            None,
                            routing.find_closest_nodes(&NodeId(key), K_BUCKET_SIZE),
                        )
                    }
                };
                let resp = DhtMessage::FoundValue {
                    sender: local_contact,
                    key,
                    value: val,
                    closer_nodes: closer,
                };
                let datagram = P2pDatagram::dht(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            DhtMessage::FoundValue {
                key,
                value,
                closer_nodes,
                ..
            } => {
                if let Some(data) = value {
                    let mut storage = self.dht_storage.write().await;
                    storage.store(key, data, 3600);
                } else {
                    let mut routing = self.dht_routing.write().await;
                    for c in closer_nodes {
                        routing.update(c);
                    }
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // 5.3 P2P 内容分块传输与端到端加密消息
    // -------------------------------------------------------------------------

    /// 发布一块本地数据，生成 Blake3 Merkle Root 清单
    pub async fn publish_data(
        &self,
        file_name: &str,
        data: &[u8],
        chunk_size: u32,
    ) -> FileManifest {
        let (manifest, chunks) = FileManifest::create_from_data(file_name, data, chunk_size);
        self.published_files
            .write()
            .await
            .insert(manifest.file_id, (manifest.clone(), chunks));
        tracing::info!(
            "已发布分块文件: '{}' (大小: {} 字节, 分块数: {}, 根哈希: {:02x?}...)",
            file_name,
            data.len(),
            manifest.total_chunks,
            &manifest.file_id[..4]
        );
        manifest
    }

    /// 注册准备接收指定清单的文件
    pub async fn start_receiving_manifest(&self, manifest: FileManifest) {
        let assembler = ChunkAssembler::new(manifest.clone());
        self.active_assemblers
            .write()
            .await
            .insert(manifest.file_id, assembler);
    }

    /// 处理分块传输信令
    pub async fn handle_transfer_message(
        &self,
        msg: TransferMessage,
        src_addr: SocketAddr,
        socket: &UdpSocket,
    ) {
        match msg {
            TransferMessage::ManifestRequest { file_id } => {
                let manifest = {
                    let files = self.published_files.read().await;
                    files.get(&file_id).map(|(m, _)| m.clone())
                };
                let resp = TransferMessage::ManifestResponse { manifest };
                let datagram = P2pDatagram::transfer(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            TransferMessage::ManifestResponse { manifest } => {
                if let Some(m) = manifest {
                    self.start_receiving_manifest(m).await;
                }
            }
            TransferMessage::ChunkRequest {
                file_id,
                chunk_index,
            } => {
                let chunk_data = {
                    let files = self.published_files.read().await;
                    files
                        .get(&file_id)
                        .and_then(|(_, chunks)| chunks.get(chunk_index as usize).cloned())
                };
                let resp = TransferMessage::ChunkResponse {
                    file_id,
                    chunk_index,
                    data: chunk_data,
                };
                let datagram = P2pDatagram::transfer(resp);
                if let Ok(encoded) = codec::encode(&datagram) {
                    let _ = socket.send_to(&encoded, src_addr).await;
                }
            }
            TransferMessage::ChunkResponse {
                file_id,
                chunk_index,
                data,
            } => {
                if let Some(chunk_bytes) = data {
                    let mut assemblers = self.active_assemblers.write().await;
                    if let Some(assembler) = assemblers.get_mut(&file_id) {
                        match assembler.add_chunk(chunk_index as usize, chunk_bytes) {
                            Ok(true) => {
                                tracing::info!(
                                    "文件 {:02x?}... 全部 {} 个分块校验下载完成！",
                                    &file_id[..4],
                                    assembler.manifest().total_chunks
                                );
                            }
                            Ok(false) => {
                                tracing::debug!(
                                    "文件 {:02x?}... 分块 #{} 校验成功 (当前进度: {}/{})",
                                    &file_id[..4],
                                    chunk_index,
                                    assembler.completed_chunks(),
                                    assembler.manifest().total_chunks
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "分块 #{} Blake3 哈希校验失败，已被拒绝: {:#}",
                                    chunk_index,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// 获取已组装完成的文件数据
    pub async fn take_assembled_data(&self, file_id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        let assemblers = self.active_assemblers.write().await;
        if let Some(assembler) = assemblers.get(file_id) {
            if assembler.is_complete() {
                let data = assembler.assemble().context("组装文件分块失败")?;
                return Ok(Some(data));
            }
        }
        Ok(None)
    }

    /// 处理端到端加密直接消息
    pub async fn handle_direct_message(&self, message: P2pDirectMessage) -> Result<Vec<u8>> {
        let identity_opt = self.identity.read().await.clone();
        let Some(local_identity) = identity_opt else {
            anyhow::bail!("本地节点未配置加密身份，无法解密直接消息");
        };

        let plaintext = message
            .open(&local_identity)
            .context("解密端到端消息失败")?;
        tracing::info!(
            "收到来自节点 {} 的端到端加密消息 (长度: {} 字节)",
            message.sender_client_id,
            plaintext.len()
        );
        self.received_messages
            .write()
            .await
            .push((message, plaintext.clone()));
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_overlay_creation_and_dht_storage() {
        let id = Identity::generate().unwrap();
        let overlay = P2pOverlay::from_identity(&id);

        let mut key = [0u8; 32];
        key[0] = 42;
        let val = b"hello darknet DHT".to_vec();

        {
            let mut storage = overlay.dht_storage.write().await;
            storage.store(key, val.clone(), 60);
            assert_eq!(storage.get(&key), Some(val.as_slice()));
        }
    }

    #[tokio::test]
    async fn test_overlay_publish_and_chunk_assembly() {
        let id = Identity::generate().unwrap();
        let overlay = P2pOverlay::from_identity(&id);

        let test_data = b"FuckIPv6 NextGen Mass P2P Infrastructure Chunk Test Data!!".repeat(100);
        let manifest = overlay.publish_data("payload.bin", &test_data, 128).await;

        assert!(manifest.total_chunks > 1);

        // 初始化接收器
        overlay.start_receiving_manifest(manifest.clone()).await;

        // 模拟提取分块并输入接收器
        let files = overlay.published_files.read().await;
        let (_, chunks) = files.get(&manifest.file_id).unwrap();

        for (idx, chunk) in chunks.iter().enumerate() {
            let mut assemblers = overlay.active_assemblers.write().await;
            let assembler = assemblers.get_mut(&manifest.file_id).unwrap();
            let complete = assembler.add_chunk(idx, chunk.clone()).unwrap();
            if idx + 1 == chunks.len() {
                assert!(complete);
            }
        }

        let assembled = overlay
            .take_assembled_data(&manifest.file_id)
            .await
            .unwrap()
            .expect("Should assemble completely");
        assert_eq!(assembled, test_data);
    }
}
