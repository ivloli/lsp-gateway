use crate::storage::rate_limiter::config::RateLimiterConfig;
use crate::storage::rate_limiter::sliding_window::SlidingWindowRateLimiter;
use crate::storage::rate_limiter::token_bucket::TokenBucketRateLimiter;
use crate::storage::rate_limiter::traits::RateLimiter;
use crate::storage::RedisService;
use std::sync::Arc;
use tracing::debug;

/// Rate limiter algorithm selection
#[derive(Debug, Clone)]
pub enum RateLimitAlgorithm {
    /// Sliding window with sorted set (original)
    SlidingWindow,

    /// Token bucket with configurable burst
    TokenBucket { burst_multiplier: f64 },
}

/// Factory for creating rate limiters
pub struct RateLimiterFactory;

impl RateLimiterFactory {
    pub fn create_from_config(
        redis_service: Arc<RedisService>,
        provider_id: String,
        max_rps: u32,
    ) -> Arc<dyn RateLimiter> {
        let config = RateLimiterConfig::global();
        Self::create(
            config.algorithm.clone(),
            redis_service,
            provider_id,
            max_rps,
        )
    }

    /// Create a rate limiter with specified algorithm
    pub fn create(
        algorithm: RateLimitAlgorithm,
        redis_service: Arc<RedisService>,
        provider_id: String,
        max_rps: u32,
    ) -> Arc<dyn RateLimiter> {
        match algorithm {
            RateLimitAlgorithm::SlidingWindow => {
                debug!(
                    "Creating SlidingWindow rate limiter: provider={}, max_rps={}",
                    provider_id, max_rps
                );
                Arc::new(SlidingWindowRateLimiter::new(
                    redis_service,
                    provider_id,
                    max_rps,
                ))
            }
            RateLimitAlgorithm::TokenBucket { burst_multiplier } => {
                let burst_capacity = (max_rps as f64 * burst_multiplier).ceil() as u32;
                debug!(
                    "Creating TokenBucket rate limiter: provider={}, rate={}/s, burst={} ({}x)",
                    provider_id, max_rps, burst_capacity, burst_multiplier
                );
                Arc::new(TokenBucketRateLimiter::new(
                    redis_service,
                    provider_id,
                    max_rps,
                    burst_capacity,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_algorithm_selection() {
        // Just verify the enum variants exist
        let _sw = RateLimitAlgorithm::SlidingWindow;
        let _tb = RateLimitAlgorithm::TokenBucket {
            burst_multiplier: 2.0,
        };
    }
}
