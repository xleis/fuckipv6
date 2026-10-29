use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// JWT 认证声明
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    /// 主体标识 (通常为用户名或 client id)
    pub sub: String,
    /// 角色 (如 "admin", "viewer")
    pub role: String,
    /// 过期时间戳 (UNIX 秒)
    pub exp: u64,
    /// 签发时间戳 (UNIX 秒)
    pub iat: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
}

impl Default for JwtHeader {
    fn default() -> Self {
        Self {
            alg: "HS256".to_string(),
            typ: "JWT".to_string(),
        }
    }
}

/// JWT 密钥管理器
#[derive(Clone)]
pub struct JwtManager {
    secret: Vec<u8>,
    expiration_secs: u64,
}

impl JwtManager {
    /// 创建 JWT 管理器
    pub fn new(secret: impl Into<Vec<u8>>, expiration_secs: u64) -> Self {
        Self {
            secret: secret.into(),
            expiration_secs,
        }
    }

    /// 签发指定主体和角色的 JWT Token
    pub fn sign(&self, sub: &str, role: &str) -> Result<String> {
        let now = now_secs();
        let claims = Claims {
            sub: sub.to_string(),
            role: role.to_string(),
            exp: now + self.expiration_secs,
            iat: now,
        };

        self.sign_claims(&claims)
    }

    /// 签发完整 Claims
    pub fn sign_claims(&self, claims: &Claims) -> Result<String> {
        let header_json =
            serde_json::to_vec(&JwtHeader::default()).context("序列化 JWT Header 失败")?;
        let claims_json = serde_json::to_vec(claims).context("序列化 JWT Claims 失败")?;

        let header_b64 = base64_url_encode(&header_json);
        let claims_b64 = base64_url_encode(&claims_json);

        let signing_input = format!("{header_b64}.{claims_b64}");
        let signature = hmac_sha256(&self.secret, signing_input.as_bytes());
        let signature_b64 = base64_url_encode(&signature);

        Ok(format!("{signing_input}.{signature_b64}"))
    }

    /// 校验 Token 并返回 Claims
    pub fn verify(&self, token: &str) -> Result<Claims> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            bail!("无效的 JWT 格式，预期 3 段结构");
        }

        let header_b64 = parts[0];
        let claims_b64 = parts[1];
        let signature_b64 = parts[2];

        // 验证签名
        let signing_input = format!("{header_b64}.{claims_b64}");
        let expected_signature = hmac_sha256(&self.secret, signing_input.as_bytes());
        let expected_signature_b64 = base64_url_encode(&expected_signature);

        if !constant_time_eq(signature_b64.as_bytes(), expected_signature_b64.as_bytes()) {
            bail!("JWT 签名校验失败");
        }

        // 解析并验证 Header
        let header_bytes = base64_url_decode(header_b64).context("解码 JWT Header 失败")?;
        let header: JwtHeader =
            serde_json::from_slice(&header_bytes).context("解析 JWT Header JSON 失败")?;
        if header.alg != "HS256" || header.typ != "JWT" {
            bail!("不支持的 JWT 算法或类型");
        }

        // 解析 Claims
        let claims_bytes = base64_url_decode(claims_b64).context("解码 JWT Claims 失败")?;
        let claims: Claims =
            serde_json::from_slice(&claims_bytes).context("解析 JWT Claims JSON 失败")?;

        // 校验过期时间
        let now = now_secs();
        if claims.exp < now {
            bail!("JWT Token 已过期 (exp: {}, now: {})", claims.exp, now);
        }

        Ok(claims)
    }
}

/// 计算当前 UNIX 时间戳（秒）
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// RFC 2104 标准 HMAC-SHA256
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hash = Sha256::digest(key);
        k[..32].copy_from_slice(&hash);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().into()
}

