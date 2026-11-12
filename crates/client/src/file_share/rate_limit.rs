//! Shared bandwidth admission for File chunk I/O.
//!
//! The adapter returns a retry interval instead of sleeping on an async runtime
//! thread. A single token bucket is shared by uploads and downloads admitted by
//! one File runtime, so the configured bandwidth applies to aggregate File
//! traffic rather than independently to every transfer.

use std::sync::Mutex;

use fuckipv6_common::domain::MAX_TRANSFER_CHUNK_BYTES;

/// A bounded, share-wide token bucket. Tokens are measured in bytes and begin
/// empty so the first chunk is paced instead of receiving an initial burst.
#[derive(Debug, Default)]
pub(crate) struct TransferRateLimiter {
    state: Mutex<RateLimitState>,
}

#[derive(Debug, Default)]
struct RateLimitState {
    rate_bytes_per_second: u64,
    tokens: u64,
    last_refill_unix_ms: u64,
}

impl TransferRateLimiter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Consume bandwidth for one new chunk. When the bucket is empty this
    /// returns the bounded time until the chunk can be retried. Replaying an
    /// already accepted chunk must not call this method.
    pub(crate) fn try_consume(
        &self,
        bytes: u64,
        max_bandwidth_kbps: u32,
        now_unix_ms: u64,
    ) -> Result<(), u64> {
        if max_bandwidth_kbps == 0 {
            if let Ok(mut state) = self.state.lock() {
                *state = RateLimitState::default();
            }
            return Ok(());
        }

        let rate_bytes_per_second = u64::from(max_bandwidth_kbps).saturating_mul(1_000) / 8;
        let capacity = rate_bytes_per_second.max(MAX_TRANSFER_CHUNK_BYTES);
        let Ok(mut state) = self.state.lock() else {
            return Err(1_000);
        };

        if state.rate_bytes_per_second != rate_bytes_per_second {
            state.rate_bytes_per_second = rate_bytes_per_second;
            state.tokens = 0;
            state.last_refill_unix_ms = now_unix_ms;
        } else if now_unix_ms > state.last_refill_unix_ms {
            let elapsed_ms = now_unix_ms.saturating_sub(state.last_refill_unix_ms);
            let refill =
                u128::from(elapsed_ms).saturating_mul(u128::from(rate_bytes_per_second)) / 1_000;
            state.tokens = state
                .tokens
                .saturating_add(u64::try_from(refill).unwrap_or(u64::MAX))
                .min(capacity);
            state.last_refill_unix_ms = now_unix_ms;
        }

        if state.tokens >= bytes {
            state.tokens = state.tokens.saturating_sub(bytes);
            return Ok(());
        }

        let deficit = bytes.saturating_sub(state.tokens);
        let retry_after_ms = u128::from(deficit)
            .saturating_mul(1_000)
            .div_ceil(u128::from(rate_bytes_per_second));
        Err(u64::try_from(retry_after_ms).unwrap_or(u64::MAX).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_are_paced_without_blocking_and_retry_is_quantified() {
        let limiter = TransferRateLimiter::new();
        assert_eq!(limiter.try_consume(1_000, 8, 1_000), Err(1_000));
        assert_eq!(limiter.try_consume(1_000, 8, 1_999), Err(1));
        assert_eq!(limiter.try_consume(1_000, 8, 2_000), Ok(()));
        assert_eq!(limiter.try_consume(1_000, 8, 2_500), Err(500));
        assert_eq!(limiter.try_consume(1_000, 8, 3_000), Ok(()));
    }

    #[test]
    fn unlimited_mode_does_not_accumulate_state() {
        let limiter = TransferRateLimiter::new();
        assert_eq!(limiter.try_consume(1_000, 8, 1_000), Err(1_000));
        assert_eq!(limiter.try_consume(u64::MAX, 0, 1_000), Ok(()));
        assert_eq!(
            limiter.try_consume(MAX_TRANSFER_CHUNK_BYTES, 0, 1_000),
            Ok(())
        );
    }

    #[test]
    fn rate_change_starts_a_fresh_bucket() {
        let limiter = TransferRateLimiter::new();
        assert_eq!(limiter.try_consume(2_000, 8, 1_000), Err(2_000));
        assert_eq!(limiter.try_consume(2_000, 16, 2_000), Err(1_000));
        assert_eq!(limiter.try_consume(2_000, 16, 3_000), Ok(()));
    }
}
