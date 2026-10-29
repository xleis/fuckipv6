use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::crypto::{decrypt_payload, encrypt_payload, Identity};

/// 默认分块大小：64 KB
pub const DEFAULT_CHUNK_SIZE: u32 = 64 * 1024;

/// 文件分块元数据描述清单 (基于 Blake3 分块树哈希)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileManifest {
    /// 文件的全局唯一根哈希 (Blake3 Merkle Root)
    pub file_id: [u8; 32],
    /// 文件名
    pub file_name: String,
    /// 文件总大小 (字节)
    pub total_size: u64,
    /// 单块分块大小 (字节)
    pub chunk_size: u32,
    /// 分块总数
    pub total_chunks: u32,
    /// 所有分块对应的 Blake3 校验哈希列表
    pub chunk_hashes: Vec<[u8; 32]>,
}

impl FileManifest {
    /// 将完整数据切分为指定大小的分块，并生成对应的 FileManifest 与分块列表
    pub fn create_from_data(
        file_name: impl Into<String>,
        data: &[u8],
        chunk_size: u32,
    ) -> (Self, Vec<Vec<u8>>) {
        let chunk_size = if chunk_size == 0 {
            DEFAULT_CHUNK_SIZE
        } else {
            chunk_size
        };

        let mut chunks = Vec::new();
        let mut chunk_hashes = Vec::new();

        if data.is_empty() {
            let hash = blake3::hash(&[]);
            chunk_hashes.push(*hash.as_bytes());
            chunks.push(Vec::new());
        } else {
            for chunk_slice in data.chunks(chunk_size as usize) {
                let hash = blake3::hash(chunk_slice);
                chunk_hashes.push(*hash.as_bytes());
                chunks.push(chunk_slice.to_vec());
            }
        }

        // 计算全文件 Root Hash
        let mut root_hasher = blake3::Hasher::new();
        for h in &chunk_hashes {
            root_hasher.update(h);
        }
        let root_hash = *root_hasher.finalize().as_bytes();

        let manifest = Self {
            file_id: root_hash,
            file_name: file_name.into(),
            total_size: data.len() as u64,
            chunk_size,
            total_chunks: chunks.len() as u32,
            chunk_hashes,
        };

        (manifest, chunks)
    }

    /// 校验指定索引的分块数据是否符合 Blake3 哈希
    pub fn verify_chunk(&self, chunk_index: usize, chunk_data: &[u8]) -> bool {
        if let Some(expected_hash) = self.chunk_hashes.get(chunk_index) {
            let actual = blake3::hash(chunk_data);
            actual.as_bytes() == expected_hash
        } else {
            false
        }
    }
}

/// 多源并行分块装配器（支持乱序拉取与独立 Blake3 校验）
pub struct ChunkAssembler {
    manifest: FileManifest,
    received_chunks: HashMap<usize, Vec<u8>>,
    received_bytes: u64,
}

impl ChunkAssembler {
    pub fn new(manifest: FileManifest) -> Self {
        Self {
            manifest,
            received_chunks: HashMap::new(),
            received_bytes: 0,
        }
    }

    pub fn manifest(&self) -> &FileManifest {
        &self.manifest
    }

    /// 存入接收到的分块数据，自动进行 Blake3 校验
    ///
    /// - 若校验通过：存入缓冲区，若此时所有分块均已就绪则返回 `Ok(true)`；
    /// - 若校验失败：拒绝接受并报错，防止恶意节点投毒。
    pub fn add_chunk(&mut self, chunk_index: usize, data: Vec<u8>) -> Result<bool> {
        if chunk_index >= self.manifest.total_chunks as usize {
            anyhow::bail!(
                "分块索引超出范围: {} >= {}",
                chunk_index,
                self.manifest.total_chunks
            );
        }

        if !self.manifest.verify_chunk(chunk_index, &data) {
            anyhow::bail!(
                "分块 [{}] Blake3 校验失败：数据哈希与清单不符",
                chunk_index
            );
        }

        if let std::collections::hash_map::Entry::Vacant(e) = self.received_chunks.entry(chunk_index) {
            self.received_bytes += data.len() as u64;
            e.insert(data);
        }

        Ok(self.is_complete())
    }

    /// 获取已接收的分块数量
    pub fn completed_chunks(&self) -> usize {
        self.received_chunks.len()
    }

    /// 检查是否已收齐所有分块
    pub fn is_complete(&self) -> bool {
        self.received_chunks.len() == self.manifest.total_chunks as usize
    }

    /// 获取当前尚未下载的分块索引列表（用于多源调度）
    pub fn missing_chunks(&self) -> Vec<usize> {
        let mut missing = Vec::new();
        for i in 0..self.manifest.total_chunks as usize {
            if !self.received_chunks.contains_key(&i) {
                missing.push(i);
            }
        }
        missing
    }

    /// 当前下载进度 (0.0 ~ 1.0)
    pub fn progress(&self) -> f32 {
        if self.manifest.total_chunks == 0 {
            1.0
        } else {
            self.received_chunks.len() as f32 / self.manifest.total_chunks as f32
        }
    }