/// 常量时间比较，防止时序攻击
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// URL 安全且无 Padding 的 Base64 编码
pub fn base64_url_encode(input: &[u8]) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity((input.len() * 4).div_ceil(3));

    let (chunks, remainder) = input.as_chunks::<3>();

    for chunk in chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        output.push(CHARSET[((n >> 18) & 0x3F) as usize] as char);
        output.push(CHARSET[((n >> 12) & 0x3F) as usize] as char);
        output.push(CHARSET[((n >> 6) & 0x3F) as usize] as char);
        output.push(CHARSET[(n & 0x3F) as usize] as char);
    }

    match remainder.len() {
        1 => {
            let n = (remainder[0] as u32) << 16;
            output.push(CHARSET[((n >> 18) & 0x3F) as usize] as char);
            output.push(CHARSET[((n >> 12) & 0x3F) as usize] as char);
        }
        2 => {
            let n = ((remainder[0] as u32) << 16) | ((remainder[1] as u32) << 8);
            output.push(CHARSET[((n >> 18) & 0x3F) as usize] as char);
            output.push(CHARSET[((n >> 12) & 0x3F) as usize] as char);
            output.push(CHARSET[((n >> 6) & 0x3F) as usize] as char);
        }
        _ => {}
    }

    output
}

/// URL 安全且无 Padding 的 Base64 解码
pub fn base64_url_decode(input: &str) -> Result<Vec<u8>> {
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut output = Vec::with_capacity((input.len() * 3) / 4);

    for &byte in input.as_bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' => continue,
            _ => bail!("无效的 Base64 字符: {}", byte as char),
        };

        buffer = (buffer << 6) | (val as u32);
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_url_roundtrip() -> Result<()> {
        let cases = vec![
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
            vec![0, 255, 128, 64, 32, 16],
        ];

        for case in cases {
            let encoded = base64_url_encode(&case);
            assert!(!encoded.contains('+'));
            assert!(!encoded.contains('/'));
            assert!(!encoded.contains('='));
            let decoded = base64_url_decode(&encoded)?;
            assert_eq!(decoded, case);
        }
        Ok(())
    }

    #[test]
    fn test_jwt_sign_and_verify_success() -> Result<()> {
        let manager = JwtManager::new(b"test-secret-key-12345", 3600);
        let token = manager.sign("admin", "admin")?;
        let claims = manager.verify(&token)?;

        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.role, "admin");
        assert!(claims.exp > claims.iat);
        Ok(())
    }

    #[test]
    fn test_jwt_rejects_tampered_payload() -> Result<()> {
        let manager = JwtManager::new(b"test-secret-key-12345", 3600);
        let token = manager.sign("alice", "viewer")?;

        let mut parts: Vec<&str> = token.split('.').collect();
        // 篡改 payload
        let fake_claims = Claims {
            sub: "alice".to_string(),
            role: "admin".to_string(), // 越权
            exp: now_secs() + 3600,
            iat: now_secs(),
        };
        let fake_claims_b64 = base64_url_encode(&serde_json::to_vec(&fake_claims)?);
        parts[1] = &fake_claims_b64;
        let tampered_token = parts.join(".");

        let result = manager.verify(&tampered_token);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("签名校验失败"));
        Ok(())
    }

    #[test]
    fn test_jwt_rejects_expired_token() -> Result<()> {
        let manager = JwtManager::new(b"test-secret-key-12345", 10);
        let past_claims = Claims {
            sub: "admin".to_string(),
            role: "admin".to_string(),
            exp: now_secs() - 100, // 已经过期
            iat: now_secs() - 200,
        };
        let token = manager.sign_claims(&past_claims)?;

        let result = manager.verify(&token);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("已过期"));
        Ok(())
    }

    #[test]
    fn test_jwt_rejects_wrong_secret() -> Result<()> {
        let manager_a = JwtManager::new(b"secret-A", 3600);
        let manager_b = JwtManager::new(b"secret-B", 3600);

        let token = manager_a.sign("admin", "admin")?;
        let result = manager_b.verify(&token);
        assert!(result.is_err());
        Ok(())
    }
}
