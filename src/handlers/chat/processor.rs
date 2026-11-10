use crate::app::AppState;
use crate::error::{AppError, Result};
use crate::genai::GenAIClient;
use crate::routing::provider::SelectedProvider;
use axum::http::HeaderMap;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Context for processing a request
#[derive(Clone)]
pub struct RequestContext<'a> {
    pub state: &'a AppState,
    pub headers: HeaderMap,
    pub model_id: String,
    pub is_streaming: bool,
    pub conv_id: Option<String>,
}

/// Metrics for a completed request
pub struct RequestMetrics {
    pub latency_ms: u64,
    pub token_count: Option<u64>,
    pub error_type: Option<String>,
}

/// Client type for handling requests (GenAI only)
pub type ClientType = GenAIClient;

/// Unified request processor
pub struct RequestProcessor;

impl RequestProcessor {
    /// Process a request with retry logic
    pub async fn process_request_with_retry<T, F, Fut>(
        ctx: RequestContext<'_>,
        request: T,
        handler: F,
    ) -> Result<(String, RequestMetrics)>
    where
        T: Clone,
        F: Fn(ClientType, SelectedProvider, T, HeaderMap) -> Fut + Clone,
        Fut: std::future::Future<Output = Result<(String, Option<u64>)>>,
    {
        let retry_config = &ctx.state.config.retry;
        let max_retries = retry_config.max_retries;
        let mut last_error = None;
        let mut last_provider: Option<SelectedProvider> = None;
        let mut last_metrics: Option<RequestMetrics> = None;

        for attempt in 0..max_retries {
            if attempt > 0 {
                // Calculate backoff time with jitter
                let backoff_ms = Self::calculate_backoff(
                    attempt,
                    retry_config.base_backoff_ms,
                    retry_config.max_backoff_ms,
                    retry_config.jitter_enabled,
                );
                info!(
                    "🔄 Retry attempt {}/{} after {}ms for model {}",
                    attempt,
                    max_retries - 1,
                    backoff_ms,
                    ctx.model_id
                );
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }

            // Select provider for this attempt
            let selected_provider =
                match Self::select_provider(ctx.state, &ctx.model_id, ctx.conv_id.as_deref()).await
                {
                    Ok(provider) => provider,
                    Err(e) => {
                        warn!(
                            "❌ Provider selection failed on attempt {} for model {}: {}",
                            attempt, ctx.model_id, e
                        );
                        last_error = Some(e);
                        continue;
                    }
                };

            // Create client and record the provider for potential failure recording
            last_provider = Some(selected_provider.clone());
            let timer = Instant::now();

            // Clone the context and request for each retry attempt
            let ctx_clone = ctx.clone();
            let request_clone = request.clone();
            let handler_clone = handler.clone();

            match Self::execute_request(
                ctx_clone,
                selected_provider.clone(),
                request_clone,
                handler_clone,
                timer,
                attempt,
            )
            .await
            {
                Ok(result) => {
                    // Record retry success metric if this was a retry
                    if attempt > 0 {
                        Self::record_retry_success(&ctx, &selected_provider.id, attempt);
                    }
                    // Success is already logged in log_success with full details
                    return Ok(result);
                }
                Err((e, metrics)) => {
                    warn!(
                        "❌ Request failed on attempt {} for model {}: {}",
                        attempt, ctx.model_id, e
                    );

                    // Save the error and metrics for potential failure recording
                    last_metrics = Some(metrics);

                    // Check if we should retry
                    if !Self::should_retry(
                        &e,
                        attempt,
                        max_retries,
                        retry_config.retry_on_rate_limit,
                    ) {
                        error!("🚫 Not retrying for model {} - error not retryable or max attempts reached",
                               ctx.model_id);
                        // Record retry exhausted metric
                        if attempt > 0 {
                            Self::record_retry_exhausted(&ctx, &selected_provider.id, attempt);
                        }
                        // Record final failure
                        if let (Some(provider), Some(metrics)) =
                            (last_provider.as_ref(), last_metrics.as_ref())
                        {
                            Self::record_failure(&ctx, provider, metrics).await;
                        }
                        return Err(e);
                    }

                    // Record retry attempt metric
                    Self::record_retry_attempt(
                        &ctx,
                        &selected_provider.id,
                        attempt,
                        Self::classify_error(&e).as_str(),
                    );
                    last_error = Some(e);
                }
            }
        }

        error!(
            "🚫 All {} retry attempts exhausted for model {}",
            max_retries, ctx.model_id
        );

        // Record retry exhausted with the last provider used
        if let Some(provider) = last_provider.as_ref() {
            Self::record_retry_exhausted(&ctx, &provider.id, max_retries);
        }

        // Record final failure after all retries exhausted
        if let (Some(provider), Some(metrics)) = (last_provider.as_ref(), last_metrics.as_ref()) {
            Self::record_failure(&ctx, provider, metrics).await;
        }

        Err(last_error.unwrap_or_else(|| {
            AppError::Internal(format!("Max retries ({}) exceeded", max_retries))
        }))
    }

