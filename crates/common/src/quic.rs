use anyhow::{Context, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    PKCS_ED25519,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::pki_types::{ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DistinguishedName as RustlsDistinguishedName, SignatureScheme};
use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use x509_parser::oid_registry::OID_SIG_ED25519;
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::Identity;
use crate::{ClientId, ErrorCode};

pub const FUCKIPV6_QUIC_ALPN: &[u8] = b"fuckipv6-quic-v1";
pub const QUIC_RELAY_CHANNEL_BINDING_LABEL: &[u8] = b"fuckipv6-quic-client-cert-binding-v1";
pub const QUIC_RELAY_CHANNEL_BINDING_CONTEXT: &[u8] = b"relay-register";
pub const MAX_QUIC_FRAME_SIZE: usize = 64 * 1024;

/// 从当前 TLS 1.3 会话导出 relay 注册使用的 32 字节 channel binding。
///
/// 客户端与服务端必须调用同一 helper，并使用相同的版本化 label/context。
/// exporter 结果随 TLS 会话变化，不能作为静态协议常量或跨连接复用。
pub fn export_quic_relay_channel_binding(connection: &quinn::Connection) -> Result<[u8; 32]> {
    let mut binding = [0_u8; 32];
    connection
        .export_keying_material(
            &mut binding,
            QUIC_RELAY_CHANNEL_BINDING_LABEL,
            QUIC_RELAY_CHANNEL_BINDING_CONTEXT,
        )
        .map_err(|err| {
            anyhow::anyhow!("无法从 QUIC TLS 会话导出 relay channel binding: {err:?}")
        })?;
    Ok(binding)
}

/// 校验当前 QUIC TLS 终端证书与 Relay Register 声明的身份完全相同。
///
/// 调用方必须在写入任何 relay 状态之前执行。TLS 层已通过 mandatory client
/// auth 验证证书用途和私钥持有证明；本函数把证书 SPKI、Register 公钥和
/// Register 中签名的证书 pin 绑定在一起。
pub fn verify_quic_client_identity_binding(
    connection: &quinn::Connection,
    registration: &RelayRegistrationBinding<'_>,
) -> Result<()> {
    let peer_identity = connection
        .peer_identity()
        .context("QUIC 中继连接缺少客户端证书身份")?;
    let certificates = peer_identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| anyhow::anyhow!("QUIC 客户端证书身份类型无效"))?;
    if certificates.len() != 1 {
        anyhow::bail!(
            "QUIC 客户端证书链必须恰好包含一个终端证书，实际为 {}",
            certificates.len()
        );
    }

    let certificate = &certificates[0];
    let tls_public_key = ed25519_public_key_from_certificate(certificate)?;
    if tls_public_key.as_slice() != registration.public_key {
        anyhow::bail!("QUIC 客户端证书公钥与 Relay Register 应用身份不一致");
    }

    let tls_public_key_pin = public_key_pin_from_certificate(certificate)?;
    if tls_public_key_pin != registration.client_certificate_public_key_pin {
        anyhow::bail!("QUIC 客户端证书 SPKI pin 与 Relay Register 声明不一致");
    }

    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
pub struct QuicCertificate {
    pub cert_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    pub public_key_pin: [u8; 32],
}

impl std::fmt::Debug for QuicCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicCertificate")
            .field("cert_der_len", &self.cert_der.len())
            .field("private_key_der", &"[REDACTED]")
            .field("public_key_pin", &self.public_key_pin)
            .finish()
    }
}

impl Drop for QuicCertificate {
    fn drop(&mut self) {
        self.private_key_der.zeroize();
    }
}

