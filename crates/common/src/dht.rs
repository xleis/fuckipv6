use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

pub const K_BUCKET_SIZE: usize = 20;
pub const ID_BITS: usize = 256;
pub const ID_BYTES: usize = 32;

/// Kademlia 256-bit 节点标识符 (基于公钥 Blake3 哈希生成)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct NodeId(pub [u8; ID_BYTES]);

impl NodeId {
    pub fn new(bytes: [u8; ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// 从公钥字节通过 Blake3 生成 NodeId
    pub fn from_public_key(pk: &[u8]) -> Self {
        let hash = blake3::hash(pk);
        Self(*hash.as_bytes())
    }

    /// 计算与另一节点的异或度量距离 (XOR Distance)
    pub fn distance(&self, other: &NodeId) -> Distance {
        let mut result = [0u8; ID_BYTES];
        for (i, byte) in result.iter_mut().enumerate() {
            *byte = self.0[i] ^ other.0[i];
        }
        Distance(result)
    }

    /// 计算两节点的最长公共前缀比特长度 (Common Prefix Length: 0..256)
    pub fn common_prefix_len(&self, other: &NodeId) -> usize {
        let dist = self.distance(other);
        let mut count = 0;
        for &byte in &dist.0 {
            if byte == 0 {
                count += 8;
            } else {
                count += byte.leading_zeros() as usize;
                break;
            }
        }
        count
    }

    /// 转换为 16 进制字符串
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in &self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({}..{})", &self.to_hex()[..8], &self.to_hex()[56..])
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// XOR 距离度量（大端序高位在前，自然字典序与数值距离严格一致）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Distance(pub [u8; ID_BYTES]);

impl fmt::Debug for Distance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut hex = String::with_capacity(64);
        for byte in &self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        write!(f, "Distance({}..{})", &hex[..8], &hex[56..])
    }
}

/// Kademlia 节点联系人信息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DhtContact {
    pub id: NodeId,
    pub virtual_ip: IpAddr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<SocketAddr>,
    pub last_seen: u64,
}

impl DhtContact {
    pub fn new(id: NodeId, virtual_ip: IpAddr, endpoints: Vec<SocketAddr>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            virtual_ip,
            endpoints,
            last_seen: now,
        }
    }

    pub fn touch(&mut self) {
        self.last_seen = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
}

/// 单个 K-Bucket 桶（容量为 K=20，按最近活跃时间排序）
#[derive(Debug, Clone)]
pub struct KBucket {
    contacts: VecDeque<DhtContact>,
    replacement_cache: VecDeque<DhtContact>,
    capacity: usize,
}

impl Default for KBucket {
    fn default() -> Self {
        Self::new(K_BUCKET_SIZE)
    }
}

