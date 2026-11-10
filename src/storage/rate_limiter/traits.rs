use crate::error::Result;
use async_trait::async_trait;

/// Unified trait for all rate limiter implementations
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// Check if a request is allowed and consume a token/slot
    async fn allow_request(&self) -> Result<RateLimitResult>;

    /// Get current usage without consuming
    async fn get_current_usage(&self) -> Result<RateLimitUsage>;

    /// Get the algorithm name for monitoring/debugging
    fn algorithm_name(&self) -> &str;

    /// Get provider ID
    fn provider_id(&self) -> &str;
}

/// Result of a rate limit check
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed
    pub allowed: bool,

    /// Remaining capacity (tokens or slots)
    pub remaining: i64,

    /// Suggested retry time in milliseconds (if rejected)
    pub retry_after_ms: Option<u64>,

    /// Current usage ratio (0.0 - 1.0)
    pub usage_ratio: f64,
}

/// Current usage statistics
#[derive(Debug, Clone)]
pub struct RateLimitUsage {
    /// Current count/tokens
    pub current: f64,

    /// Maximum limit
    pub limit: u32,

    /// Usage ratio (0.0 - 1.0)
    pub ratio: f64,
}

impl RateLimitResult {
    pub fn allowed(remaining: i64, limit: u32) -> Self {
        let usage_ratio = 1.0 - (remaining as f64 / limit as f64).max(0.0).min(1.0);
        Self {
            allowed: true,
            remaining,
            retry_after_ms: None,
            usage_ratio,
        }
    }

    pub fn rejected(retry_after_ms: u64, _limit: u32) -> Self {
        Self {
            allowed: false,
            remaining: 0,
            retry_after_ms: Some(retry_after_ms),
            usage_ratio: 1.0,
        }
    }
}
