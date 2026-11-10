use crate::error::Result;
use crate::models::ModelProvider;
use crate::storage::{RateLimiterFactory, RedisService};
use futures::future::join_all;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// Rate limit filter that filters out providers exceeding their RPS limits
pub struct RateLimitFilter {
    redis_service: Arc<RedisService>,
}

#[derive(Debug)]
pub struct RateLimitFilterResult {
    pub available_providers: Vec<ModelProvider>,
    pub total_count: usize,
    pub all_rate_limited: bool,
}

impl RateLimitFilter {
    pub fn new(redis_service: Arc<RedisService>) -> Self {
        Self { redis_service }
    }

    /// Filter providers based on their rate limits
    ///
    /// # Arguments
    /// * `providers` - List of providers to filter
    /// * `group` - The provider group for metrics
    pub async fn filter(
        &self,
        providers: Vec<ModelProvider>,
        group: &str,
    ) -> Result<RateLimitFilterResult> {
        let total_count = providers.len();
        let group_owned = group.to_string();

        debug!(
            "Filtering {} providers by rate limits (concurrent)",
            total_count
        );

        // 并发检查所有 providers 的 rate limit
        let check_futures = providers.into_iter().map(|provider| {
            let redis_service = Arc::clone(&self.redis_service);
            let group_clone = group_owned.clone();
            async move {
                // 无限流配置的 provider 直接通过
                if provider.max_rps <= 0 {
                    debug!("Provider {} has no rate limit (max_rps: {})", provider.id, provider.max_rps);
                    return (provider, true, false); // (provider, allowed, is_rate_limited)
                }

                let limiter = RateLimiterFactory::create_from_config(
                    redis_service,
                    provider.id.clone(),
                    provider.max_rps as u32,
                );

                match limiter.allow_request().await {
                    Ok(rate_limit_result) => {
                        if rate_limit_result.allowed {
                            crate::monitoring::increment_requests_allowed(&provider.id, &group_clone);
                            debug!(
                                "Provider {} passed rate limit check (algorithm={}, remaining={}, usage={:.1}%)",
                                provider.id,
                                limiter.algorithm_name(),
                                rate_limit_result.remaining,
                                rate_limit_result.usage_ratio * 100.0
                            );
                            (provider, true, false)
                        } else {
                            crate::monitoring::increment_requests_rejected(&provider.id, &group_clone);
                            info!(
                                "Provider {} rate limited (algorithm={}, retry_after={:?}ms)",
                                provider.id,
                                limiter.algorithm_name(),
                                rate_limit_result.retry_after_ms
                            );
                            (provider, false, true)
                        }
                    }
                    Err(e) => {
                        warn!("Rate limit check failed for provider {}: {}, allowing request", provider.id, e);
                        crate::monitoring::increment_requests_allowed(&provider.id, &group_clone);
                        (provider, true, false)
                    }
                }
            }
        });

        // 等待所有检查完成
        let results = join_all(check_futures).await;

        // 收集结果
        let mut available_providers = Vec::new();
        let mut rate_limited_count = 0;

        for (provider, allowed, is_rate_limited) in results {
            if allowed {
                available_providers.push(provider);
            } else if is_rate_limited {
                rate_limited_count += 1;
            }
        }

        // All providers are rate limited if we have providers with rate limits and all of them are limited
        let all_rate_limited =
            total_count > 0 && available_providers.is_empty() && rate_limited_count == total_count;

        Ok(RateLimitFilterResult {
            available_providers,
            total_count,
            all_rate_limited,
        })
    }

    /// Get current usage for a provider
    pub async fn get_provider_current_usage(&self, provider_id: &str, max_rps: u32) -> Option<f64> {
        let limiter = RateLimiterFactory::create_from_config(
            Arc::clone(&self.redis_service),
            provider_id.to_string(),
            max_rps,
        );

        limiter
            .get_current_usage()
            .await
            .ok()
            .map(|usage| usage.current)
    }

    /// Check capacity of providers without consuming tokens
    /// Filters out providers that are at or near capacity (utilization >= 95%)
    pub async fn check_capacity(
        &self,
        providers: Vec<ModelProvider>,
    ) -> Result<Vec<ModelProvider>> {
        let total_count = providers.len();
        debug!(
            "Checking capacity for {} providers (without consuming tokens)",
            total_count
        );

        let mut available_providers = Vec::new();

        for provider in providers {
            // Providers with no rate limit always pass
            if provider.max_rps <= 0 {
                debug!(
                    "Provider {} has no rate limit (max_rps: {})",
                    provider.id, provider.max_rps
                );
                available_providers.push(provider);
                continue;
            }

            // Query current usage without consuming token
            let current_rps = self
                .get_provider_current_usage(&provider.id, provider.max_rps as u32)
                .await
                .unwrap_or(0.0);

            let utilization = current_rps / provider.max_rps as f64;

            // Filter out providers at or near capacity (>= 95%)
            if utilization < 0.95 {
                debug!(
                    "Provider {} has capacity: current={:.1}, max={}, utilization={:.1}%",
                    provider.id,
                    current_rps,
                    provider.max_rps,
                    utilization * 100.0
                );
                available_providers.push(provider);
            } else {
                info!(
                    "Provider {} at capacity: current={:.1}, max={}, utilization={:.1}%",
                    provider.id,
                    current_rps,
                    provider.max_rps,
                    utilization * 100.0
                );
            }
        }

        debug!(
            "Capacity check: {} → {} providers available",
            total_count,
            available_providers.len()
        );
        Ok(available_providers)
    }