impl KBucket {
    pub fn new(capacity: usize) -> Self {
        Self {
            contacts: VecDeque::with_capacity(capacity),
            replacement_cache: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// 更新节点在桶中的位置（LRU 策略）：
    /// - 若已存在：移至末尾（最新活跃）
    /// - 若不存在且未满：插入末尾
    /// - 若不存在且已满：暂存至替补缓存 (replacement cache)
    pub fn update(&mut self, mut contact: DhtContact) -> bool {
        contact.touch();

        if let Some(pos) = self.contacts.iter().position(|c| c.id == contact.id) {
            self.contacts.remove(pos);
            self.contacts.push_back(contact);
            return true;
        }

        if self.contacts.len() < self.capacity {
            self.contacts.push_back(contact);
            true
        } else {
            // 桶已满，放入替补缓存
            if let Some(pos) = self.replacement_cache.iter().position(|c| c.id == contact.id) {
                self.replacement_cache.remove(pos);
            }
            if self.replacement_cache.len() >= self.capacity {
                self.replacement_cache.pop_front();
            }
            self.replacement_cache.push_back(contact);
            false
        }
    }

    /// 移除某个节点
    pub fn remove(&mut self, id: &NodeId) -> Option<DhtContact> {
        if let Some(pos) = self.contacts.iter().position(|c| &c.id == id) {
            let removed = self.contacts.remove(pos);
            // 从替补缓存晋升一个候选节点
            if let Some(replacement) = self.replacement_cache.pop_front() {
                self.contacts.push_back(replacement);
            }
            removed
        } else {
            if let Some(pos) = self.replacement_cache.iter().position(|c| &c.id == id) {
                self.replacement_cache.remove(pos);
            }
            None
        }
    }

    pub fn contacts(&self) -> &VecDeque<DhtContact> {
        &self.contacts
    }

    pub fn len(&self) -> usize {
        self.contacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contacts.is_empty()
    }
}

/// Kademlia 路由表（包含 256 个 K-Bucket，按与本地 NodeId 的公共前缀长度分桶）
#[derive(Debug, Clone)]
pub struct DhtRoutingTable {
    local_id: NodeId,
    buckets: Vec<KBucket>,
}

impl DhtRoutingTable {
    pub fn new(local_id: NodeId) -> Self {
        let mut buckets = Vec::with_capacity(ID_BITS);
        for _ in 0..ID_BITS {
            buckets.push(KBucket::default());
        }
        Self { local_id, buckets }
    }

    pub fn local_id(&self) -> &NodeId {
        &self.local_id
    }

    /// 计算对应分桶的索引 (0..255)
    pub fn bucket_index(&self, id: &NodeId) -> usize {
        let cpl = self.local_id.common_prefix_len(id);
        if cpl >= ID_BITS {
            ID_BITS - 1
        } else {
            cpl
        }
    }

    /// 将对端节点插入或更新到对应分桶
    pub fn update(&mut self, contact: DhtContact) -> bool {
        if contact.id == self.local_id {
            return false;
        }
        let idx = self.bucket_index(&contact.id);
        self.buckets[idx].update(contact)
    }

    /// 移除失效节点
    pub fn remove(&mut self, id: &NodeId) -> Option<DhtContact> {
        if id == &self.local_id {
            return None;
        }
        let idx = self.bucket_index(id);
        self.buckets[idx].remove(id)
    }

    /// 查询距离目标 target 最近的 count 个活跃联系人（按 XOR 距离升序排列）
    pub fn find_closest_nodes(&self, target: &NodeId, count: usize) -> Vec<DhtContact> {
        let mut all_contacts = Vec::new();
        for bucket in &self.buckets {
            for contact in bucket.contacts() {
                all_contacts.push(contact.clone());
            }
        }

        all_contacts.sort_by(|a, b| {
            let dist_a = a.id.distance(target);
            let dist_b = b.id.distance(target);
            dist_a.cmp(&dist_b)
        });

        all_contacts.truncate(count);
        all_contacts
    }

    /// 获取路由表内存储的所有节点数量
    pub fn total_contacts(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }
}

/// Kademlia 分布式键值存储（用于 Peer 信息缓存与无中心服务发现）
#[derive(Debug, Clone, Default)]
pub struct DhtStorage {
    values: HashMap<[u8; 32], (Vec<u8>, u64)>, // key -> (value, expire_timestamp)
}

impl DhtStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入键值，指定存活时间 ttl_secs（0 表示永久）
    pub fn store(&mut self, key: [u8; 32], value: Vec<u8>, ttl_secs: u64) {
        let expire = if ttl_secs == 0 {
            u64::MAX
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now.saturating_add(ttl_secs)
        };
        self.values.insert(key, (value, expire));
    }

    /// 获取指定键的值（若已过期则自动清除并返回 None）
    pub fn get(&mut self, key: &[u8; 32]) -> Option<&[u8]> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let is_expired = match self.values.get(key) {
            Some((_, expire)) => now > *expire,
            None => return None,
        };

        if is_expired {
            self.values.remove(key);
            None
        } else {
            self.values.get(key).map(|(val, _)| val.as_slice())
        }
    }

