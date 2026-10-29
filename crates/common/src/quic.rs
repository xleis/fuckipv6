use anyhow::{Context, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::pki_types::{ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{ClientId, ErrorCode};

pub const FUCKIPV6_QUIC_ALPN: &[u8] = b"fuckipv6-quic-v1";
pub const MAX_QUIC_FRAME_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicCertificate {
    pub cert_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    pub public_key_pin: [u8; 32],
}

impl QuicCertificate {
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

    let certified_key = rcgen::generate_simple_self_signed(subject_alt_names)
        .context("无法生成 QUIC 自签名证书")?;
    let cert_der = certified_key.cert.der().to_vec();
    let private_key_der = certified_key.key_pair.serialize_der();
    let public_key_pin = public_key_pin_from_spki(&certified_key.key_pair.public_key_der());

    Ok(QuicCertificate {
        cert_der,
        private_key_der,
        public_key_pin,
    })
}

pub fn public_key_pin_from_certificate(cert: &CertificateDer<'_>) -> Result<[u8; 32]> {
    let (_remaining, certificate) =
        x509_parser::parse_x509_certificate(cert.as_ref()).context("无法解析 QUIC 证书")?;
    Ok(public_key_pin_from_spki(certificate.public_key().raw))
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

pub fn build_quic_server_config(certificate: &QuicCertificate) -> Result<quinn::ServerConfig> {
    let mut server_crypto = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 QUIC 服务端 TLS 协议版本配置")?
    .with_no_client_auth()
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

pub fn build_pinned_quic_client_config(
    expected_server_pin: [u8; 32],
) -> Result<quinn::ClientConfig> {
    let mut client_crypto = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("无法创建 QUIC 客户端 TLS 协议版本配置")?
    .dangerous()
    .with_custom_certificate_verifier(PinnedServerVerifier::new(expected_server_pin))
    .with_no_client_auth();
    client_crypto.alpn_protocols = vec![FUCKIPV6_QUIC_ALPN.to_vec()];
    client_crypto.enable_early_data = true;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayMessage {
    Register {
        client_id: ClientId,
        virtual_ip: Ipv6Addr,
        public_key: Vec<u8>,
        timestamp: u64,
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

    pub fn register_challenge(
        client_id: &str,
        virtual_ip: &Ipv6Addr,
        public_key: &[u8],
        timestamp: u64,
    ) -> Vec<u8> {
        let mut challenge = Vec::with_capacity(client_id.len() + public_key.len() + 64);
        challenge.extend_from_slice(b"fuckipv6-relay-register-v1");
        challenge.extend_from_slice(client_id.as_bytes());
        challenge.extend_from_slice(&virtual_ip.octets());
        challenge.extend_from_slice(public_key);
        challenge.extend_from_slice(&timestamp.to_be_bytes());
        challenge
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

    let mut client = quinn::Endpoint::client(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .context("无法启动 QUIC 0-RTT loopback 客户端")?;
    client.set_default_client_config(build_pinned_quic_client_config(certificate.public_key_pin)?);

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
    fn relay_register_challenge_is_signed_and_verifiable() -> anyhow::Result<()> {
        let identity = Identity::generate()?;
        let virtual_ip = "fd00::1001".parse()?;
        let timestamp = 123;
        let challenge = RelayMessage::register_challenge(
            identity.client_id(),
            &virtual_ip,
            &identity.public_key(),
            timestamp,
        );
        let signature = identity.sign(&challenge);

        assert!(Identity::verify(
            &identity.public_key(),
            &challenge,
            &signature.to_bytes()
        ));
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
