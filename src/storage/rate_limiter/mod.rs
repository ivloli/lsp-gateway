//! Rate Limiter Module
//!
//! Provides multiple rate limiting algorithms for controlling request rates to providers.
//!
//! ## Algorithms
//!
//! ### Sliding Window (Original)
//! - Uses Redis Sorted Set to track individual requests
//! - Accurate but higher memory usage (O(max_rps) per provider)
//! - 4 Redis commands per request
//!
//! ### Token Bucket (New, Recommended)
//! - Uses Redis Hash to track token state
//! - Lower memory usage (O(1) per provider)
//! - 3 Redis commands per request
//! - Built-in burst support
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::storage::rate_limiter::{RateLimiterFactory, RateLimiter};
//!
//! // Create with cached config (recommended, zero overhead)
//! let limiter = RateLimiterFactory::create_from_config(
//!     redis_service,
//!     "provider-1".to_string(),
//!     1000, // max_rps
//! );
//!
//! // Or create from environment variables (legacy, reads env vars)
//! let limiter = RateLimiterFactory::from_env(
//!     redis_service,
//!     "provider-1".to_string(),
//!     1000, // max_rps
//! );
//!
//! // Check if request is allowed
//! let result = limiter.allow_request().await?;
//! if result.allowed {
//!     // Process request
//! } else {
//!     // Reject with retry_after
//! }
//! ```
//!
//! ## Configuration
//!
//! Environment variables (loaded once at startup):
//! - `RATE_LIMIT_ALGORITHM`: "sliding_window" | "token_bucket" (default: "sliding_window")
//! - `BURST_MULTIPLIER`: float (default: 2.0, range: 1.0-10.0, only for token_bucket)

pub mod config;
pub mod factory;
pub mod sliding_window;
pub mod token_bucket;
pub mod traits;

// Re-export main types
pub use config::RateLimiterConfig;
pub use factory::{RateLimitAlgorithm, RateLimiterFactory};
pub use sliding_window::SlidingWindowRateLimiter;
pub use token_bucket::TokenBucketRateLimiter;
pub use traits::{RateLimitResult, RateLimitUsage, RateLimiter};
