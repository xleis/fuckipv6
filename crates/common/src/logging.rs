use anyhow::Result;
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer};

pub const DEFAULT_LOG_LEVEL: &str = "info";
const ERROR_LOG_CAPACITY: usize = 512;

static ERROR_LOGS: OnceLock<Mutex<VecDeque<AggregatedLogEntry>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatedLogEntry {
    pub timestamp_ms: u128,
    pub level: String,
    pub target: String,
    pub message: String,
}

pub fn init_logging_default() -> Result<()> {
    init_logging_with_level(DEFAULT_LOG_LEVEL)
}

pub fn init_logging_with_level(level: &str) -> Result<()> {
    let filter = logging_env_filter(level)?;
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .compact();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(ErrorLogLayer)
        .with(fmt_layer)
        .try_init();
    Ok(())
}

pub fn logging_env_filter(config_level: &str) -> Result<EnvFilter> {
    if std::env::var_os("RUST_LOG").is_some() {
        return EnvFilter::try_from_default_env().map_err(Into::into);
    }

    let level = normalize_log_level(config_level)?;
    EnvFilter::try_new(level).map_err(Into::into)
}

pub fn normalize_log_level(level: &str) -> Result<&'static str> {
    match level.trim().to_ascii_lowercase().as_str() {
        "" => Ok(DEFAULT_LOG_LEVEL),
        "trace" => Ok("trace"),
        "debug" => Ok("debug"),
        "info" => Ok("info"),
        "warn" | "warning" => Ok("warn"),
        "error" => Ok("error"),
        other => anyhow::bail!(
            "无效日志级别 `{}`，可用值: trace, debug, info, warn, error",
            other
        ),
    }
}

pub fn recent_error_logs(max_lines: usize) -> Vec<AggregatedLogEntry> {
    let max_lines = max_lines.min(ERROR_LOG_CAPACITY);
    let logs = error_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    logs.iter()
        .rev()
        .take(max_lines)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub fn recent_error_log_lines(max_lines: usize) -> Vec<String> {
    recent_error_logs(max_lines)
        .into_iter()
        .map(|entry| {
            format!(
                "{} {} {}: {}",
                entry.timestamp_ms, entry.level, entry.target, entry.message
            )
        })
        .collect()
}

pub fn clear_aggregated_error_logs() {
    error_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn error_logs() -> &'static Mutex<VecDeque<AggregatedLogEntry>> {
    ERROR_LOGS.get_or_init(|| Mutex::new(VecDeque::with_capacity(ERROR_LOG_CAPACITY)))
}

fn push_error_log(entry: AggregatedLogEntry) {
    let mut logs = error_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if logs.len() >= ERROR_LOG_CAPACITY {
        logs.pop_front();
    }
    logs.push_back(entry);
}

struct ErrorLogLayer;

impl<S> Layer<S> for ErrorLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if !matches!(*metadata.level(), Level::WARN | Level::ERROR) {
            return;
        }

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = visitor.finish();
        if message.is_empty() {
            return;
        }

        push_error_log(AggregatedLogEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        });
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl MessageVisitor {
    fn finish(self) -> String {
        match (self.message, self.fields.is_empty()) {
            (Some(message), true) => message,
            (Some(message), false) => format!("{} {}", message, self.fields.join(" ")),
            (None, false) => self.fields.join(" "),
            (None, true) => String::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let value = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(value);
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_supported_levels() {
        assert_eq!(
            normalize_log_level("DEBUG").expect("DEBUG should normalize"),
            "debug"
        );
        assert_eq!(
            normalize_log_level(" warning ").expect("warning should normalize"),
            "warn"
        );
        assert_eq!(
            normalize_log_level("").expect("empty level should use default"),
            "info"
        );
    }

    #[test]
    fn rejects_invalid_level() {
        let err = normalize_log_level("verbose")
            .expect_err("unsupported log level should be rejected")
            .to_string();
        assert!(err.contains("无效日志级别"));
    }

    #[test]
    fn aggregates_recent_warning_and_error_logs() {
        clear_aggregated_error_logs();

        push_error_log(AggregatedLogEntry {
            timestamp_ms: 1,
            level: "WARN".to_string(),
            target: "test".to_string(),
            message: "first".to_string(),
        });
        push_error_log(AggregatedLogEntry {
            timestamp_ms: 2,
            level: "ERROR".to_string(),
            target: "test".to_string(),
            message: "second".to_string(),
        });

        let logs = recent_error_logs(1);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "second");
        assert_eq!(
            recent_error_log_lines(2),
            vec![
                "1 WARN test: first".to_string(),
                "2 ERROR test: second".to_string()
            ]
        );
    }
}