impl QuicCertificate {
    /// 使用长期 Ed25519 身份构造客户端证书。
    ///
    /// 证书与 TLS CertificateVerify 使用同一个长期私钥。证书仅证明客户端持有
    /// 该身份私钥；应用层身份仍需在 Relay Register 中通过 channel binding 绑定。
    pub fn from_identity(identity: &Identity) -> Result<Self> {
        let mut seed = identity.signing_key_seed();
        let private_key_der = Zeroizing::new(ed25519_pkcs8_from_seed(&seed));
        seed.zeroize();

        let key_pair = KeyPair::from_pkcs8_der_and_sign_algo(
            &PrivatePkcs8KeyDer::from(private_key_der.as_slice()),
            &PKCS_ED25519,
        )
        .context("无法从 Ed25519 身份构造 QUIC 客户端密钥")?;

        let mut params = CertificateParams::new(vec![format!(
            "{}.client.fuckipv6.local",
            identity.client_id()
        )])
        .context("无法创建 QUIC 客户端证书参数")?;
        params.distinguished_name = client_distinguished_name(identity.client_id());
        params.is_ca = IsCa::NoCa;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let cert = params
            .self_signed(&key_pair)
            .context("无法签发 QUIC 客户端身份证书")?;

        let cert_der = cert.der().to_vec();
        let private_key_der = key_pair.serialize_der();
        let public_key_pin = public_key_pin_from_spki(&key_pair.public_key_der());

        Ok(Self {
            cert_der,
            private_key_der,
            public_key_pin,
        })
    }

    pub fn certificate_der(&self) -> CertificateDer<'static> {
        CertificateDer::from(self.cert_der.clone())
    }

    pub fn private_key_der(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.private_key_der.clone()).into()
    }
}

pub fn generate_self_signed_quic_certificate(
    subject_alt_names: Vec<String>,
) -> Result<QuicCertificate> {
    let subject_alt_names = if subject_alt_names.is_empty() {
        vec!["fuckipv6.local".to_string()]
    } else {
        subject_alt_names
    };

    let key_pair =
        KeyPair::generate_for(&PKCS_ED25519).context("无法生成 Ed25519 QUIC 证书密钥")?;
    let mut params =
        CertificateParams::new(subject_alt_names).context("无法创建 QUIC 自签名证书参数")?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "fuckipv6.local");
    params.is_ca = IsCa::NoCa;
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let cert = params
        .self_signed(&key_pair)
        .context("无法生成 QUIC 自签名证书")?;

    let cert_der = cert.der().to_vec();
    let private_key_der = key_pair.serialize_der();
    let public_key_pin = public_key_pin_from_spki(&key_pair.public_key_der());

    Ok(QuicCertificate {
        cert_der,
        private_key_der,
        public_key_pin,
    })
}

fn client_distinguished_name(client_id: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, format!("fuckipv6-client-{client_id}"));
    name
}

