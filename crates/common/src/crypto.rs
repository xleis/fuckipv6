use anyhow::{Context, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha512};
use std::path::Path;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

/// 客户端身份
#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    client_id: String,
}

impl Identity {
    /// 生成新的身份
    pub fn generate() -> Result<Self> {
        let mut secret_bytes = [0u8; 32];
        getrandom::fill(&mut secret_bytes)
            .context("Failed to generate Ed25519 private key seed")?;
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key = signing_key.verifying_key();

        // client_id = base64(blake3(public_key))
        let hash = blake3::hash(verifying_key.as_bytes());
        let client_id = base64_encode(hash.as_bytes());

        Ok(Self {
            signing_key,
            verifying_key,
            client_id,
        })
    }

    /// 从文件加载身份
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read(path.as_ref())
            .with_context(|| format!("Failed to read key file: {:?}", path.as_ref()))?;

        if content.len() != 64 {
            anyhow::bail!("Invalid key file length");
        }

        let secret_bytes: [u8; 32] = content[..32]
            .try_into()
            .context("Invalid Ed25519 secret key bytes")?;
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key = signing_key.verifying_key();

        let hash = blake3::hash(verifying_key.as_bytes());
        let client_id = base64_encode(hash.as_bytes());

        Ok(Self {
            signing_key,
            verifying_key,
            client_id,
        })
    }

    /// 保存身份到文件
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(&self.signing_key.to_bytes());
        bytes.extend_from_slice(self.verifying_key.as_bytes());

        std::fs::write(path.as_ref(), bytes)
            .with_context(|| format!("Failed to write key file: {:?}", path.as_ref()))?;

        Ok(())
    }

    /// 如果文件不存在则生成并保存，否则加载
    pub fn load_or_generate(path: impl AsRef<Path>) -> Result<Self> {
        if path.as_ref().exists() {
            Self::load_from_file(path)
        } else {
            let identity = Self::generate().context("Failed to generate client identity")?;
            identity.save_to_file(&path).with_context(|| {
                format!("Failed to save generated key file: {:?}", path.as_ref())
            })?;
            Ok(identity)
        }
    }

    /// 获取客户端ID
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// 获取公钥字节 (32字节数组)
    pub fn public_key_bytes(&self) -> [u8; 32] {
        *self.verifying_key.as_bytes()
    }

    /// 获取公钥字节
    pub fn public_key(&self) -> Vec<u8> {
        self.verifying_key.as_bytes().to_vec()
    }

    /// 返回长期 Ed25519 签名种子的受控副本，仅供同 crate 的可信适配层使用。
    ///
    /// 调用方必须在使用后立即清零返回的缓冲区，不得持久化或记录该值。
    pub(crate) fn signing_key_seed(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// 构造与当前长期身份等价的 Secure Session Ed25519 身份。
    ///
    /// 该方法只返回由 Secure Session 类型封装的身份句柄，不暴露种子或
    /// 签名私钥字节。两条路径使用相同的 Ed25519 种子，因此客户端注册、
    /// 旧版验签和 v2 握手看到的是同一个长期公钥。
    pub fn secure_session_identity(&self) -> fuckipv6_secure_session::handshake::Ed25519Identity {
        fuckipv6_secure_session::handshake::Ed25519Identity::from_seed(self.signing_key.to_bytes())
    }

    /// 签名消息
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    /// 验证签名
    pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
        if public_key.len() != 32 || signature.len() != 64 {
            return false;
        }

        let pk_bytes: [u8; 32] = match public_key.try_into() {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };

        let sig_bytes: [u8; 64] = match signature.try_into() {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };

        if let Ok(pk) = VerifyingKey::from_bytes(&pk_bytes) {
            let sig = Signature::from_bytes(&sig_bytes);
            return pk.verify(message, &sig).is_ok();
        }
        false
    }

    /// 计算公钥指纹 (Base64 编码的 BLAKE3 哈希)
    pub fn public_key_fingerprint(public_key: &[u8]) -> String {
        let hash = blake3::hash(public_key);
        base64_encode(hash.as_bytes())
    }

    /// 验证对端的公钥指纹
    pub fn verify_peer_fingerprint(public_key: &[u8], expected_fingerprint: &str) -> bool {
        Self::public_key_fingerprint(public_key) == expected_fingerprint
    }

    /// 获取与本 Ed25519 私钥等价的 X25519 静态私钥（RFC 8032 / RFC 7748 双射转换）
    pub fn x25519_secret(&self) -> StaticSecret {
        let seed = self.signing_key.to_bytes();
        let hash = Sha512::digest(seed);
        let mut secret_bytes = [0u8; 32];
        secret_bytes.copy_from_slice(&hash[..32]);
        secret_bytes[0] &= 248;
        secret_bytes[31] &= 127;
        secret_bytes[31] |= 64;
        StaticSecret::from(secret_bytes)
    }

    /// 获取与本 Ed25519 公钥等价的 X25519 公钥
    pub fn x25519_public(&self) -> X25519PublicKey {
        let montgomery = self.verifying_key.to_montgomery();
        X25519PublicKey::from(montgomery.to_bytes())
    }

    /// 将对端 Ed25519 公钥转换为 X25519 公钥
    pub fn peer_x25519_public(peer_ed25519_public: &[u8]) -> Result<X25519PublicKey> {
        let pk_bytes: [u8; 32] = peer_ed25519_public
            .try_into()
            .context("对端 Ed25519 公钥长度无效，应为 32 字节")?;
        let verifying_key =
            VerifyingKey::from_bytes(&pk_bytes).context("对端 Ed25519 公钥非有效椭圆曲线点")?;
        let montgomery = verifying_key.to_montgomery();
        Ok(X25519PublicKey::from(montgomery.to_bytes()))
    }

    /// 基于 X25519 Diffie-Hellman 和 Blake3 KDF 计算与对端的对称会话秘钥
    pub fn derive_shared_secret(&self, peer_ed25519_public: &[u8]) -> Result<[u8; 32]> {
        let peer_public = Self::peer_x25519_public(peer_ed25519_public)?;
        let static_secret = self.x25519_secret();
        let shared = static_secret.diffie_hellman(&peer_public);
        let session_key =
            blake3::derive_key("fuckipv6 data plane session key v1", shared.as_bytes());
        Ok(session_key)
    }
}