    /// 拼接完整文件数据
    pub fn assemble(&self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            anyhow::bail!(
                "文件分块未齐备，目前完成进度: {:.1}%",
                self.progress() * 100.0
            );
        }

        let mut output = Vec::with_capacity(self.manifest.total_size as usize);
        for i in 0..self.manifest.total_chunks as usize {
            if let Some(chunk) = self.received_chunks.get(&i) {
                output.extend_from_slice(chunk);
            } else {
                anyhow::bail!("缺失分块: {}", i);
            }
        }

        Ok(output)
    }
}

mod serde_sig64 {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(sig: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(sig)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SigVisitor;
        impl<'de> serde::de::Visitor<'de> for SigVisitor {
            type Value = [u8; 64];

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a 64-byte array")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.len() != 64 {
                    return Err(serde::de::Error::invalid_length(v.len(), &self));
                }
                let mut buf = [0u8; 64];
                buf.copy_from_slice(v);
                Ok(buf)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut buf = [0u8; 64];
                for (i, elem) in buf.iter_mut().enumerate() {
                    *elem = seq
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
                }
                Ok(buf)
            }
        }
        deserializer.deserialize_bytes(SigVisitor)
    }
}

/// 端到端加密的点对点消息 (公钥即地址通信)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct P2pDirectMessage {
    /// 消息随机唯一 ID
    pub message_id: [u8; 16],
    /// 发送端公钥 (Ed25519)
    pub sender_public_key: [u8; 32],
    /// 发送端 ClientId (Base64 blake3)
    pub sender_client_id: String,
    /// 接收端公钥 (Ed25519)
    pub recipient_public_key: [u8; 32],
    /// 发送端单次协商的临时 X25519 公钥 (用于前向安全 ECDH)
    pub ephemeral_public_key: [u8; 32],
    /// 消息发送时间戳 (Unix Epoch 秒)
    pub timestamp: u64,
    /// 发送端对 (密文 + 临时公钥 + 时间戳) 的 Ed25519 签名
    #[serde(with = "serde_sig64")]
    pub signature: [u8; 64],
    /// ChaCha20-Poly1305 加密后的密文数据包 (含 Nonce + 密文 + Tag)
    pub encrypted_payload: Vec<u8>,
}

impl P2pDirectMessage {
    /// 构造并签署一条发送给指定对端的端到端加密消息
    pub fn seal(
        sender_identity: &Identity,
        recipient_public_key: &[u8; 32],
        plaintext: &[u8],
    ) -> Result<Self> {
        // 1. 生成一次性临时 X25519 密钥对
        let mut ephemeral_secret_bytes = [0u8; 32];
        getrandom::fill(&mut ephemeral_secret_bytes)
            .context("生成临时 X25519 密钥失败")?;
        let ephemeral_secret = StaticSecret::from(ephemeral_secret_bytes);
        let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);

        // 2. 将对端 Ed25519 公钥转为 X25519 公钥
        let recipient_x25519_pub = Identity::peer_x25519_public(recipient_public_key)
            .context("对端 Ed25519 公钥转换失败")?;

        // 3. 计算 ECDH 共享秘钥
        let shared = ephemeral_secret.diffie_hellman(&recipient_x25519_pub);
        let session_key = blake3::derive_key(
            "fuckipv6 p2p direct message session key v1",
            shared.as_bytes(),
        );

        // 4. ChaCha20-Poly1305 加密载荷
        let encrypted_payload = encrypt_payload(&session_key, plaintext)
            .context("加密直接消息失败")?;

        let mut message_id = [0u8; 16];
        getrandom::fill(&mut message_id).context("生成消息 ID 失败")?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 5. 签署数据防伪造 (message_id + ephemeral_pub + timestamp + recipient_pub + ciphertext)
        let mut sign_data = Vec::new();
        sign_data.extend_from_slice(&message_id);
        sign_data.extend_from_slice(ephemeral_public.as_bytes());
        sign_data.extend_from_slice(&now.to_be_bytes());
        sign_data.extend_from_slice(recipient_public_key);
        sign_data.extend_from_slice(&encrypted_payload);

        let signature = sender_identity.sign(&sign_data);