fn ed25519_pkcs8_from_seed(seed: &[u8; 32]) -> Vec<u8> {
    // RFC 8410 PKCS#8 v1 wrapper for a raw Ed25519 private seed.
    let mut der = Vec::with_capacity(48);
    der.extend_from_slice(&[0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06]);
    der.extend_from_slice(&[0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
    der.extend_from_slice(seed);
    der
}

pub fn public_key_pin_from_certificate(cert: &CertificateDer<'_>) -> Result<[u8; 32]> {
    let (remaining, certificate) =
        x509_parser::parse_x509_certificate(cert.as_ref()).context("无法解析 QUIC 证书")?;
    if !remaining.is_empty() {
        anyhow::bail!("QUIC 证书包含尾随字节");
    }
    Ok(public_key_pin_from_spki(certificate.public_key().raw))
}

/// 从证书中提取 Ed25519 原始公钥，并拒绝其他公钥算法。
pub fn ed25519_public_key_from_certificate(cert: &CertificateDer<'_>) -> Result<[u8; 32]> {
    let (remaining, certificate) =
        x509_parser::parse_x509_certificate(cert.as_ref()).context("无法解析 QUIC 证书")?;
    if !remaining.is_empty() {
        anyhow::bail!("QUIC 证书包含尾随字节");
    }
    if certificate.public_key().algorithm.algorithm != OID_SIG_ED25519 {
        anyhow::bail!("QUIC 证书不是 Ed25519 公钥");
    }
    certificate
        .public_key()
        .subject_public_key
        .data
        .as_ref()
        .try_into()
        .context("QUIC 证书中的 Ed25519 公钥长度无效")
}

pub fn public_key_pin_from_spki(spki_der: &[u8]) -> [u8; 32] {
    *blake3::hash(spki_der).as_bytes()
}

#[derive(Debug)]
pub struct PinnedServerVerifier {
    expected_pin: [u8; 32],
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedServerVerifier {
    pub fn new(expected_pin: [u8; 32]) -> Arc<Self> {
        Arc::new(Self {
            expected_pin,
            crypto_provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let pin = public_key_pin_from_certificate(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;

        if pin == self.expected_pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct Ed25519ClientCertVerifier {
    root_hint_subjects: Vec<RustlsDistinguishedName>,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl Ed25519ClientCertVerifier {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            root_hint_subjects: Vec::new(),
            crypto_provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }

    fn verify_client_certificate(
        &self,
        cert: &CertificateDer<'_>,
        now: UnixTime,
    ) -> std::result::Result<(), rustls::Error> {
        let (remaining, certificate) =
            x509_parser::parse_x509_certificate(cert.as_ref()).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        if !remaining.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        if certificate.public_key().algorithm.algorithm != OID_SIG_ED25519 {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        let now =
            x509_parser::time::ASN1Time::from_timestamp(now.as_secs() as i64).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        if !certificate.validity().is_valid_at(now) {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired,
            ));
        }
        if certificate.issuer().as_raw() != certificate.subject().as_raw() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        let has_client_auth = certificate
            .extended_key_usage()
            .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))?
            .is_some_and(|usage| usage.value.client_auth);
        if !has_client_auth {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(())
    }
}

impl ClientCertVerifier for Ed25519ClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[RustlsDistinguishedName] {
        &self.root_hint_subjects
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        self.verify_client_certificate(end_entity, now)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// 构建强制客户端证书认证的 QUIC 服务端配置。
pub fn build_quic_server_config(certificate: &QuicCertificate) -> Result<quinn::ServerConfig> {
    build_quic_server_config_with_client_auth(certificate)
}

pub fn build_quic_server_config_with_client_auth(
    certificate: &QuicCertificate,
) -> Result<quinn::ServerConfig> {
    let mut server_crypto = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 QUIC 服务端 TLS 协议版本配置")?
    .with_client_cert_verifier(Ed25519ClientCertVerifier::new())
    .with_single_cert(
        vec![certificate.certificate_der()],
        certificate.private_key_der(),
    )
    .context("无法创建 QUIC 服务端证书配置")?;
    server_crypto.alpn_protocols = vec![FUCKIPV6_QUIC_ALPN.to_vec()];
    server_crypto.max_early_data_size = u32::MAX;

    let mut server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));
    let transport =
        Arc::get_mut(&mut server_config.transport).context("无法获取 QUIC 服务端传输配置")?;
    let idle_timeout = quinn::IdleTimeout::try_from(std::time::Duration::from_secs(120))
        .map_err(|e| anyhow::anyhow!("QUIC 空闲超时配置非法: {e}"))?;
    transport.max_concurrent_uni_streams(0_u8.into());
    transport.max_idle_timeout(Some(idle_timeout));
    Ok(server_config)
}

/// 仅供显式 legacy/负向测试使用的无客户端认证配置。
#[doc(hidden)]
pub fn build_quic_server_config_without_client_auth(
    certificate: &QuicCertificate,
) -> Result<quinn::ServerConfig> {
    let mut server_crypto = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 legacy QUIC 服务端 TLS 协议版本配置")?
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.certificate_der()],
        certificate.private_key_der(),
    )
    .context("无法创建 legacy QUIC 服务端证书配置")?;
    server_crypto.alpn_protocols = vec![FUCKIPV6_QUIC_ALPN.to_vec()];
    server_crypto.max_early_data_size = 0;

    let mut server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));
    let transport = Arc::get_mut(&mut server_config.transport)
        .context("无法获取 legacy QUIC 服务端传输配置")?;
    let idle_timeout = quinn::IdleTimeout::try_from(std::time::Duration::from_secs(120))
        .map_err(|e| anyhow::anyhow!("QUIC 空闲超时配置非法: {e}"))?;
    transport.max_concurrent_uni_streams(0_u8.into());
    transport.max_idle_timeout(Some(idle_timeout));
    Ok(server_config)
}

/// 构建使用指定客户端证书并 pin 服务端公钥的 QUIC 客户端配置。
pub fn build_pinned_quic_client_config_with_auth(
    expected_server_pin: [u8; 32],
    client_certificate: &QuicCertificate,
) -> Result<quinn::ClientConfig> {
    let mut client_crypto = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 QUIC 客户端 TLS 协议版本配置")?
    .dangerous()
    .with_custom_certificate_verifier(PinnedServerVerifier::new(expected_server_pin))
    .with_client_auth_cert(
        vec![client_certificate.certificate_der()],
        client_certificate.private_key_der(),
    )
    .context("无法配置 QUIC 客户端身份证书")?;
    client_crypto.alpn_protocols = vec![FUCKIPV6_QUIC_ALPN.to_vec()];
    client_crypto.enable_early_data = true;
    finish_pinned_client_config(client_crypto)
}

/// 仅供显式 legacy/自检路径使用的无客户端证书配置。
#[doc(hidden)]
pub fn build_pinned_quic_client_config(
    expected_server_pin: [u8; 32],
) -> Result<quinn::ClientConfig> {
    let mut client_crypto = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 legacy QUIC 客户端 TLS 协议版本配置")?
    .dangerous()
    .with_custom_certificate_verifier(PinnedServerVerifier::new(expected_server_pin))
    .with_no_client_auth();
    client_crypto.alpn_protocols = vec![FUCKIPV6_QUIC_ALPN.to_vec()];
    client_crypto.enable_early_data = true;
    finish_pinned_client_config(client_crypto)
}

fn finish_pinned_client_config(client_crypto: rustls::ClientConfig) -> Result<quinn::ClientConfig> {
    let mut client_config =
        quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    let idle_timeout = quinn::IdleTimeout::try_from(std::time::Duration::from_secs(120))
        .map_err(|e| anyhow::anyhow!("QUIC 客户端空闲超时配置非法: {e}"))?;
    transport.max_idle_timeout(Some(idle_timeout));
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicFrame {
    pub stream_id: u64,
    pub payload: Vec<u8>,
}

pub async fn send_quic_frame(stream: &mut quinn::SendStream, frame: &QuicFrame) -> Result<()> {
    if frame.payload.len() > MAX_QUIC_FRAME_SIZE {
        anyhow::bail!(
            "QUIC frame payload too large: {} > {}",
            frame.payload.len(),
            MAX_QUIC_FRAME_SIZE
        );
    }

    stream
        .write_u64(frame.stream_id)
        .await
        .context("无法写入 QUIC frame stream_id")?;
    stream
        .write_u32(frame.payload.len() as u32)
        .await
        .context("无法写入 QUIC frame 长度")?;
    stream
        .write_all(&frame.payload)
        .await
        .context("无法写入 QUIC frame payload")?;
    stream.finish().context("无法结束 QUIC send stream")?;
    Ok(())
}

pub async fn recv_quic_frame(stream: &mut quinn::RecvStream) -> Result<QuicFrame> {
    let stream_id = stream
        .read_u64()
        .await
        .context("无法读取 QUIC frame stream_id")?;
    let payload_len = stream
        .read_u32()
        .await
        .context("无法读取 QUIC frame 长度")? as usize;
    if payload_len > MAX_QUIC_FRAME_SIZE {
        anyhow::bail!(
            "QUIC frame payload too large: {} > {}",
            payload_len,
            MAX_QUIC_FRAME_SIZE
        );
    }

    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .await
        .context("无法读取 QUIC frame payload")?;

    Ok(QuicFrame { stream_id, payload })
}

/// relay 注册签名覆盖的完整身份与会话绑定字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayRegistrationBinding<'a> {
    pub client_id: &'a str,
    pub virtual_ip: Ipv6Addr,
    pub public_key: &'a [u8],
    pub timestamp: u64,
    pub server_public_key_pin: [u8; 32],
    pub client_certificate_public_key_pin: [u8; 32],
    pub tls_channel_binding: [u8; 32],
}

impl RelayRegistrationBinding<'_> {
    /// 生成版本化、无字段拼接歧义的注册签名载荷。
    pub fn challenge(&self) -> Vec<u8> {
        let mut challenge = Vec::with_capacity(
            self.client_id.len() + self.public_key.len() + 1 + 8 + 8 + 16 + 8 + 32 * 3,
        );
        challenge.extend_from_slice(b"fuckipv6-relay-register-v2");
        append_len_prefixed(&mut challenge, self.client_id.as_bytes());
        challenge.extend_from_slice(&self.virtual_ip.octets());
        append_len_prefixed(&mut challenge, self.public_key);
        challenge.extend_from_slice(&self.timestamp.to_be_bytes());
        challenge.extend_from_slice(&self.server_public_key_pin);
        challenge.extend_from_slice(&self.client_certificate_public_key_pin);
        challenge.extend_from_slice(&self.tls_channel_binding);
        challenge
    }
}

