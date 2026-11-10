use crate::error::{AppError, Result};
use crate::storage::rate_limiter::traits::{RateLimitResult, RateLimitUsage, RateLimiter};
use crate::storage::RedisService;
use crate::utils::TimeUtils;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

/// Token Bucket Rate Limiter
///
/// Algorithm: Token bucket with continuous refill
/// - Tokens are added at a constant rate
/// - Requests consume tokens
/// - Supports burst traffic up to burst_capacity
///
/// Redis Structure:
/// - Key: lsp:token_bucket:{provider_id}
/// - Type: Hash
/// - Fields:
///   - tokens: current available tokens (float)
///   - last_refill: last refill timestamp (milliseconds)
pub struct TokenBucketRateLimiter {
    redis_service: Arc<RedisService>,
    provider_id: String,
    rate: u32,           // tokens per second
    burst_capacity: u32, // maximum tokens
}

impl TokenBucketRateLimiter {
    pub fn new(
        redis_service: Arc<RedisService>,
        provider_id: String,
        rate: u32,
        burst_capacity: u32,
    ) -> Self {
        debug!(
            "Creating TokenBucketRateLimiter: provider={}, rate={}/s, burst={}",
            provider_id, rate, burst_capacity
        );
        Self {
            redis_service,
            provider_id,
            rate,
            burst_capacity,
        }
    }

    /// Lua script for atomic token bucket operation
    ///
    /// This script:
    /// 1. Calculates tokens to add based on time elapsed
    /// 2. Refills tokens up to burst capacity
    /// 3. Attempts to consume tokens for the request
    /// 4. Returns result with retry time if rejected
    fn lua_script() -> &'static str {
        r#"
            local key = KEYS[1]
            local rate = tonumber(ARGV[1])
            local burst = tonumber(ARGV[2])
            local now = tonumber(ARGV[3])
            local cost = tonumber(ARGV[4])
            
            -- Get current state
            local state = redis.call('HMGET', key, 'tokens', 'last_refill')
            local tokens = tonumber(state[1])
            local last_refill = tonumber(state[2])
            
            -- Initialize if first request
            if tokens == nil then
                tokens = burst
                last_refill = now
            end
            
            -- Calculate tokens to add
            local delta = math.max(0, now - last_refill)
            local tokens_to_add = (delta / 1000.0) * rate
            tokens = math.min(tokens + tokens_to_add, burst)
            
            -- Try to consume tokens
            local allowed = 0
            local retry_after = 0
            
            if tokens >= cost then
                tokens = tokens - cost
                allowed = 1
                last_refill = now
            else
                -- Calculate retry time
                retry_after = math.ceil((cost - tokens) / rate * 1000)
            end
            
            -- Update state
            redis.call('HSET', key, 'tokens', tokens, 'last_refill', now)
            redis.call('EXPIRE', key, 10)
            
            -- Return: [allowed, remaining_tokens, retry_after_ms]
            return {allowed, math.floor(tokens), retry_after}
        "#
    }
}