        Ok(Self {
            message_id,
            sender_public_key: sender_identity.public_key_bytes(),
            sender_client_id: sender_identity.client_id().to_string(),
            recipient_public_key: *recipient_public_key,
            ephemeral_public_key: *ephemeral_public.as_bytes(),
            timestamp: now,
            signature: signature.to_bytes(),
            encrypted_payload,
        })
    }

    /// 接收端验证签名并解密端到端消息
    pub fn open(&self, recipient_identity: &Identity) -> Result<Vec<u8>> {
        // 1. 验证目标确实是本节点
        if self.recipient_public_key != recipient_identity.public_key_bytes() {
            anyhow::bail!("消息目标公钥与本节点公钥不匹配");
        }

        // 2. 校验发送者 Ed25519 签名
        let mut sign_data = Vec::new();
        sign_data.extend_from_slice(&self.message_id);
        sign_data.extend_from_slice(&self.ephemeral_public_key);
        sign_data.extend_from_slice(&self.timestamp.to_be_bytes());
        sign_data.extend_from_slice(&self.recipient_public_key);
        sign_data.extend_from_slice(&self.encrypted_payload);

        if !Identity::verify(&self.sender_public_key, &sign_data, &self.signature) {
            anyhow::bail!("消息签名验证失败：数据可能已被篡改或伪造");
        }

        // 3. 基于接收端 X25519 私钥与发送端临时公钥推导共享密钥
        let recipient_secret = recipient_identity.x25519_secret();
        let sender_ephemeral_pub = X25519PublicKey::from(self.ephemeral_public_key);
        let shared = recipient_secret.diffie_hellman(&sender_ephemeral_pub);
        let session_key = blake3::derive_key(
            "fuckipv6 p2p direct message session key v1",
            shared.as_bytes(),
        );

        // 4. 解密
        let plaintext = decrypt_payload(&session_key, &self.encrypted_payload)
            .context("解密端到端消息失败")?;

        Ok(plaintext)
    }
}

/// 分块传输信令消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferMessage {
    /// 请求文件清单
    ManifestRequest { file_id: [u8; 32] },
    /// 文件清单响应
    ManifestResponse { manifest: Option<FileManifest> },
    /// 请求指定分块
    ChunkRequest {
        file_id: [u8; 32],
        chunk_index: u32,
    },
    /// 分块数据响应
    ChunkResponse {
        file_id: [u8; 32],
        chunk_index: u32,
        data: Option<Vec<u8>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_manifest_creation_and_assembly() {
        // 创建 150KB 测试数据，以 32KB 分块
        let mut original_data = Vec::with_capacity(150 * 1024);
        for i in 0..(150 * 1024) {
            original_data.push((i % 256) as u8);
        }

        let chunk_size = 32 * 1024;
        let (manifest, chunks) =
            FileManifest::create_from_data("test_firmware.bin", &original_data, chunk_size);

        assert_eq!(manifest.file_name, "test_firmware.bin");
        assert_eq!(manifest.total_size, 150 * 1024);
        assert_eq!(manifest.total_chunks, 5); // 150 / 32 = 4 + 1 = 5
        assert_eq!(chunks.len(), 5);

        // 校验各个分块
        for (i, chunk) in chunks.iter().enumerate() {
            assert!(manifest.verify_chunk(i, chunk));
        }

        // 故意破坏分块数据校验
        let mut corrupted_chunk = chunks[0].clone();
        corrupted_chunk[0] ^= 0xff;
        assert!(!manifest.verify_chunk(0, &corrupted_chunk));

        // 乱序组装测试 (4 -> 1 -> 3 -> 0 -> 2)
        let mut assembler = ChunkAssembler::new(manifest.clone());
        assert_eq!(assembler.missing_chunks().len(), 5);

        let order = [4, 1, 3, 0, 2];
        for (step, &idx) in order.iter().enumerate() {
            let is_complete = assembler.add_chunk(idx, chunks[idx].clone()).unwrap();
            if step < 4 {
                assert!(!is_complete);
            } else {
                assert!(is_complete);
            }
        }

        assert!(assembler.is_complete());
        let assembled_data = assembler.assemble().unwrap();
        assert_eq!(assembled_data, original_data);
    }

    #[test]
    fn test_chunk_assembler_rejects_corrupted_chunk() {
        let (manifest, mut chunks) =
            FileManifest::create_from_data("safe.bin", b"Some verified contents", 1024);
        let mut assembler = ChunkAssembler::new(manifest);

        chunks[0][0] ^= 0x1; // 篡改
        assert!(assembler.add_chunk(0, chunks[0].clone()).is_err());
    }

    #[test]
    fn test_p2p_direct_encrypted_message_flow() {
        let alice = Identity::generate().unwrap();
        let bob = Identity::generate().unwrap();

        let secret_letter = b"Secret rendezvous at 22:00. Bring the keys.";

        // Alice 加密并签署发送给 Bob
        let sealed_msg =
            P2pDirectMessage::seal(&alice, &bob.public_key_bytes(), secret_letter).unwrap();

        assert_eq!(sealed_msg.sender_client_id, alice.client_id());
        assert_eq!(sealed_msg.recipient_public_key, bob.public_key_bytes());

        // Bob 接收并解密
        let decrypted = sealed_msg.open(&bob).unwrap();
        assert_eq!(decrypted, secret_letter);

        // 验证：第三方 Eve 无法解密
        let eve = Identity::generate().unwrap();
        assert!(sealed_msg.open(&eve).is_err());
    }

    #[test]
    fn test_p2p_direct_message_rejects_tampered_signature() {
        let alice = Identity::generate().unwrap();
        let bob = Identity::generate().unwrap();

        let mut sealed_msg =
            P2pDirectMessage::seal(&alice, &bob.public_key_bytes(), b"Test tamper").unwrap();

        // 篡改密文
        let last = sealed_msg.encrypted_payload.len() - 1;
        sealed_msg.encrypted_payload[last] ^= 0x42;

        // Bob 校验签名应失败
        assert!(sealed_msg.open(&bob).is_err());
    }
}