fn append_len_prefixed(buffer: &mut Vec<u8>, field: &[u8]) {
    buffer.extend_from_slice(&(field.len() as u64).to_be_bytes());
    buffer.extend_from_slice(field);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayMessage {
    Register {
        client_id: ClientId,
        virtual_ip: Ipv6Addr,
        public_key: Vec<u8>,
        timestamp: u64,
        server_public_key_pin: [u8; 32],
        client_certificate_public_key_pin: [u8; 32],
        tls_channel_binding: [u8; 32],
        signature: Vec<u8>,
    },
    RegisterAck {
        accepted: bool,
        message: String,
    },
    Packet {
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        #[serde(with = "crate::protocol::serde_bytes")]
        payload: Vec<u8>,
    },
    Error {
        code: u16,
        message: String,
    },
}

impl RelayMessage {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.as_u16(),
            message: message.into(),
        }
    }

    pub fn default_error(code: ErrorCode) -> Self {
        Self::error(code, code.default_message())
    }
}

pub fn encode_relay_message(message: &RelayMessage) -> Result<Vec<u8>> {
    crate::codec::encode(message).context("无法序列化 QUIC relay 消息")
}

pub fn decode_relay_message(payload: &[u8]) -> Result<RelayMessage> {
    crate::codec::decode(payload).context("无法反序列化 QUIC relay 消息")
}