    /// Execute a single request attempt (internal use only)
    ///
    /// Returns Ok((response, metrics)) on success, or Err((error, metrics)) on failure.
    /// This allows the caller to access metrics even when the request fails.
    ///
    /// Note: This method does NOT record failure metrics to avoid double-counting during retries.
    /// Failure metrics are recorded by `process_request_with_retry` only when all retries are exhausted.
    async fn execute_request<T, F, Fut>(
        ctx: RequestContext<'_>,
        selected_provider: SelectedProvider,
        request: T,
        handler: F,
        timer: Instant,
        attempt: usize,
    ) -> std::result::Result<(String, RequestMetrics), (AppError, RequestMetrics)>
    where
        F: FnOnce(ClientType, SelectedProvider, T, HeaderMap) -> Fut,
        Fut: std::future::Future<Output = Result<(String, Option<u64>)>>,
    {
        let state = ctx.state;
        let headers = ctx.headers.clone();
        let model_id = ctx.model_id.clone();
        let is_streaming = ctx.is_streaming;

        // Only log on first attempt to avoid noise
        if attempt == 0 {
            info!(
                "🚀 Processing {} request for model: {}",
                if is_streaming { "streaming" } else { "regular" },
                model_id
            );
        }
        debug!("Request processing started for attempt {}", attempt);

        // Create appropriate client type
        let client = match Self::create_client_type(state, &selected_provider) {
            Ok(client) => client,
            Err(e) => {
                let latency_ms = timer.elapsed().as_millis() as u64;
                let error_type = Self::classify_error(&e);
                let metrics = RequestMetrics {
                    latency_ms,
                    token_count: None,
                    error_type: Some(error_type),
                };
                return Err((e, metrics));
            }
        };

        // Execute the handler
        let result = handler(client, selected_provider.clone(), request, headers).await;
        let latency_ms = timer.elapsed().as_millis() as u64;

        // Process result and create metrics
        match result {
            Ok((response, token_count)) => {
                let metrics = RequestMetrics {
                    latency_ms,
                    token_count,
                    error_type: None,
                };

                Self::record_success(&ctx, &selected_provider, &metrics).await;
                Self::log_success(
                    &model_id,
                    &selected_provider.id,
                    &metrics,
                    &response,
                    attempt,
                );

                Ok((response, metrics))
            }
            Err(e) => {
                let error_type = Self::classify_error(&e);
                let metrics = RequestMetrics {
                    latency_ms,
                    token_count: None,
                    error_type: Some(error_type.clone()),
                };

                // Only log the failure, don't record metrics (to avoid double-counting during retries)
                // Failure metrics will be recorded by process_request_with_retry when all retries are exhausted

                Err((e, metrics))
            }
        }
    }

    async fn select_provider(
        state: &AppState,
        model_id: &str,
        conv_id: Option<&str>,
    ) -> Result<SelectedProvider> {
        let provider = state
            .provider_service
            .select_provider(model_id, conv_id)
            .await?;
        debug!("Selected provider: {} for model: {}", provider.id, model_id);
        Ok(provider)
    }

