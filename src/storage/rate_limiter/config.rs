//! Rate Limiter Configuration
//!
//! Provides cached configuration loaded once at startup to avoid
//! repeated environment variable reads on every request.

use once_cell::sync::Lazy;
use std::sync::Arc;
use tracing::{info, warn};

use super::factory::RateLimitAlgorithm;

/// Global rate limiter configuration loaded once at startup
static RATE_LIMITER_CONFIG: Lazy<Arc<RateLimiterConfig>> =
    Lazy::new(|| Arc::new(RateLimiterConfig::from_env()));

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Selected algorithm
    pub algorithm: RateLimitAlgorithm,

    /// Algorithm name for logging
    pub algorithm_name: String,
}

impl RateLimiterConfig {
    /// Load configuration from environment variables (called once at startup)
    fn from_env() -> Self {
        let algorithm_str = std::env::var("RATE_LIMIT_ALGORITHM")
            .unwrap_or_else(|_| "token_bucket".to_string())
            .to_lowercase();

        let (algorithm, algorithm_name) = match algorithm_str.as_str() {
            "token_bucket" => {
                let burst_multiplier = std::env::var("BURST_MULTIPLIER")
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(1.0);

                // Validate burst multiplier
                let burst_multiplier = if burst_multiplier < 1.0 || burst_multiplier > 10.0 {
                    warn!(
                        "Invalid BURST_MULTIPLIER={}, using default 1.0 (valid range: 1.0-10.0)",
                        burst_multiplier
                    );
                    1.0
                } else {
                    burst_multiplier
                };

                info!(
                    "Rate limiter configured: algorithm=token_bucket, burst_multiplier={}",
                    burst_multiplier
                );

                (
                    RateLimitAlgorithm::TokenBucket { burst_multiplier },
                    "token_bucket".to_string(),
                )
            }
            "sliding_window" => {
                info!("Rate limiter configured: algorithm=sliding_window");
                (
                    RateLimitAlgorithm::SlidingWindow,
                    "sliding_window".to_string(),
                )
            }
            _ => {
                warn!(
                    "Unknown RATE_LIMIT_ALGORITHM='{}', falling back to sliding_window",
                    algorithm_str
                );
                (
                    RateLimitAlgorithm::SlidingWindow,
                    "sliding_window".to_string(),
                )
            }
        };

        Self {
            algorithm,
            algorithm_name,
        }
    }

    /// Get the global configuration instance
    pub fn global() -> Arc<Self> {
        RATE_LIMITER_CONFIG.clone()
    }

    /// Get algorithm name
    pub fn algorithm_name(&self) -> &str {
        &self.algorithm_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_singleton() {
        let config1 = RateLimiterConfig::global();
        let config2 = RateLimiterConfig::global();

        // Should be the same instance
        assert!(Arc::ptr_eq(&config1, &config2));
    }
}
