use anyhow::{Context, Result};
use fuckipv6_common::{Identity, NetworkTicket, RelayEndpoint};
use fuckipv6_coordinator::{
    relay::server::RelayServer, signaling::server::SignalingServer,
    signaling::session::SessionManager,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// 内嵌协调端 Actor
///
/// 赋予普通客户端节点完整的中心协调与中继服务能力，
/// 使每个节点都能一键创建自己的对等网络。
pub struct EmbeddedCoordinator {
    pub ticket: NetworkTicket,
    pub signaling_addr: SocketAddr,
    pub relay_addr: SocketAddr,
    pub session_manager: Arc<SessionManager>,
    _signaling_task: JoinHandle<Result<()>>,
    _relay_task: JoinHandle<Result<()>>,
}

impl EmbeddedCoordinator {
    /// 启动内嵌协调 Actor
    pub async fn start(
        network_name: &str,
        signaling_port: u16,
        relay_port: u16,
        external_host: Option<String>,
        ip_pool: Option<&str>,
    ) -> Result<Self> {
        let pool = ip_pool.unwrap_or("fd00::1000/120");
        let session_manager =
            Arc::new(SessionManager::new(pool).context("无法初始化内嵌协调器地址池")?);

        let signaling_bind = format!("0.0.0.0:{signaling_port}");
        let signaling_server =
            SignalingServer::new_with_manager(signaling_bind, session_manager.clone());

        let relay_bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), relay_port);
        let relay_server =
            RelayServer::bind_with_generated_certificate(relay_bind, session_manager.clone())
                .context("无法启动内嵌 QUIC 中继服务")?;

        let actual_relay_addr = relay_server.local_addr();
        let relay_pin = relay_server.certificate().public_key_pin;
        let relay_endpoint = RelayEndpoint {
            addr: actual_relay_addr,
            server_name: "fuckipv6.local".to_string(),
            public_key_pin: relay_pin,
        };

        let signaling_server = signaling_server.with_relay_endpoint(relay_endpoint);
        signaling_server.spawn_timeout_cleanup(120);

        let host_identity = Identity::generate()?;
        let host_pubkey = host_identity.public_key_bytes();

        // 确定票据暴露的接入地址
        let ticket_endpoint = if let Some(host) = external_host {
            if host.contains(':') {
                host
            } else {
                format!("{host}:{signaling_port}")
            }
        } else {
            // 本机探测默认地址
            format!("127.0.0.1:{signaling_port}")
        };

        let ticket = NetworkTicket::new(host_pubkey, ticket_endpoint, network_name)
            .with_ipv6(pool)
            .with_ipv4("10.8.0.0/24");

        let signaling_task = tokio::spawn(async move { signaling_server.run().await });

        let relay_task = tokio::spawn(async move { relay_server.run().await });

        let signaling_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), signaling_port);

        tracing::info!(
            "🚀 内嵌协调 Actor 启动成功: 信令={}, QUIC中继={}",
            signaling_addr,
            actual_relay_addr
        );
        tracing::info!("🎫 邀请票据: {}", ticket);

        Ok(Self {
            ticket,
            signaling_addr,
            relay_addr: actual_relay_addr,
            session_manager,
            _signaling_task: signaling_task,
            _relay_task: relay_task,
        })
    }
}

impl Drop for EmbeddedCoordinator {
    fn drop(&mut self) {
        self._signaling_task.abort();
        self._relay_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embedded_coordinator_startup() {
        // 使用端口 0 避免冲突 (信令使用系统空闲端口)
        let coordinator = EmbeddedCoordinator::start(
            "test-mesh",
            0, // random free port
            0,
            Some("127.0.0.1".to_string()),
            Some("fd00::1000/120"),
        )
        .await
        .expect("start embedded coordinator failed");

        assert_eq!(coordinator.ticket.network_name, "test-mesh");
        assert!(coordinator.ticket.to_string().starts_with("fk://"));
    }
}