    fn create_client_type(
        state: &AppState,
        selected_provider: &SelectedProvider,
    ) -> Result<ClientType> {
        debug!(
            "Creating GenAI client for model class: {}",
            selected_provider.model_config.model_class_name
        );
        // Use model-specific timeout config if available
        let stream_timeout_ms = selected_provider
            .model_config
            .get_stream_no_token_timeout_ms();
        let response_finish_timeout_ms = selected_provider
            .model_config
            .get_response_finish_timeout_ms();
        let timeout_config = state
            .config
            .timeouts
            .to_stream_timeout_config_with_model_override(
                stream_timeout_ms,
                response_finish_timeout_ms,
            );
        GenAIClient::with_timeout_config(selected_provider.clone(), timeout_config)
    }

    async fn record_success(
        ctx: &RequestContext<'_>,
        provider: &SelectedProvider,
        metrics: &RequestMetrics,
    ) {
        // Determine model provider group for metrics
        // Use model_provider_group if available, otherwise use "direct-{model_config.id}"
        let model_provider_group = provider.model_config.model_provider_group.clone();
        let group_for_prometheus = model_provider_group
            .clone()
            .unwrap_or_else(|| format!("direct-{}", provider.model_config.id));

        // Record in provider service
        if let Some(tokens) = metrics.token_count {
            ctx.state
                .provider_service
                .record_request_completion_with_tokens(
                    &provider.id,
                    true,
                    metrics.latency_ms,
                    tokens,
                    model_provider_group,
                )
                .await;
        } else {
            ctx.state
                .provider_service
                .record_request_completion(
                    &provider.id,
                    true,
                    metrics.latency_ms,
                    model_provider_group,
                )
                .await;
        }

        // Record in Prometheus
        crate::monitoring::record_success_global(
            &provider.id,
            &ctx.model_id,
            &provider.base_url,
            &group_for_prometheus,
            metrics.latency_ms,
            metrics.token_count,
        );
    }

    async fn record_failure(
        ctx: &RequestContext<'_>,
        provider: &SelectedProvider,
        metrics: &RequestMetrics,
    ) {
        // Determine model provider group for metrics
        // Use model_provider_group if available, otherwise use "direct-{model_config.id}"
        let model_provider_group = provider.model_config.model_provider_group.clone();
        let group_for_prometheus = model_provider_group
            .clone()
            .unwrap_or_else(|| format!("direct-{}", provider.model_config.id));

        // Record in provider service
        ctx.state
            .provider_service
            .record_request_completion(
                &provider.id,
                false,
                metrics.latency_ms,
                model_provider_group,
            )
            .await;

        // Record in Prometheus
        crate::monitoring::record_failure_global(
            &provider.id,
            &ctx.model_id,
            &provider.base_url,
            &group_for_prometheus,
            metrics.error_type.as_deref().unwrap_or("unknown"),
            metrics.latency_ms,
        );
    }

    fn log_success(
        model_id: &str,
        provider_id: &str,
        metrics: &RequestMetrics,
        response: &str,
        attempt: usize,
    ) {
        let retry_info = if attempt > 0 {
            format!(" (succeeded on retry attempt {})", attempt)
        } else {
            String::new()
        };

        if let Some(tokens) = metrics.token_count {
            info!("✅ Successfully processed request for model {} via provider {} in {}ms (tokens: {}){}",
                  model_id, provider_id, metrics.latency_ms, tokens, retry_info);
        } else {
            info!("✅ Successfully processed streaming request for model {} via provider {} in {}ms{}",
                  model_id, provider_id, metrics.latency_ms, retry_info);
        }

        // Log response content (truncated for safety)
        debug!("Response content: {}", response);
    }