    /// Consume token for the selected provider
    /// Returns Ok(true) if token consumed successfully, Ok(false) if rate limited
    ///
    /// # Arguments
    /// * `provider` - The provider to consume token for
    /// * `is_retry` - Whether this is a retry request (applies surge capacity if true)
    /// * `surge_percentage` - Surge capacity percentage (default 0.3 = 30%)
    /// * `group` - The provider group for metrics
    pub async fn consume_token(
        &self,
        provider: &ModelProvider,
        is_retry: bool,
        surge_percentage: f64,
        group: &str,
    ) -> Result<bool> {
        // Providers with no rate limit always succeed
        if provider.max_rps <= 0 {
            debug!(
                "Provider {} has no rate limit, token consumption skipped",
                provider.id
            );
            return Ok(true);
        }

        // Calculate effective max_rps with surge capacity for retry requests
        let effective_max_rps = if is_retry {
            let surge_multiplier = 1.0 + surge_percentage;
            let surged_rps = (provider.max_rps as f64 * surge_multiplier) as u32;
            info!(
                "🔄 Retry request detected for provider {}, applying surge capacity: {} → {} RPS ({:.0}% surge)",
                provider.id, provider.max_rps, surged_rps, surge_percentage * 100.0
            );
            surged_rps
        } else {
            provider.max_rps as u32
        };

        let limiter = RateLimiterFactory::create_from_config(
            Arc::clone(&self.redis_service),
            provider.id.clone(),
            effective_max_rps,
        );

        match limiter.allow_request().await {
            Ok(rate_limit_result) => {
                if rate_limit_result.allowed {
                    crate::monitoring::increment_requests_allowed(&provider.id, group);
                    debug!(
                        "Token consumed for provider {} (algorithm={}, remaining={}, usage={:.1}%, is_retry={})",
                        provider.id,
                        limiter.algorithm_name(),
                        rate_limit_result.remaining,
                        rate_limit_result.usage_ratio * 100.0,
                        is_retry
                    );
                    Ok(true)
                } else {
                    crate::monitoring::increment_requests_rejected(&provider.id, group);
                    info!(
                        "Token consumption failed for provider {} - rate limited (algorithm={}, retry_after={:?}ms, is_retry={})",
                        provider.id,
                        limiter.algorithm_name(),
                        rate_limit_result.retry_after_ms,
                        is_retry
                    );
                    Ok(false)
                }
            }
            Err(e) => {
                warn!(
                    "Token consumption error for provider {}: {}",
                    provider.id, e
                );
                Err(e)
            }
        }
    }

    /// Filter providers with smart wait strategy
    /// Dynamically adjusts wait time based on remaining time and attempts
    ///
    /// # Arguments
    /// * `providers` - List of providers to filter
    /// * `group` - The provider group for metrics
    /// * `max_wait_seconds` - Maximum wait time in seconds
    pub async fn filter_with_wait_strategy(
        &self,
        providers: Vec<ModelProvider>,
        group: &str,
        max_wait_seconds: u64,
    ) -> Result<RateLimitFilterResult> {
        let start_time = Instant::now();
        let mut attempt = 0;

        loop {
            attempt += 1;
            let result = self.filter(providers.clone(), group).await?;

            // If we have available providers, return immediately
            if !result.available_providers.is_empty() {
                if attempt > 1 {
                    debug!("Found available providers after {} attempts", attempt);
                }
                return Ok(result);
            }

            // Check if we've exceeded max wait time
            let elapsed = start_time.elapsed().as_secs();
            if elapsed >= max_wait_seconds {
                if result.all_rate_limited {
                    warn!(
                        "All {} providers rate limited after {}s and {} attempts",
                        result.total_count, elapsed, attempt
                    );
                }
                return Ok(result);
            }

            // If not all providers are rate limited, return immediately
            // (some might be unavailable for other reasons)
            if !result.all_rate_limited {
                debug!(
                    "Not all providers rate limited, returning after {} attempts",
                    attempt
                );
                return Ok(result);
            }

            // Smart wait: dynamically adjust wait duration based on remaining time
            let remaining = max_wait_seconds - elapsed;
            let wait_duration = if remaining > 3 {
                // Plenty of time remaining, wait 1 second
                Duration::from_secs(1)
            } else if remaining > 1 {
                // Limited time, wait 500ms for faster response
                Duration::from_millis(500)
            } else {
                // Last chance, wait 200ms
                Duration::from_millis(200)
            };

            info!("All {} providers rate limited, waiting {:?} before retry (attempt {}, elapsed {}s)",
                  result.total_count, wait_duration, attempt, elapsed);

            sleep(wait_duration).await;
        }
    }
}