pub async fn send_relay_message(
    stream: &mut quinn::SendStream,
    stream_id: u64,
    message: &RelayMessage,
) -> Result<()> {
    let payload = encode_relay_message(message).context("无法编码 QUIC relay 消息")?;
    send_quic_frame(stream, &QuicFrame { stream_id, payload })
        .await
        .context("无法发送 QUIC relay frame")
}

pub async fn recv_relay_message(stream: &mut quinn::RecvStream) -> Result<(u64, RelayMessage)> {
    let frame = recv_quic_frame(stream)
        .await
        .context("无法读取 QUIC relay frame")?;
    let message = decode_relay_message(&frame.payload).context("无法解码 QUIC relay frame")?;
    Ok((frame.stream_id, message))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroRttSelfTestReport {
    pub server_addr: SocketAddr,
    pub accepted: bool,
    pub echoed_payload: Vec<u8>,
}

pub async fn run_quic_zero_rtt_reconnect_self_test(
    certificate: &QuicCertificate,
) -> Result<ZeroRttSelfTestReport> {
    let server_config = build_quic_server_config(certificate)?;
    let server = quinn::Endpoint::server(
        server_config,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    )
    .context("无法启动 QUIC 0-RTT loopback 服务端")?;
    let server_addr = server
        .local_addr()
        .context("无法读取 QUIC 0-RTT loopback 服务端地址")?;
    let (server_payload_tx, mut server_payload_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_task = tokio::spawn(run_zero_rtt_test_server(server, server_payload_tx));

    let client_identity = Identity::generate()?;
    let client_certificate = QuicCertificate::from_identity(&client_identity)?;
    let mut client = quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .context("无法启动 QUIC 0-RTT loopback 客户端")?;
    client.set_default_client_config(build_pinned_quic_client_config_with_auth(
        certificate.public_key_pin,
        &client_certificate,
    )?);

    let first_connection = client
        .connect(server_addr, "fuckipv6.local")
        .context("无法发起 QUIC 0-RTT 首次连接")?
        .into_0rtt()
        .err()
        .ok_or_else(|| anyhow::anyhow!("QUIC 0-RTT unexpectedly available before warmup"))?
        .await
        .context("QUIC 0-RTT 首次握手失败")?;
    let mut half_rtt_marker = first_connection
        .accept_uni()
        .await
        .context("QUIC 0-RTT 首次连接未收到 0.5-RTT 标记")?;
    let marker = half_rtt_marker
        .read_to_end(usize::MAX)
        .await
        .context("QUIC 0-RTT 首次连接读取 0.5-RTT 标记失败")?;
    if marker != b"half-rtt" {
        anyhow::bail!("QUIC 0-RTT half-rtt marker mismatch");
    }
    let mut established_marker = first_connection
        .accept_uni()
        .await
        .context("QUIC 0-RTT 首次连接未收到握手完成标记")?;
    let marker = established_marker
        .read_to_end(usize::MAX)
        .await
        .context("QUIC 0-RTT 首次连接读取握手完成标记失败")?;
    if marker != b"established" {
        anyhow::bail!("QUIC 0-RTT handshake marker mismatch");
    }
    first_connection.close(0u32.into(), b"warmup-done");
    client.wait_idle().await;

    let connecting = client
        .connect(server_addr, "fuckipv6.local")
        .context("无法发起 QUIC 0-RTT 重连")?;
    let (connection, zero_rtt) = connecting
        .into_0rtt()
        .map_err(|_| anyhow::anyhow!("QUIC 0-RTT session ticket unavailable"))?;
    let (mut send, _recv) = connection
        .open_bi()
        .await
        .context("QUIC 0-RTT 重连无法打开双向流")?;
    let frame = QuicFrame {
        stream_id: 2,
        payload: b"zero-rtt".to_vec(),
    };
    send_quic_frame(&mut send, &frame).await?;
    let accepted = zero_rtt.await;
    let echoed_payload =
        tokio::time::timeout(std::time::Duration::from_secs(2), server_payload_rx.recv())
            .await
            .context("等待 QUIC 0-RTT 服务端接收 payload 超时")?
            .ok_or_else(|| anyhow::anyhow!("QUIC 0-RTT 服务端接收通道已关闭"))?;

    connection.close(0u32.into(), b"done");
    client.wait_idle().await;
    server_task.abort();

    Ok(ZeroRttSelfTestReport {
        server_addr,
        accepted,
        echoed_payload,
    })
}

async fn run_zero_rtt_test_server(
    server: quinn::Endpoint,
    server_payload_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    while let Some(incoming) = server.accept().await {
        let server_payload_tx = server_payload_tx.clone();
        tokio::spawn(async move {
            let Ok(connecting) = incoming.accept() else {
                return;
            };
            let Ok((connection, established)) = connecting.into_0rtt() else {
                return;
            };
            let reader_connection = connection.clone();
            let reader_task = tokio::spawn(async move {
                if let Ok((_send, mut recv)) = reader_connection.accept_bi().await {
                    if let Ok(frame) = recv_quic_frame(&mut recv).await {
                        let _ = server_payload_tx.send(frame.payload);
                    }
                }
            });

            if let Ok(mut stream) = connection.open_uni().await {
                let _ = stream.write_all(b"half-rtt").await;
                let _ = stream.finish();
            }
            let _ = established.await;
            if let Ok(mut stream) = connection.open_uni().await {
                let _ = stream.write_all(b"established").await;
                let _ = stream.finish();
            }

            if let Ok(Ok(())) =
                tokio::time::timeout(std::time::Duration::from_millis(500), reader_task).await
            {
                if let Ok(mut stream) = connection.open_uni().await {
                    let _ = stream.write_all(b"received").await;
                    let _ = stream.finish();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identity;

    #[test]
    fn generates_self_signed_certificate_and_stable_public_key_pin() -> anyhow::Result<()> {
        let cert = generate_self_signed_quic_certificate(vec!["localhost".to_string()])?;
        assert!(!cert.cert_der.is_empty());
        assert!(!cert.private_key_der.is_empty());

        let parsed_pin = public_key_pin_from_certificate(&cert.certificate_der())?;
        assert_eq!(parsed_pin, cert.public_key_pin);
        Ok(())
    }

    #[test]
    fn identity_certificate_matches_application_ed25519_key() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let certificate = QuicCertificate::from_identity(&identity)?;
        let cert_der = certificate.certificate_der();

        assert_eq!(
            ed25519_public_key_from_certificate(&cert_der)?,
            identity.public_key().as_slice()
        );
        assert_eq!(
            public_key_pin_from_certificate(&cert_der)?,
            certificate.public_key_pin
        );

        let (remaining, parsed) = x509_parser::parse_x509_certificate(cert_der.as_ref())?;
        assert!(remaining.is_empty());
        let usage = parsed
            .extended_key_usage()?
            .context("identity certificate is missing extended key usage")?;
        assert!(usage.value.client_auth);
        assert!(!usage.value.server_auth);
        assert!(format!("{certificate:?}").contains("[REDACTED]"));
        assert!(!format!("{certificate:?}").contains(&format!("{:?}", certificate.private_key_der)));
        Ok(())
    }

    #[tokio::test]
    async fn mandatory_client_auth_accepts_identity_certificate() -> anyhow::Result<()> {
        let server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let identity = Identity::generate()?;
        let client_certificate = QuicCertificate::from_identity(&identity)?;
        let server = quinn::Endpoint::server(
            build_quic_server_config(&server_certificate)?,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;
        let expected_public_key = identity.public_key();
        let server_task = tokio::spawn(async move {
            let incoming = server
                .accept()
                .await
                .context("server did not accept client")?;
            let connection = incoming.await.context("server handshake failed")?;
            let peer_identity = connection
                .peer_identity()
                .context("server did not receive a client identity")?;
            let certificates = peer_identity
                .downcast::<Vec<CertificateDer<'static>>>()
                .map_err(|_| anyhow::anyhow!("unexpected client identity type"))?;
            let client_certificate = certificates
                .first()
                .context("server received no client certificate")?;
            assert_eq!(
                ed25519_public_key_from_certificate(client_certificate)?,
                expected_public_key.as_slice()
            );
            Ok::<(), anyhow::Error>(())
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_pinned_quic_client_config_with_auth(
            server_certificate.public_key_pin,
            &client_certificate,
        )?);
        let connection = client
            .connect(server_addr, "fuckipv6.local")?
            .await
            .context("client handshake failed")?;
        connection.close(0u32.into(), b"done");
        client.wait_idle().await;
        server_task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn authenticated_endpoints_derive_same_relay_channel_binding() -> anyhow::Result<()> {
        let server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let client_certificate = QuicCertificate::from_identity(&Identity::generate()?)?;
        let server = quinn::Endpoint::server(
            build_quic_server_config(&server_certificate)?,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;
        let server_task = tokio::spawn(async move {
            let incoming = server
                .accept()
                .await
                .context("server did not accept client")?;
            let connection = incoming.await.context("server handshake failed")?;
            export_quic_relay_channel_binding(&connection)
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_pinned_quic_client_config_with_auth(
            server_certificate.public_key_pin,
            &client_certificate,
        )?);
        let connection = client
            .connect(server_addr, "fuckipv6.local")?
            .await
            .context("client handshake failed")?;
        let client_binding = export_quic_relay_channel_binding(&connection)?;

        connection.close(0u32.into(), b"done");
        client.wait_idle().await;
        let server_binding = server_task.await??;
        assert_eq!(client_binding, server_binding);
        Ok(())
    }

    #[tokio::test]
    async fn mandatory_client_auth_rejects_missing_client_certificate() -> anyhow::Result<()> {
        let server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let server = quinn::Endpoint::server(
            build_quic_server_config(&server_certificate)?,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;
        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("server did not accept client");
            incoming.await.map(|_| ())
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_pinned_quic_client_config(
            server_certificate.public_key_pin,
        )?);
        let connecting = client.connect(server_addr, "fuckipv6.local")?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .context("client handshake unexpectedly timed out")?;
        if let Ok(connection) = result {
            let _close_reason =
                tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed())
                    .await
                    .context("connection without client auth remained open")?;
        }
        let server_result = server_task.await?;
        assert!(server_result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn mandatory_client_auth_rejects_non_client_auth_certificate() -> anyhow::Result<()> {
        let server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let invalid_client_certificate = certificate_without_client_auth()?;
        let server = quinn::Endpoint::server(
            build_quic_server_config(&server_certificate)?,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;
        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("server did not accept client");
            incoming.await.map(|_| ())
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_pinned_quic_client_config_with_auth(
            server_certificate.public_key_pin,
            &invalid_client_certificate,
        )?);
        let connecting = client.connect(server_addr, "fuckipv6.local")?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .context("client handshake unexpectedly timed out")?;
        if let Ok(connection) = result {
            let _close_reason =
                tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed())
                    .await
                    .context("connection with non-client-auth certificate remained open")?;
        }
        let server_result = server_task.await?;
        assert!(server_result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn authenticated_client_rejects_wrong_server_pin() -> anyhow::Result<()> {
        let server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let wrong_server_certificate =
            generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let client_certificate = QuicCertificate::from_identity(&Identity::generate()?)?;
        let server = quinn::Endpoint::server(
            build_quic_server_config(&server_certificate)?,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )?;
        let server_addr = server.local_addr()?;
        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("server did not accept client");
            incoming.await.map(|_| ())
        });

        let mut client =
            quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
        client.set_default_client_config(build_pinned_quic_client_config_with_auth(
            wrong_server_certificate.public_key_pin,
            &client_certificate,
        )?);
        let connecting = client.connect(server_addr, "fuckipv6.local")?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .context("client handshake unexpectedly timed out")?;
        assert!(result.is_err(), "wrong server pin must fail the handshake");
        let server_result = server_task.await?;
        assert!(server_result.is_err());
        Ok(())
    }

    fn certificate_without_client_auth() -> anyhow::Result<QuicCertificate> {
        let identity = Identity::generate()?;
        let mut seed = identity.signing_key_seed();
        let private_key_der = Zeroizing::new(ed25519_pkcs8_from_seed(&seed));
        seed.zeroize();
        let key_pair = KeyPair::from_pkcs8_der_and_sign_algo(
            &PrivatePkcs8KeyDer::from(private_key_der.as_slice()),
            &PKCS_ED25519,
        )?;
        let mut params = CertificateParams::new(vec!["invalid.client.fuckipv6.local".to_string()])?;
        params.distinguished_name = client_distinguished_name(identity.client_id());
        params.is_ca = IsCa::NoCa;
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.self_signed(&key_pair)?;
        Ok(QuicCertificate {
            cert_der: cert.der().to_vec(),
            private_key_der: key_pair.serialize_der(),
            public_key_pin: public_key_pin_from_spki(&key_pair.public_key_der()),
        })
    }

    #[test]
    fn relay_register_challenge_covers_identity_and_tls_binding() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let public_key = identity.public_key();
        let virtual_ip = "fd00::1001".parse()?;
        let binding = RelayRegistrationBinding {
            client_id: identity.client_id(),
            virtual_ip,
            public_key: &public_key,
            timestamp: 123,
            server_public_key_pin: [1; 32],
            client_certificate_public_key_pin: [2; 32],
            tls_channel_binding: [3; 32],
        };
        let challenge = binding.challenge();
        let signature = identity.sign(&challenge);
        assert!(Identity::verify(
            &public_key,
            &challenge,
            &signature.to_bytes()
        ));

        let other_public_key = Identity::generate()?.public_key();
        let other_virtual_ip = "fd00::1002".parse()?;
        let variants = [
            RelayRegistrationBinding {
                client_id: "different-client",
                ..binding
            },
            RelayRegistrationBinding {
                virtual_ip: other_virtual_ip,
                ..binding
            },
            RelayRegistrationBinding {
                public_key: &other_public_key,
                ..binding
            },
            RelayRegistrationBinding {
                timestamp: 124,
                ..binding
            },
            RelayRegistrationBinding {
                server_public_key_pin: [4; 32],
                ..binding
            },
            RelayRegistrationBinding {
                client_certificate_public_key_pin: [5; 32],
                ..binding
            },
            RelayRegistrationBinding {
                tls_channel_binding: [6; 32],
                ..binding
            },
        ];
        for variant in variants {
            assert_ne!(variant.challenge(), challenge);
        }
        Ok(())
    }

    #[tokio::test]
    async fn zero_rtt_reconnect_self_test_uses_resumed_session() -> anyhow::Result<()> {
        let cert = generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])?;
        let report = run_quic_zero_rtt_reconnect_self_test(&cert).await?;
        assert!(report.accepted);
        assert_eq!(report.echoed_payload, b"zero-rtt");
        Ok(())
    }

    #[test]
    fn relay_error_uses_named_error_codes() {
        let error = RelayMessage::default_error(ErrorCode::Overloaded);
        match error {
            RelayMessage::Error { code, message } => {
                assert_eq!(code, 503);
                assert_eq!(message, "Service overloaded");
            }
            other => panic!("unexpected relay message: {:?}", other),
        }
    }
}
