use crate::error::{AppError, Result};
use crate::storage::rate_limiter::traits::{RateLimitResult, RateLimitUsage, RateLimiter};
use crate::storage::RedisService;
use crate::utils::TimeUtils;
use async_trait::async_trait;
use rand::Rng;
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

/// Sliding Window Rate Limiter
///
/// Algorithm: Sliding window with sorted set (consistent with Node SafeRollingRateLimiter)
/// - Each request is stored as a member in a sorted set
/// - Score is the timestamp (milliseconds)
/// - Old entries are removed on each check
/// - Uses ZCOUNT for precise range query
///
/// Redis Structure (consistent with Node):
/// - Key: rate-limit:{provider_id}
/// - Type: Sorted Set
/// - Members: {timestamp}:{random}:{pid} format
/// - Scores: millisecond timestamps
pub struct SlidingWindowRateLimiter {
    redis_service: Arc<RedisService>,
    provider_id: String,
    max_rps: u32,
    window_seconds: u64,
}

impl SlidingWindowRateLimiter {
    pub fn new(redis_service: Arc<RedisService>, provider_id: String, max_rps: u32) -> Self {
        debug!(
            "Creating SlidingWindowRateLimiter: provider={}, max_rps={}",
            provider_id, max_rps
        );
        Self {
            redis_service,
            provider_id,
            max_rps,
            window_seconds: 1,
        }
    }

    fn lua_script() -> &'static str {
        r#"
            local key = KEYS[1]
            local now = tonumber(ARGV[1])
            local member = ARGV[2]
            local window = tonumber(ARGV[3])
            local limit = tonumber(ARGV[4])
            
            -- Remove expired requests
            redis.call('ZREMRANGEBYSCORE', key, 0, now - window)
            
            -- Get current count (use ZCOUNT for precise range query)
            local count = redis.call('ZCOUNT', key, now - window, now)
            
            if count >= limit then
                return {0, count}
            end
            
            -- Add new request
            redis.call('ZADD', key, now, member)
            return {1, count + 1}
        "#
    }
}

#[async_trait]
impl RateLimiter for SlidingWindowRateLimiter {
    async fn allow_request(&self) -> Result<RateLimitResult> {
        let start = Instant::now();

        if self.max_rps == 0 {
            return Ok(RateLimitResult::rejected(0, self.max_rps));
        }

        let now = TimeUtils::current_timestamp_millis_checked()? as i64;

        // Generate unique member: timestamp:random:pid (consistent with Node implementation)
        // Node: `${currentTimestamp}:${Math.random()}:${process.pid}`
        let random_val: f64 = rand::thread_rng().gen();
        let pid = std::process::id();
        let unique_member = format!("{}:{}:{}", now, random_val, pid);

        // Key format: rate-limit:{provider_id} (consistent with Node implementation)
        let key = format!("rate-limit:{}", self.provider_id);
        let window_ms = (self.window_seconds * 1000) as i64;

        let mut conn = self.redis_service.connection_manager.clone();

        // Lua script args: now, member, window_ms, limit
        let result: Vec<i64> = redis::cmd("EVAL")
            .arg(Self::lua_script())
            .arg(1)
            .arg(&key)
            .arg(now)
            .arg(&unique_member)
            .arg(window_ms)
            .arg(self.max_rps)
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Sliding window check failed: {}", e)))?;

        debug!(
            "Lua script: key={}, now={}, member={}, window_ms={}, limit={} => [allowed={}, count={}]",
            key, now, unique_member, window_ms, self.max_rps, result[0], result[1]
        );

        let allowed = result[0] == 1;
        let current_count = result[1];
        let remaining = (self.max_rps as i64).saturating_sub(current_count);

        // Record metrics
        let duration = start.elapsed().as_secs_f64();
        crate::monitoring::record_rate_limiter_latency(
            "sliding_window",
            &self.provider_id,
            duration,
        );
        crate::monitoring::increment_rate_limiter_requests(
            "sliding_window",
            &self.provider_id,
            if allowed { "allowed" } else { "rejected" },
        );

        if allowed {
            let usage_ratio = current_count as f64 / self.max_rps as f64;
            crate::monitoring::update_rate_limiter_usage(
                "sliding_window",
                &self.provider_id,
                usage_ratio,
            );

            debug!(
                "Sliding window allowed: provider={}, count={}/{}, remaining={}, latency={:.3}ms",
                self.provider_id,
                current_count,
                self.max_rps,
                remaining,
                duration * 1000.0
            );
            Ok(RateLimitResult::allowed(remaining, self.max_rps))
        } else {
            crate::monitoring::update_rate_limiter_usage("sliding_window", &self.provider_id, 1.0);

            debug!(
                "Sliding window rejected: provider={}, count={}/{}, latency={:.3}ms",
                self.provider_id,
                current_count,
                self.max_rps,
                duration * 1000.0
            );
            // Estimate retry time based on window
            let retry_after = (self.window_seconds * 1000) as u64;
            Ok(RateLimitResult::rejected(retry_after, self.max_rps))
        }
    }

    async fn get_current_usage(&self) -> Result<RateLimitUsage> {
        let now = TimeUtils::current_timestamp_millis_checked()? as i64;
        let key = format!("rate-limit:{}", self.provider_id);
        let window_start = now - (self.window_seconds * 1000) as i64;

        let mut conn = self.redis_service.connection_manager.clone();

        let current_count: i64 = redis::cmd("ZCOUNT")
            .arg(&key)
            .arg(window_start)
            .arg(now)
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to get sliding window count: {}", e)))?;

        let ratio = current_count as f64 / self.max_rps as f64;

        Ok(RateLimitUsage {
            current: current_count as f64,
            limit: self.max_rps,
            ratio,
        })
    }

    fn algorithm_name(&self) -> &str {
        "sliding_window"
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}
