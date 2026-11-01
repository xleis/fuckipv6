//! Direct HTTPS transport for the headless WebUI.
//!
//! TLS configuration is loaded and validated before the listener is exposed.
//! Missing, malformed, mismatched, expired, or origin-incompatible certificate
//! material fails startup instead of falling back to plaintext HTTP.

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use rustls::pki_types::CertificateDer;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use x509_parser::extensions::GeneralName;

pub(super) const ENV_WEBUI_TLS_CERT: &str = "FUCKIPV6_WEBUI_TLS_CERT";
pub(super) const ENV_WEBUI_TLS_KEY: &str = "FUCKIPV6_WEBUI_TLS_KEY";

#[derive(Clone, Debug)]
pub(super) struct WebUiTlsConfig {
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl WebUiTlsConfig {
    pub(super) fn from_process_env() -> Result<Option<Self>> {
        let cert = optional_env(ENV_WEBUI_TLS_CERT)?;
        let key = optional_env(ENV_WEBUI_TLS_KEY)?;
        match (cert, key) {
            (None, None) => Ok(None),
            (Some(_), None) => {
                anyhow::bail!("{ENV_WEBUI_TLS_CERT} 已设置，但 {ENV_WEBUI_TLS_KEY} 未设置")
            }
            (None, Some(_)) => {
                anyhow::bail!("{ENV_WEBUI_TLS_KEY} 已设置，但 {ENV_WEBUI_TLS_CERT} 未设置")
            }
            (Some(cert), Some(key)) => Ok(Some(Self {
                cert_path: PathBuf::from(cert),
                key_path: PathBuf::from(key),
            })),
        }
    }

    pub(super) async fn rustls_config(&self, public_host: &str) -> Result<RustlsConfig> {
        let cert_pem = tokio::fs::read(&self.cert_path)
            .await
            .with_context(|| format!("无法读取 WebUI TLS 证书: {}", self.cert_path.display()))?;
        let key_pem = tokio::fs::read(&self.key_path)
            .await
            .with_context(|| format!("无法读取 WebUI TLS 私钥: {}", self.key_path.display()))?;

        let certs = parse_certificates(&cert_pem)?;
        validate_leaf_certificate(&certs[0], public_host, SystemTime::now())?;
        let key = parse_private_key(&key_pem)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("WebUI TLS 无法启用安全的默认协议版本")?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("WebUI TLS 证书和私钥不匹配或无法用于服务端 TLS")?;
        Ok(RustlsConfig::from_config(Arc::new(config)))
    }
}

fn parse_certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let certs = rustls_pemfile::certs(&mut std::io::Cursor::new(pem))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("WebUI TLS 证书 PEM 无效")?;
    if certs.is_empty() {
        anyhow::bail!("WebUI TLS 证书 PEM 不包含证书");
    }
    Ok(certs)
}

fn parse_private_key(pem: &[u8]) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut std::io::Cursor::new(pem))
        .context("WebUI TLS 私钥 PEM 无效")?
        .ok_or_else(|| anyhow::anyhow!("WebUI TLS 私钥 PEM 不包含受支持的私钥"))
}

fn validate_leaf_certificate(
    cert: &CertificateDer<'_>,
    public_host: &str,
    now: SystemTime,
) -> Result<()> {
    let (remaining, certificate) =
        x509_parser::parse_x509_certificate(cert.as_ref()).context("WebUI TLS 证书无法解析")?;
    if !remaining.is_empty() {
        anyhow::bail!("WebUI TLS 证书包含尾随字节");
    }

    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch，无法验证 WebUI TLS 证书")?
        .as_secs()
        .try_into()
        .context("当前时间超出 WebUI TLS 证书验证范围")?;
    let now = x509_parser::time::ASN1Time::from_timestamp(timestamp)
        .context("当前时间无法转换为证书验证时间")?;
    if !certificate.validity().is_valid_at(now) {
        anyhow::bail!("WebUI TLS 证书当前无效或已过期");
    }

    if let Some(usage) = certificate
        .extended_key_usage()
        .context("WebUI TLS 证书扩展无法解析")?
    {
        if !usage.value.server_auth {
            anyhow::bail!("WebUI TLS 证书未授权用于服务端认证");
        }
    }

    let san = certificate
        .subject_alternative_name()
        .context("WebUI TLS 证书 SAN 扩展无法解析")?
        .ok_or_else(|| anyhow::anyhow!("WebUI TLS 证书缺少 DNS subjectAltName"))?;
    if !san.value.general_names.iter().any(|name| match name {
        GeneralName::DNSName(name) => dns_name_matches(name, public_host),
        _ => false,
    }) {
        anyhow::bail!("WebUI TLS 证书不包含公网 origin 主机 {public_host}");
    }
    Ok(())
}

fn dns_name_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(suffix) => {
            let Some(prefix) = host.strip_suffix(&format!(".{suffix}")) else {
                return false;
            };
            !prefix.is_empty() && !prefix.contains('.')
        }
        None => pattern == host,
    }
}

fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => anyhow::bail!("{name} 不能为空"),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => anyhow::bail!("{name} 必须是有效的 UTF-8"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{generate_simple_self_signed, CertifiedKey};

    fn certified(host: &str) -> CertifiedKey {
        generate_simple_self_signed(vec![host.to_string()]).expect("test certificate")
    }

    #[test]
    fn certificate_must_match_public_host_and_validity_window() {
        let CertifiedKey { cert, .. } = certified("relay.xlei.cc");
        let der = CertificateDer::from(cert.der().to_vec());
        assert!(validate_leaf_certificate(&der, "relay.xlei.cc", SystemTime::now()).is_ok());
        assert!(validate_leaf_certificate(&der, "other.xlei.cc", SystemTime::now()).is_err());
        assert!(validate_leaf_certificate(&der, "relay.xlei.cc", UNIX_EPOCH,).is_err());
    }

    #[test]
    fn wildcard_matches_exactly_one_label() {
        assert!(dns_name_matches("*.xlei.cc", "relay.xlei.cc"));
        assert!(!dns_name_matches("*.xlei.cc", "nested.relay.xlei.cc"));
        assert!(!dns_name_matches("*.xlei.cc", "xlei.cc"));
    }

    #[tokio::test]
    async fn mismatched_private_key_fails_closed() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "fuckipv6-webui-tls-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&directory).await?;
        let first = certified("relay.xlei.cc");
        let second = certified("relay.xlei.cc");
        let cert_path = directory.join("cert.pem");
        let key_path = directory.join("key.pem");
        tokio::fs::write(&cert_path, first.cert.pem()).await?;
        tokio::fs::write(&key_path, second.key_pair.serialize_pem()).await?;
        let config = WebUiTlsConfig {
            cert_path,
            key_path,
        };
        assert!(config.rustls_config("relay.xlei.cc").await.is_err());
        let _ = tokio::fs::remove_dir_all(&directory).await;
        Ok(())
    }
}
