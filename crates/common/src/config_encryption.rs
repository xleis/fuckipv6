use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, KeyInit};
use serde::{Deserialize, Serialize};
use std::path::Path;

const FORMAT_VERSION: u8 = 1;
const KDF: &str = "argon2id";
const CIPHER: &str = "chacha20poly1305";
const SALT_LEN: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedConfig {
    version: u8,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

pub fn encrypt_config_bytes(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    validate_passphrase(passphrase)?;

    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).context("生成配置加密 salt 失败")?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("加密配置文件失败"))?;

    let envelope = EncryptedConfig {
        version: FORMAT_VERSION,
        kdf: KDF.to_string(),
        cipher: CIPHER.to_string(),
        salt: encode_b64(&salt),
        nonce: encode_b64(&nonce),
        ciphertext: encode_b64(&ciphertext),
    };

    serde_json::to_vec_pretty(&envelope).context("序列化加密配置失败")
}

pub fn decrypt_config_bytes(encrypted: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    validate_passphrase(passphrase)?;

    let envelope: EncryptedConfig =
        serde_json::from_slice(encrypted).context("解析加密配置文件失败")?;

    if envelope.version != FORMAT_VERSION {
        anyhow::bail!("不支持的加密配置版本: {}", envelope.version);
    }
    if envelope.kdf != KDF {
        anyhow::bail!("不支持的配置 KDF: {}", envelope.kdf);
    }
    if envelope.cipher != CIPHER {
        anyhow::bail!("不支持的配置加密算法: {}", envelope.cipher);
    }

    let salt = decode_b64(&envelope.salt).context("解析 salt 失败")?;
    let nonce = decode_b64(&envelope.nonce).context("解析 nonce 失败")?;
    let ciphertext = decode_b64(&envelope.ciphertext).context("解析 ciphertext 失败")?;

    if salt.len() != SALT_LEN {
        anyhow::bail!("加密配置 salt 长度无效");
    }

    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("解密配置文件失败"))
}

pub fn encrypt_config_file(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    passphrase: &str,
) -> Result<()> {
    let plaintext = std::fs::read(input.as_ref())
        .with_context(|| format!("读取配置文件失败: {}", input.as_ref().display()))?;
    let encrypted = encrypt_config_bytes(&plaintext, passphrase)?;
    std::fs::write(output.as_ref(), encrypted)
        .with_context(|| format!("写入加密配置失败: {}", output.as_ref().display()))?;
    Ok(())
}

pub fn decrypt_config_file(input: impl AsRef<Path>, passphrase: &str) -> Result<Vec<u8>> {
    let encrypted = std::fs::read(input.as_ref())
        .with_context(|| format!("读取加密配置失败: {}", input.as_ref().display()))?;
    decrypt_config_bytes(&encrypted, passphrase)
}

pub fn is_encrypted_config_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("enc"))
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("创建 Argon2 参数失败: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("派生配置加密密钥失败: {}", e))?;
    Ok(key)
}

fn validate_passphrase(passphrase: &str) -> Result<()> {
    if passphrase.is_empty() {
        anyhow::bail!("配置加密口令不能为空");
    }
    Ok(())
}

fn encode_b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn decode_b64(data: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .context("base64 解码失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypts_and_decrypts_config_bytes() -> anyhow::Result<()> {
        let plaintext = br#"coordinator_url = "ws://localhost:6666""#;
        let encrypted = encrypt_config_bytes(plaintext, "secret")?;

        assert_ne!(encrypted, plaintext);
        let decrypted = decrypt_config_bytes(&encrypted, "secret")?;
        assert_eq!(decrypted, plaintext);
        Ok(())
    }

    #[test]
    fn rejects_wrong_passphrase() -> anyhow::Result<()> {
        let encrypted = encrypt_config_bytes(b"hello", "secret")?;
        assert!(decrypt_config_bytes(&encrypted, "wrong").is_err());
        Ok(())
    }

    #[test]
    fn detects_encrypted_config_paths() {
        assert!(is_encrypted_config_path("client.toml.enc"));
        assert!(!is_encrypted_config_path("client.toml"));
    }
}