    /// 清理已过期的记录
    pub fn prune_expired(&mut self) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let before = self.values.len();
        self.values.retain(|_, (_, expire)| now <= *expire);
        before - self.values.len()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Kademlia P2P 协议信元消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DhtMessage {
    /// 存活探测请求
    Ping { sender: DhtContact },
    /// 存活探测响应
    Pong { sender: DhtContact },
    /// 寻节点请求：查找距离 target_id 最近的联系人
    FindNode {
        sender: DhtContact,
        target_id: NodeId,
    },
    /// 寻节点响应：返回最近的节点列表
    FoundNodes {
        sender: DhtContact,
        closest_nodes: Vec<DhtContact>,
    },
    /// 存储键值请求
    Store {
        sender: DhtContact,
        key: [u8; 32],
        value: Vec<u8>,
        ttl_secs: u64,
    },
    /// 存储键值确认
    StoreAck {
        sender: DhtContact,
        key: [u8; 32],
        success: bool,
    },
    /// 查询键值请求
    FindValue {
        sender: DhtContact,
        key: [u8; 32],
    },
    /// 查询键值响应：若本节点拥有此值则返回 value，否则返回更近的联系人引导发起者迭代查询
    FoundValue {
        sender: DhtContact,
        key: [u8; 32],
        value: Option<Vec<u8>>,
        closer_nodes: Vec<DhtContact>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_xor_metric_and_distance() {
        let mut id1_bytes = [0u8; 32];
        let mut id2_bytes = [0u8; 32];
        id1_bytes[0] = 0b1000_0000;
        id2_bytes[0] = 0b1100_0000;

        let id1 = NodeId::new(id1_bytes);
        let id2 = NodeId::new(id2_bytes);

        let dist = id1.distance(&id2);
        assert_eq!(dist.0[0], 0b0100_0000);

        // 公共前缀应为 1 bit
        assert_eq!(id1.common_prefix_len(&id2), 1);

        // 自我距离为 0
        let zero_dist = id1.distance(&id1);
        assert_eq!(zero_dist.0, [0u8; 32]);
        assert_eq!(id1.common_prefix_len(&id1), 256);
    }

    #[test]
    fn test_kbucket_lru_and_capacity() {
        let mut bucket = KBucket::new(3);

        let c1 = DhtContact::new(NodeId::new([1; 32]), "10.0.0.1".parse().unwrap(), vec![]);
        let c2 = DhtContact::new(NodeId::new([2; 32]), "10.0.0.2".parse().unwrap(), vec![]);
        let c3 = DhtContact::new(NodeId::new([3; 32]), "10.0.0.3".parse().unwrap(), vec![]);
        let c4 = DhtContact::new(NodeId::new([4; 32]), "10.0.0.4".parse().unwrap(), vec![]);

        assert!(bucket.update(c1.clone()));
        assert!(bucket.update(c2.clone()));
        assert!(bucket.update(c3.clone()));
        assert_eq!(bucket.len(), 3);

        // 插入第 4 个，桶满进入替补缓存
        assert!(!bucket.update(c4.clone()));
        assert_eq!(bucket.len(), 3);

        // 移除 c2，c4 应当自动晋升
        let removed = bucket.remove(&c2.id);
        assert_eq!(removed.unwrap().id, c2.id);
        assert_eq!(bucket.len(), 3);
        assert!(bucket.contacts().iter().any(|c| c.id == c4.id));
    }

    #[test]
    fn test_routing_table_closest_nodes_lookup() {
        let local = NodeId::new([0u8; 32]);
        let mut table = DhtRoutingTable::new(local);

        // 插入 5 个不同距离的节点
        for i in 1..=5 {
            let mut id_bytes = [0u8; 32];
            id_bytes[31] = i;
            let contact = DhtContact::new(
                NodeId::new(id_bytes),
                format!("10.0.0.{i}").parse().unwrap(),
                vec![],
            );
            table.update(contact);
        }

        assert_eq!(table.total_contacts(), 5);

        // 查找距离 [0..0, 3] 最近的 2 个节点
        let mut target_bytes = [0u8; 32];
        target_bytes[31] = 3;
        let target = NodeId::new(target_bytes);

        let closest = table.find_closest_nodes(&target, 2);
        assert_eq!(closest.len(), 2);
        // 最近的应当是 target 自身 (i=3, 距离=0)
        assert_eq!(closest[0].id, target);
    }

    #[test]
    fn test_dht_storage_store_get_and_expiration() {
        let mut storage = DhtStorage::new();
        let key = [42u8; 32];
        let val = b"hello kademlia dht".to_vec();

        // 存储永久数据
        storage.store(key, val.clone(), 0);
        assert_eq!(storage.get(&key), Some(val.as_slice()));

        // 存储即将过期数据 (ttl = 1 秒)
        let key_exp = [99u8; 32];
        storage.store(key_exp, b"temporary".to_vec(), 1);
        assert_eq!(storage.get(&key_exp), Some(b"temporary".as_slice()));
    }

    #[test]
    fn test_dht_message_codec() {
        let contact = DhtContact::new(
            NodeId::new([7u8; 32]),
            "fd00::123".parse().unwrap(),
            vec!["1.2.3.4:6666".parse().unwrap()],
        );
        let msg = DhtMessage::FindNode {
            sender: contact,
            target_id: NodeId::new([88u8; 32]),
        };

        let encoded = crate::codec::encode(&msg).unwrap();
        let decoded: DhtMessage = crate::codec::decode(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }
}