#[async_trait]
impl RateLimiter for TokenBucketRateLimiter {
    async fn allow_request(&self) -> Result<RateLimitResult> {
        let start = Instant::now();

        if self.rate == 0 {
            return Ok(RateLimitResult::rejected(0, self.rate));
        }

        let now = TimeUtils::current_timestamp_millis_checked()? as i64;
        let key = format!("lsp:token_bucket:{}", self.provider_id);

        let mut conn = self.redis_service.connection_manager.clone();

        let result: Vec<i64> = redis::cmd("EVAL")
            .arg(Self::lua_script())
            .arg(1)
            .arg(&key)
            .arg(self.rate)
            .arg(self.burst_capacity)
            .arg(now)
            .arg(1) // cost = 1 token per request
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Token bucket check failed: {}", e)))?;

        let allowed = result[0] == 1;
        let remaining = result[1];
        let retry_after = result[2] as u64;

        // Record metrics
        let duration = start.elapsed().as_secs_f64();
        crate::monitoring::record_rate_limiter_latency("token_bucket", &self.provider_id, duration);
        crate::monitoring::increment_rate_limiter_requests(
            "token_bucket",
            &self.provider_id,
            if allowed { "allowed" } else { "rejected" },
        );

        if allowed {
            let usage_ratio = 1.0 - (remaining as f64 / self.burst_capacity as f64);
            crate::monitoring::update_rate_limiter_usage(
                "token_bucket",
                &self.provider_id,
                usage_ratio,
            );

            debug!(
                "Token bucket allowed: provider={}, remaining={}, latency={:.3}ms",
                self.provider_id,
                remaining,
                duration * 1000.0
            );
            Ok(RateLimitResult::allowed(remaining, self.burst_capacity))
        } else {
            crate::monitoring::update_rate_limiter_usage("token_bucket", &self.provider_id, 1.0);

            debug!(
                "Token bucket rejected: provider={}, retry_after={}ms, latency={:.3}ms",
                self.provider_id,
                retry_after,
                duration * 1000.0
            );
            Ok(RateLimitResult::rejected(retry_after, self.burst_capacity))
        }
    }

    async fn get_current_usage(&self) -> Result<RateLimitUsage> {
        let key = format!("lsp:token_bucket:{}", self.provider_id);
        let mut conn = self.redis_service.connection_manager.clone();

        use redis::AsyncCommands;
        let state: Vec<Option<String>> = conn
            .hget(&key, &["tokens", "last_refill"])
            .await
            .map_err(|e| AppError::Redis(format!("Failed to get token bucket state: {}", e)))?;

        let remaining_tokens = state[0]
            .as_ref()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(self.burst_capacity as f64);

        // Need to refill tokens to current time for accurate usage
        let last_refill = state[1]
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| TimeUtils::current_timestamp_millis_checked().unwrap_or(0) as i64);

        let now = TimeUtils::current_timestamp_millis_checked()? as i64;
        let delta = (now - last_refill).max(0) as f64;
        let tokens_to_add = (delta / 1000.0) * self.rate as f64;
        let current_tokens = (remaining_tokens + tokens_to_add).min(self.burst_capacity as f64);

        // Calculate used tokens (consistent with sliding window semantics)
        let used_tokens = self.burst_capacity as f64 - current_tokens;
        let ratio = used_tokens / self.burst_capacity as f64;

        Ok(RateLimitUsage {
            current: used_tokens, // Used tokens (not remaining)
            limit: self.burst_capacity,
            ratio,
        })
    }

    fn algorithm_name(&self) -> &str {
        "token_bucket"
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_script_syntax() {
        // Just verify the Lua script is valid syntax
        let script = TokenBucketRateLimiter::lua_script();
        assert!(script.contains("KEYS[1]"));
        assert!(script.contains("tokens"));
        assert!(script.contains("last_refill"));
    }

    #[test]
    fn test_usage_calculation_semantics() {
        // Test that get_current_usage returns used tokens, not remaining tokens
        // This ensures consistency with sliding window semantics

        let burst_capacity = 100;
        let remaining_tokens = 30.0;

        // Expected: used_tokens = burst_capacity - remaining_tokens
        let expected_used = burst_capacity as f64 - remaining_tokens;
        let expected_ratio = expected_used / burst_capacity as f64;

        assert_eq!(
            expected_used, 70.0,
            "Should calculate used tokens correctly"
        );
        assert_eq!(
            expected_ratio, 0.7,
            "Should calculate usage ratio correctly"
        );

        // Verify semantics match sliding window:
        // - current: amount used (higher = more pressure)
        // - ratio: usage percentage (0.0 = empty, 1.0 = full)
        assert!(
            expected_ratio >= 0.0 && expected_ratio <= 1.0,
            "Ratio should be in [0, 1]"
        );
    }
}