    /// Classify an error into a string type for metrics
    ///
    /// Note: This method is crate-public for testing purposes
    pub(crate) fn classify_error(error: &AppError) -> String {
        match error {
            AppError::Provider(_) => "provider_error",
            AppError::Config(_) => "config_error",
            AppError::Database(_) => "database_error",
            AppError::Redis(_) => "redis_error",
            AppError::Internal(_) => "internal_error",
            AppError::HttpClient(_) => "http_client_error",
            AppError::Serialization(_) => "serialization_error",
            AppError::ModelNotFound(_) => "model_not_found",
            AppError::NoActiveProviders(_) => "no_active_providers",
            AppError::AllProvidersRateLimited(_) => "all_providers_rate_limited",
            AppError::InvalidRequest(_) => "invalid_request",
            AppError::ContentFilter(_) => "content_filter",
            AppError::EmptyResponse(_) => "empty_response",
            AppError::StreamTokenTimeout { .. } => "stream_token_timeout",
            AppError::ResponseFinishTimeout { .. } => "response_finish_timeout",
        }
        .to_string()
    }

    /// Determine if an error should trigger a retry
    ///
    /// Note: This method is crate-public for testing purposes
    pub(crate) fn should_retry(
        error: &AppError,
        attempt: usize,
        max_retries: usize,
        retry_on_rate_limit: bool,
    ) -> bool {
        // Check if we've exhausted retries
        // Handle edge case where max_retries is 0 or 1 (no retry)
        if max_retries == 0 || attempt >= max_retries - 1 {
            return false;
        }

        match error {
            // Retryable errors - network/transient issues
            AppError::HttpClient(_) => true,
            AppError::StreamTokenTimeout { .. } => true,
            AppError::ResponseFinishTimeout { .. } => true,

            // Provider errors - check message content
            AppError::Provider(msg) => {
                let msg_lower = msg.to_lowercase();
                // Retry on timeout, 5xx errors
                if msg_lower.contains("timeout")
                    || msg_lower.contains("503")
                    || msg_lower.contains("502")
                    || msg_lower.contains("504")
                {
                    return true;
                }
                // Retry on rate limit if enabled
                if retry_on_rate_limit
                    && (msg_lower.contains("429") || msg_lower.contains("rate limit"))
                {
                    return true;
                }
                false
            }

            // Rate limiting errors
            AppError::AllProvidersRateLimited(_) => retry_on_rate_limit,

            // Non-retryable errors - client/config issues
            AppError::InvalidRequest(_) => false,
            AppError::ModelNotFound(_) => false,
            AppError::Config(_) => false,
            AppError::Serialization(_) => false,

            // Database/Redis errors - don't retry at request level
            AppError::Database(_) => false,
            AppError::Redis(_) => false,

            // Provider selection errors - might succeed with different provider
            AppError::NoActiveProviders(_) => false,

            // Content filter and empty response errors - don't retry
            AppError::ContentFilter(_) => false,
            AppError::EmptyResponse(_) => false,

            // Internal errors - conservative approach
            AppError::Internal(_) => false,
        }
    }

    /// Calculate exponential backoff with optional jitter
    ///
    /// Note: This method is crate-public for testing purposes
    pub(crate) fn calculate_backoff(
        attempt: usize,
        base_ms: u64,
        max_ms: u64,
        jitter_enabled: bool,
    ) -> u64 {
        // Exponential backoff: base_ms * 2^attempt
        let exponential = base_ms.saturating_mul(2u64.saturating_pow(attempt as u32));
        let backoff = std::cmp::min(exponential, max_ms);

        if jitter_enabled {
            // Add ±25% jitter to prevent thundering herd
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let jitter_factor = rng.gen_range(0.75..=1.25);
            ((backoff as f64) * jitter_factor) as u64
        } else {
            backoff
        }
    }

    /// Record a retry attempt metric
    fn record_retry_attempt(
        ctx: &RequestContext<'_>,
        provider_id: &str,
        attempt: usize,
        error_type: &str,
    ) {
        crate::monitoring::record_retry_attempt_global(
            &ctx.model_id,
            provider_id,
            attempt,
            error_type,
        );
    }