/// 使用 ChaCha20-Poly1305 AEAD 对数据包进行端到端加密（12字节随机 Nonce + 密文 + 16字节 Tag）
pub fn encrypt_payload(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).context("生成 AEAD 随机 Nonce 失败")?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("数据面 payload 加密失败: {e}"))?;
    let mut output = Vec::with_capacity(12 + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// 使用 ChaCha20-Poly1305 AEAD 对数据包进行端到端解密与完整性认证
pub fn decrypt_payload(key: &[u8; 32], payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() < 12 + 16 {
        anyhow::bail!(
            "Payload 长度 {} 字节过短，无法包含 12 字节 Nonce 和 16 字节 Poly1305 认证标签",
            payload.len()
        );
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("数据面 payload 解密或认证标签校验失败: {e}"))?;
    Ok(plaintext)
}

/// Base64编码 (URL安全，无填充)
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_generate() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        assert!(!identity.client_id().is_empty());
        assert_eq!(identity.public_key().len(), 32);
        Ok(())
    }

    #[test]
    fn test_identity_sign_verify() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let message = b"test message";

        let signature = identity.sign(message);
        assert!(Identity::verify(
            &identity.public_key(),
            message,
            &signature.to_bytes()
        ));
        Ok(())
    }

    #[test]
    fn secure_session_identity_uses_the_same_long_term_key() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let secure_identity = identity.secure_session_identity();
        let message = b"same long-term identity";

        assert_eq!(
            secure_identity.verifying_key().as_bytes(),
            &identity.public_key_bytes()
        );
        assert!(Identity::verify(
            identity.public_key().as_slice(),
            message,
            &secure_identity.sign(message).to_bytes(),
        ));
        Ok(())
    }

    #[test]
    fn test_identity_save_load() -> anyhow::Result<()> {
        let temp_file = std::env::temp_dir().join("test_key.pem");

        let identity1 = Identity::generate()?;
        identity1.save_to_file(&temp_file)?;

        let identity2 = Identity::load_from_file(&temp_file)?;
        assert_eq!(identity1.client_id(), identity2.client_id());

        std::fs::remove_file(temp_file).ok();
        Ok(())
    }

    #[test]
    fn test_verify_fingerprint() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let hash = blake3::hash(&identity.public_key());
        let fingerprint = base64_encode(hash.as_bytes());

        assert!(Identity::verify_peer_fingerprint(
            &identity.public_key(),
            &fingerprint
        ));
        Ok(())
    }

    #[test]
    fn test_diffie_hellman_shared_secret() -> anyhow::Result<()> {
        let alice = Identity::generate()?;
        let bob = Identity::generate()?;

        let alice_shared = alice.derive_shared_secret(&bob.public_key())?;
        let bob_shared = bob.derive_shared_secret(&alice.public_key())?;

        // 验证 Alice 与 Bob 通过非对称秘钥协商出的对称秘钥完全一致
        assert_eq!(alice_shared, bob_shared);
        assert_ne!(alice_shared, [0u8; 32]);
        Ok(())
    }

    #[test]
    fn test_aead_encrypt_decrypt_roundtrip() -> anyhow::Result<()> {
        let alice = Identity::generate()?;
        let bob = Identity::generate()?;
        let session_key = alice.derive_shared_secret(&bob.public_key())?;

        let sample_packet = b"GET /api/status HTTP/1.1\r\nHost: fd00::1002\r\n\r\n";
        let encrypted = encrypt_payload(&session_key, sample_packet)?;

        // 密文必须包含 12 字节 Nonce + 原文 + 16 字节 Poly1305 Tag
        assert_eq!(encrypted.len(), 12 + sample_packet.len() + 16);
        assert_ne!(&encrypted[12..12 + sample_packet.len()], sample_packet);

        let decrypted = decrypt_payload(&session_key, &encrypted)?;
        assert_eq!(decrypted, sample_packet);

        // 验证防篡改：修改密文任意一个字节必须触发校验失败
        let mut tampered = encrypted.clone();
        tampered[15] ^= 0x55;
        assert!(decrypt_payload(&session_key, &tampered).is_err());

        Ok(())
    }
}
