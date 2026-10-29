use anyhow::Result;
use fuckipv6_common::{ClientConfig, FallbackThreshold};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::time::{interval, Duration};

use crate::p2p::connection::P2pEngine;

const CONFIG_RELOAD_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
struct ReloadableConfig {
    fallback_threshold: FallbackThreshold,
    log_level: String,
}

impl From<&ClientConfig> for ReloadableConfig {
    fn from(config: &ClientConfig) -> Self {
        Self {
            fallback_threshold: config.p2p.fallback_threshold.clone(),
            log_level: config.log.level.clone(),
        }
    }
}

pub fn spawn_config_watcher(
    config_path: impl Into<PathBuf>,
    initial_config: ClientConfig,
    p2p: P2pEngine,
) -> tokio::task::JoinHandle<()> {
    let config_path = config_path.into();
    tokio::spawn(async move {
        if let Err(e) = watch_config(config_path, initial_config, p2p).await {
            tracing::warn!("配置热重载任务已停止: {:?}", e);
        }
    })
}

async fn watch_config(
    config_path: PathBuf,
    initial_config: ClientConfig,
    p2p: P2pEngine,
) -> Result<()> {
    let mut ticker = interval(CONFIG_RELOAD_INTERVAL);
    let mut last_modified = file_modified_at(&config_path).await;
    let mut current = ReloadableConfig::from(&initial_config);

    loop {
        ticker.tick().await;

        let modified = file_modified_at(&config_path).await;
        if modified.is_some() && modified == last_modified {
            continue;
        }

        let Ok(config) = ClientConfig::from_file(&config_path) else {
            tracing::warn!("配置文件变更后重新加载失败: {}", config_path.display());
            last_modified = modified;
            continue;
        };

        let next = ReloadableConfig::from(&config);
        apply_reloadable_changes(&p2p, &current, &next).await;
        current = next;
        last_modified = modified;
    }
}

async fn file_modified_at(path: &Path) -> Option<SystemTime> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

async fn apply_reloadable_changes(
    p2p: &P2pEngine,
    current: &ReloadableConfig,
    next: &ReloadableConfig,
) {
    if current.fallback_threshold != next.fallback_threshold {
        p2p.set_fallback_threshold(next.fallback_threshold.clone())
            .await;
        tracing::info!(
            "已热重载 P2P fallback 阈值: max_rtt={}ms, max_loss={:.3}, timeout={}s",
            next.fallback_threshold.max_rtt_ms,
            next.fallback_threshold.max_loss_rate,
            next.fallback_threshold.timeout_secs
        );
    }

    if current.log_level != next.log_level {
        tracing::warn!(
            "检测到 log.level 从 {} 变为 {}；当前日志订阅器需要重启客户端后生效",
            current.log_level,
            next.log_level
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn applies_fallback_threshold_changes() -> anyhow::Result<()> {
        let p2p = P2pEngine::new(0).await?;
        let current = ReloadableConfig {
            fallback_threshold: FallbackThreshold::default(),
            log_level: "info".to_string(),
        };
        let next = ReloadableConfig {
            fallback_threshold: FallbackThreshold {
                max_rtt_ms: 50,
                max_loss_rate: 0.01,
                timeout_secs: 5,
            },
            log_level: "info".to_string(),
        };

        apply_reloadable_changes(&p2p, &current, &next).await;

        assert_eq!(p2p.fallback_threshold().await, next.fallback_threshold);
        Ok(())
    }

    #[test]
    fn reloadable_config_extracts_supported_fields() {
        let reloadable = ReloadableConfig {
            fallback_threshold: FallbackThreshold {
                max_rtt_ms: 123,
                max_loss_rate: 0.05,
                timeout_secs: 30,
            },
            log_level: "debug".to_string(),
        };

        assert_eq!(reloadable.fallback_threshold.max_rtt_ms, 123);
        assert_eq!(reloadable.log_level, "debug");
    }
}