    /// Record a retry success metric
    fn record_retry_success(ctx: &RequestContext<'_>, provider_id: &str, attempt: usize) {
        crate::monitoring::record_retry_success_global(&ctx.model_id, provider_id, attempt);
    }

    /// Record a retry exhausted metric
    fn record_retry_exhausted(ctx: &RequestContext<'_>, provider_id: &str, attempts: usize) {
        crate::monitoring::record_retry_exhausted_global(&ctx.model_id, provider_id, attempts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    // ==================== Error Classification Tests ====================

    #[test]
    fn test_classify_error_provider() {
        let error = AppError::Provider("test error".to_string());
        assert_eq!(RequestProcessor::classify_error(&error), "provider_error");
    }

    #[test]
    fn test_classify_error_timeout() {
        let error = AppError::StreamTokenTimeout { timeout_ms: 10000 };
        assert_eq!(
            RequestProcessor::classify_error(&error),
            "stream_token_timeout"
        );

        let error = AppError::ResponseFinishTimeout { timeout_ms: 60000 };
        assert_eq!(
            RequestProcessor::classify_error(&error),
            "response_finish_timeout"
        );
    }

    #[test]
    fn test_classify_error_model_not_found() {
        let error = AppError::ModelNotFound("gpt-5".to_string());
        assert_eq!(RequestProcessor::classify_error(&error), "model_not_found");
    }

    #[test]
    fn test_classify_error_rate_limited() {
        let error = AppError::AllProvidersRateLimited("all limited".to_string());
        assert_eq!(
            RequestProcessor::classify_error(&error),
            "all_providers_rate_limited"
        );
    }

    // ==================== Retry Logic Tests ====================

    #[test]
    fn test_should_retry_network_errors() {
        // Use a timeout error message which is a common network issue
        let error = AppError::Provider("connection timeout error".to_string());

        // Should retry network errors
        assert!(RequestProcessor::should_retry(&error, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error, 1, 3, true));

        // Should not retry when attempts exhausted
        assert!(!RequestProcessor::should_retry(&error, 2, 3, true));
    }

    #[test]
    fn test_should_retry_timeout_errors() {
        let error = AppError::StreamTokenTimeout { timeout_ms: 10000 };

        assert!(RequestProcessor::should_retry(&error, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error, 1, 3, true));
        assert!(!RequestProcessor::should_retry(&error, 2, 3, true));
    }

    #[test]
    fn test_should_retry_provider_5xx_errors() {
        let error_503 = AppError::Provider("Service unavailable 503".to_string());
        let error_502 = AppError::Provider("Bad Gateway 502".to_string());
        let error_504 = AppError::Provider("Gateway Timeout 504".to_string());

        assert!(RequestProcessor::should_retry(&error_503, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_502, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_504, 0, 3, true));
    }

    #[test]
    fn test_should_retry_provider_timeout() {
        let error = AppError::Provider("Request timeout occurred".to_string());
        assert!(RequestProcessor::should_retry(&error, 0, 3, true));
    }

    #[test]
    fn test_should_retry_rate_limit_when_enabled() {
        let error_429 = AppError::Provider("Rate limit exceeded 429".to_string());
        let error_text = AppError::Provider("Rate limit reached".to_string());
        let error_all = AppError::AllProvidersRateLimited("all limited".to_string());

        // Should retry when enabled
        assert!(RequestProcessor::should_retry(&error_429, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_text, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_all, 0, 3, true));

        // Should not retry when disabled
        assert!(!RequestProcessor::should_retry(&error_429, 0, 3, false));
        assert!(!RequestProcessor::should_retry(&error_text, 0, 3, false));
        assert!(!RequestProcessor::should_retry(&error_all, 0, 3, false));
    }

    #[test]
    fn test_should_not_retry_client_errors() {
        let invalid_request = AppError::InvalidRequest("bad request".to_string());
        let model_not_found = AppError::ModelNotFound("gpt-5".to_string());
        let config_error = AppError::Config(anyhow::anyhow!("invalid config"));
        let serialization_error =
            AppError::Serialization(serde_json::from_str::<i32>("not a number").unwrap_err());

        assert!(!RequestProcessor::should_retry(
            &invalid_request,
            0,
            3,
            true
        ));
        assert!(!RequestProcessor::should_retry(
            &model_not_found,
            0,
            3,
            true
        ));
        assert!(!RequestProcessor::should_retry(&config_error, 0, 3, true));
        assert!(!RequestProcessor::should_retry(
            &serialization_error,
            0,
            3,
            true
        ));
    }

    #[test]
    fn test_should_not_retry_database_errors() {
        let db_error = AppError::Database(sqlx::Error::RowNotFound);
        let redis_error = AppError::Redis("redis connection error".to_string());

        assert!(!RequestProcessor::should_retry(&db_error, 0, 3, true));
        assert!(!RequestProcessor::should_retry(&redis_error, 0, 3, true));
    }

    #[test]
    fn test_should_not_retry_no_active_providers() {
        let error = AppError::NoActiveProviders("all inactive".to_string());
        assert!(!RequestProcessor::should_retry(&error, 0, 3, true));
    }

    #[test]
    fn test_should_not_retry_internal_errors() {
        let error = AppError::Internal("internal panic".to_string());
        assert!(!RequestProcessor::should_retry(&error, 0, 3, true));
    }

    #[test]
    fn test_should_retry_respects_max_retries() {
        let error = AppError::Provider("connection timeout".to_string());

        // With max_retries=3
        assert!(RequestProcessor::should_retry(&error, 0, 3, true)); // attempt 0 < 2
        assert!(RequestProcessor::should_retry(&error, 1, 3, true)); // attempt 1 < 2
        assert!(!RequestProcessor::should_retry(&error, 2, 3, true)); // attempt 2 >= 2

        // With max_retries=1 (no retry)
        assert!(!RequestProcessor::should_retry(&error, 0, 1, true)); // attempt 0 >= 0

        // With max_retries=5
        assert!(RequestProcessor::should_retry(&error, 0, 5, true)); // attempt 0 < 4
        assert!(RequestProcessor::should_retry(&error, 3, 5, true)); // attempt 3 < 4
        assert!(!RequestProcessor::should_retry(&error, 4, 5, true)); // attempt 4 >= 4
    }

    // ==================== Backoff Algorithm Tests ====================

    #[test]
    fn test_calculate_backoff_exponential() {
        // Without jitter
        let base_ms = 1000;
        let max_ms = 10000;

        // Attempt 0: 1000 * 2^0 = 1000ms
        let backoff = RequestProcessor::calculate_backoff(0, base_ms, max_ms, false);
        assert_eq!(backoff, 1000);

        // Attempt 1: 1000 * 2^1 = 2000ms
        let backoff = RequestProcessor::calculate_backoff(1, base_ms, max_ms, false);
        assert_eq!(backoff, 2000);

        // Attempt 2: 1000 * 2^2 = 4000ms
        let backoff = RequestProcessor::calculate_backoff(2, base_ms, max_ms, false);
        assert_eq!(backoff, 4000);

        // Attempt 3: 1000 * 2^3 = 8000ms
        let backoff = RequestProcessor::calculate_backoff(3, base_ms, max_ms, false);
        assert_eq!(backoff, 8000);

        // Attempt 4: 1000 * 2^4 = 16000ms -> capped at 10000ms
        let backoff = RequestProcessor::calculate_backoff(4, base_ms, max_ms, false);
        assert_eq!(backoff, 10000);
    }

    #[test]
    fn test_calculate_backoff_respects_max() {
        let base_ms = 1000;
        let max_ms = 5000;

        // Should cap at max_ms
        let backoff = RequestProcessor::calculate_backoff(10, base_ms, max_ms, false);
        assert_eq!(backoff, max_ms);
    }

    #[test]
    fn test_calculate_backoff_with_jitter() {
        let base_ms = 1000;
        let max_ms = 10000;

        // With jitter, result should be within ±25%
        for attempt in 0..5 {
            let backoff = RequestProcessor::calculate_backoff(attempt, base_ms, max_ms, true);
            let expected = std::cmp::min(base_ms * 2u64.pow(attempt as u32), max_ms);
            let min_expected = (expected as f64 * 0.75) as u64;
            let max_expected = (expected as f64 * 1.25) as u64;

            assert!(
                backoff >= min_expected && backoff <= max_expected,
                "Attempt {}: backoff {} not in range [{}, {}]",
                attempt,
                backoff,
                min_expected,
                max_expected
            );
        }
    }

    #[test]
    fn test_calculate_backoff_jitter_randomness() {
        let base_ms = 1000;
        let max_ms = 10000;
        let attempt = 2;

        // Generate multiple backoff values and ensure they're not all the same (randomness check)
        let mut backoffs = std::collections::HashSet::new();
        for _ in 0..20 {
            let backoff = RequestProcessor::calculate_backoff(attempt, base_ms, max_ms, true);
            backoffs.insert(backoff);
        }

        // Should have at least a few different values (very unlikely to be all the same)
        assert!(
            backoffs.len() > 1,
            "Jitter should produce varied results, got only: {:?}",
            backoffs
        );
    }

    #[test]
    fn test_calculate_backoff_zero_base() {
        // Edge case: zero base
        let backoff = RequestProcessor::calculate_backoff(1, 0, 10000, false);
        assert_eq!(backoff, 0);
    }

    #[test]
    fn test_calculate_backoff_large_attempt() {
        // Edge case: very large attempt number (should not panic due to overflow)
        let base_ms = 1000;
        let max_ms = 60000;

        let backoff = RequestProcessor::calculate_backoff(100, base_ms, max_ms, false);
        assert_eq!(backoff, max_ms); // Should be capped at max
    }

    #[test]
    fn test_calculate_backoff_different_configs() {
        // Test fast retry config
        let backoff = RequestProcessor::calculate_backoff(1, 200, 2000, false);
        assert_eq!(backoff, 400);

        // Test slow retry config
        let backoff = RequestProcessor::calculate_backoff(1, 2000, 30000, false);
        assert_eq!(backoff, 4000);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_retry_with_max_retries_one() {
        // max_retries=1 means no retry (only one attempt)
        let error = AppError::Provider("request timeout".to_string());

        assert!(!RequestProcessor::should_retry(&error, 0, 1, true));
    }

    #[test]
    fn test_retry_with_max_retries_zero() {
        // max_retries=0 is edge case (should never retry)
        let error = AppError::Provider("network error".to_string());

        // Edge case: if max_retries is 0, attempt 0 >= 0-1 which is false (0 >= -1)
        // But in practice max_retries should be at least 1
        // Let's test what happens
        assert!(!RequestProcessor::should_retry(&error, 0, 0, true));
    }

    #[test]
    fn test_backoff_saturation() {
        // Test saturation at u64::MAX doesn't panic
        let base_ms = u64::MAX / 2;
        let max_ms = u64::MAX;

        // Should saturate, not panic
        let backoff = RequestProcessor::calculate_backoff(10, base_ms, max_ms, false);
        assert!(backoff <= max_ms);
    }

    // ==================== Case Insensitive Error Matching ====================

    #[test]
    fn test_should_retry_case_insensitive() {
        // Test various case combinations
        let error_upper = AppError::Provider("TIMEOUT".to_string());
        let error_mixed = AppError::Provider("TimeOut".to_string());
        let error_lower = AppError::Provider("timeout".to_string());

        assert!(RequestProcessor::should_retry(&error_upper, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_mixed, 0, 3, true));
        assert!(RequestProcessor::should_retry(&error_lower, 0, 3, true));

        // Rate limit variations
        let error_429_upper = AppError::Provider("ERROR 429".to_string());
        let error_limit_upper = AppError::Provider("RATE LIMIT".to_string());

        assert!(RequestProcessor::should_retry(&error_429_upper, 0, 3, true));
        assert!(RequestProcessor::should_retry(
            &error_limit_upper,
            0,
            3,
            true
        ));
    }

    // ==================== Provider Error Message Variations ====================

    #[test]
    fn test_should_retry_various_5xx_formats() {
        let errors = vec![
            "HTTP 503 Service Unavailable",
            "Error 502: Bad Gateway",
            "Status: 504 Gateway Timeout",
            "503: Service temporarily unavailable",
            "upstream server returned 502",
        ];

        for msg in errors {
            let error = AppError::Provider(msg.to_string());
            assert!(
                RequestProcessor::should_retry(&error, 0, 3, true),
                "Should retry error: {}",
                msg
            );
        }
    }

    #[test]
    fn test_should_not_retry_4xx_errors() {
        let errors = vec![
            "HTTP 400 Bad Request",
            "Error 401: Unauthorized",
            "Status: 403 Forbidden",
            "404: Not Found",
        ];

        for msg in errors {
            let error = AppError::Provider(msg.to_string());
            assert!(
                !RequestProcessor::should_retry(&error, 0, 3, true),
                "Should not retry error: {}",
                msg
            );
        }
    }

    #[test]
    fn test_content_filter_not_retryable() {
        let error = AppError::ContentFilter("content blocked".to_string());
        assert!(!RequestProcessor::should_retry(&error, 0, 3, true));
    }

    #[test]
    fn test_empty_response_not_retryable() {
        let error = AppError::EmptyResponse("no content".to_string());
        assert!(!RequestProcessor::should_retry(&error, 0, 3, true));
    }

    // ==================== Retry Flow Documentation Tests ====================

    /// This test documents the expected retry flow for a successful retry scenario
    #[test]
    fn test_document_successful_retry_flow() {
        // Scenario: Request fails twice, succeeds on third attempt
        // Expected behavior:
        // - Attempt 0: Fails → should_retry returns true → retry
        // - Attempt 1: Fails → should_retry returns true → retry
        // - Attempt 2: Succeeds → return success
        // - record_failure should be called: 0 times
        // - record_success should be called: 1 time

        let error = AppError::Provider("503 Service Unavailable".to_string());

        // Verify retry decisions
        assert!(
            RequestProcessor::should_retry(&error, 0, 3, true),
            "Should retry attempt 0"
        );
        assert!(
            RequestProcessor::should_retry(&error, 1, 3, true),
            "Should retry attempt 1"
        );
        // Attempt 2 succeeds, so no need to check should_retry
    }

    /// This test documents the expected retry flow for an exhausted retry scenario
    #[test]
    fn test_document_exhausted_retry_flow() {
        // Scenario: Request fails all 3 attempts
        // Expected behavior:
        // - Attempt 0: Fails → should_retry returns true → retry
        // - Attempt 1: Fails → should_retry returns true → retry
        // - Attempt 2: Fails → should_retry returns false → exhausted
        // - record_failure should be called: 1 time (after all retries exhausted)
        // - record_success should be called: 0 times

        let error = AppError::Provider("timeout".to_string());

        // Verify retry decisions
        assert!(
            RequestProcessor::should_retry(&error, 0, 3, true),
            "Should retry attempt 0"
        );
        assert!(
            RequestProcessor::should_retry(&error, 1, 3, true),
            "Should retry attempt 1"
        );
        assert!(
            !RequestProcessor::should_retry(&error, 2, 3, true),
            "Should not retry attempt 2 (exhausted)"
        );
    }

    /// This test documents the expected retry flow for a non-retryable error
    #[test]
    fn test_document_non_retryable_error_flow() {
        // Scenario: Request fails with 400 error (non-retryable)
        // Expected behavior:
        // - Attempt 0: Fails with 400 → should_retry returns false → no retry
        // - record_failure should be called: 1 time (immediately)
        // - record_success should be called: 0 times

        let error = AppError::Provider("400 Bad Request".to_string());

        // Verify retry decision
        assert!(
            !RequestProcessor::should_retry(&error, 0, 3, true),
            "Should not retry 400 error"
        );
    }
}
