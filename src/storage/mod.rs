pub mod cache;
pub mod gateway_service;
pub mod health_service;
pub mod lock_service;
pub mod metrics_service;
pub mod rate_limiter;
pub mod redis;

pub use cache::{CacheService, CacheStats};

// Legacy rate limiter (kept for backward compatibility)
// New code should use rate_limiter::RateLimiterFactory
#[deprecated(since = "0.2.0", note = "Use rate_limiter::RateLimiterFactory instead")]
pub use rate_limiter::sliding_window::SlidingWindowRateLimiter as RollingRateLimiter;
pub use rate_limiter::{RateLimitResult, RateLimiter, RateLimiterFactory};

pub use gateway_service::{GatewayInstance, GatewayService};
pub use health_service::{ProviderHealth, ProviderHealthInfo, StorageHealthService};
pub use lock_service::LockService;
pub use metrics_service::{GatewayMetrics, ProviderMetrics, StorageMetricsService};
pub use redis::RedisService;
