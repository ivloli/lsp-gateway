use crate::config::RedisConfig;
use crate::error::Result;
use crate::storage::{GatewayMetrics, ProviderMetrics, RedisService};
use crate::utils::TimeUtils;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

/// Error types for detailed error tracking
#[derive(Debug, Clone, Copy)]
pub enum ErrorType {
    ServerError, // 5xx
    Timeout,     // Request timeout
    RateLimit,   // 429 rate limit
    Other,       // Other types of errors
}

/// Provider health metrics for health scoring calculation
#[derive(Debug, Clone)]
pub struct ProviderHealthMetrics {
    pub provider_id: String,
    pub latency_p95_ms: u64,      // P95延迟
    pub error_rate: f64,          // 错误率 (0.0-1.0)
    pub availability: f64,        // 可用率 (0.0-1.0)
    pub utilization: f64,         // 利用率 (0.0-1.0)
    pub token_stream_health: f64, // Token流健康度 (0.0-1.0)
}

/// Aggregated metrics for a provider during a reporting window
#[derive(Debug, Clone, Default)]
pub struct ProviderMetricsWindow {
    pub success_count: u64,
    pub fail_count: u64,
    pub latency_samples: Vec<u64>, // Store latency samples for P95 calculation
    pub token_count: u64,          // Total tokens processed
    pub request_start_time: Option<u64>, // Timestamp of first request in window
    pub model_provider_group: Option<String>, // Model provider group for metrics grouping
    // Enhanced error tracking for adaptive routing
    pub server_errors: u64,     // 5xx errors
    pub timeout_errors: u64,    // Timeout errors
    pub rate_limit_errors: u64, // 429 rate limit errors
    pub total_requests: u64,    // Total request count for error rate calculation
}

impl ProviderMetricsWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_success(&mut self) {
        self.success_count += 1;
        self.total_requests += 1;
        self.init_window_if_needed();
    }

    pub fn record_failure(&mut self) {
        self.fail_count += 1;
        self.total_requests += 1;
        self.init_window_if_needed();
    }

    /// Record a server error (5xx)
    pub fn record_server_error(&mut self) {
        self.server_errors += 1;
        self.fail_count += 1;
        self.total_requests += 1;
        self.init_window_if_needed();
    }

    /// Record a timeout error
    pub fn record_timeout_error(&mut self) {
        self.timeout_errors += 1;
        self.fail_count += 1;
        self.total_requests += 1;
        self.init_window_if_needed();
    }

    /// Record a rate limit error (429)
    pub fn record_rate_limit_error(&mut self) {
        self.rate_limit_errors += 1;
        self.fail_count += 1;
        self.total_requests += 1;
        self.init_window_if_needed();
    }

    pub fn record_latency(&mut self, latency_ms: u64) {
        // Store latency sample for P95 calculation (limit to avoid memory issues)
        if self.latency_samples.len() < 1000 {
            self.latency_samples.push(latency_ms);
        }

        self.init_window_if_needed();
    }

    pub fn record_tokens(&mut self, tokens: u64) {
        self.token_count += tokens;
        self.init_window_if_needed();
    }

    /// Initialize window timestamp if not set
    fn init_window_if_needed(&mut self) {
        if self.request_start_time.is_none() {
            self.request_start_time = Some(TimeUtils::current_timestamp_secs());
        }
    }

    /// Calculate P95 latency from samples
    pub fn calculate_latency_p95(&self) -> u64 {
        if self.latency_samples.is_empty() {
            return 0;
        }

        let mut sorted_samples = self.latency_samples.clone();
        sorted_samples.sort_unstable();

        let p95_index = ((sorted_samples.len() as f64) * 0.95) as usize;
        let index = if p95_index >= sorted_samples.len() {
            sorted_samples.len() - 1
        } else {
            p95_index
        };

        sorted_samples[index]
    }

    /// Calculate token throughput (tokens per second) for the reporting window
    pub fn calculate_token_throughput(&self) -> f64 {
        if self.token_count == 0 || self.request_start_time.is_none() {
            return 0.0;
        }

        let now = TimeUtils::current_timestamp_secs();

        let window_duration = now.saturating_sub(self.request_start_time.unwrap_or(now));

        if window_duration == 0 {
            return 0.0;
        }

        self.token_count as f64 / window_duration as f64
    }

    /// Calculate error rate (0.0-1.0)
    pub fn calculate_error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        let total_errors = self.server_errors + self.timeout_errors + self.rate_limit_errors;
        total_errors as f64 / self.total_requests as f64
    }

    /// Calculate utilization based on max RPS and window duration
    pub fn calculate_utilization(&self, max_rps: u32, window_seconds: u64) -> f64 {
        if max_rps == 0 || window_seconds == 0 {
            return 0.0;
        }
        let max_requests_in_window = max_rps as u64 * window_seconds;
        (self.total_requests as f64) / (max_requests_in_window as f64)
    }

    /// Calculate token stream health (simplified, 1.0 = healthy)
    pub fn calculate_token_stream_health(&self) -> f64 {
        // Simplified implementation - could be enhanced based on streaming metrics
        if self.total_requests == 0 {
            return 1.0;
        }
        // For now, consider healthy if no timeout errors
        1.0 - (self.timeout_errors as f64 / self.total_requests as f64)
    }

    pub fn reset(&mut self) {
        self.success_count = 0;
        self.fail_count = 0;
        self.latency_samples.clear();
        self.token_count = 0;
        self.request_start_time = None;
        self.server_errors = 0;
        self.timeout_errors = 0;
        self.rate_limit_errors = 0;
        self.total_requests = 0;
    }

    pub fn to_provider_metrics(&self, provider_id: String) -> ProviderMetrics {
        let timestamp = TimeUtils::current_timestamp_secs();

        ProviderMetrics {
            provider_id,
            model_provider_group: self.model_provider_group.clone(),
            success_count: self.success_count,
            fail_count: self.fail_count,
            latency_p95_ms: self.calculate_latency_p95(),
            token_count: self.token_count,
            token_throughput_tps: self.calculate_token_throughput(),
            timestamp,
        }
    }

    pub fn to_gateway_metrics(
        &self,
        gateway_id: String,
        provider_id: String,
        model_provider_group: Option<String>,
    ) -> GatewayMetrics {
        let timestamp = TimeUtils::current_timestamp_secs();

        GatewayMetrics {
            gateway_id,
            provider_id,
            model_provider_group,
            success_count: self.success_count,
            fail_count: self.fail_count,
            latency_p95_ms: self.calculate_latency_p95(),
            token_count: self.token_count,
            token_throughput_tps: self.calculate_token_throughput(),
            timestamp,
        }
    }

    /// Convert to health metrics for health scoring
    pub fn to_health_metrics(
        &self,
        provider_id: String,
        max_rps: u32,
        window_seconds: u64,
    ) -> ProviderHealthMetrics {
        ProviderHealthMetrics {
            provider_id,
            latency_p95_ms: self.calculate_latency_p95(),
            error_rate: self.calculate_error_rate(),
            availability: 1.0 - self.calculate_error_rate(), // 可用率 = 1 - 错误率
            utilization: self.calculate_utilization(max_rps, window_seconds),
            token_stream_health: self.calculate_token_stream_health(),
        }
    }
}

/// Metrics reporter service that collects and reports metrics to Redis
#[derive(Clone)]
pub struct MetricsReporter {
    redis_service: Arc<RedisService>,
    metrics_windows: Arc<RwLock<HashMap<String, ProviderMetricsWindow>>>,
    report_interval: Duration,
    gateway_id: String,
    prometheus_metrics: Option<Arc<crate::monitoring::PrometheusMetrics>>,
}

impl MetricsReporter {
    /// Create a new metrics reporter
    pub fn new(redis_service: Arc<RedisService>, config: &RedisConfig, gateway_id: String) -> Self {
        Self {
            redis_service,
            metrics_windows: Arc::new(RwLock::new(HashMap::new())),
            report_interval: Duration::from_secs(config.metrics_report_interval),
            gateway_id,
            prometheus_metrics: None,
        }
    }

    /// Set Prometheus metrics for integration
    pub fn with_prometheus_metrics(
        mut self,
        prometheus_metrics: Arc<crate::monitoring::PrometheusMetrics>,
    ) -> Self {
        self.prometheus_metrics = Some(prometheus_metrics);
        self
    }

    /// Record a successful request for a provider
    pub async fn record_success(&self, provider_id: &str) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_success();
        debug!("Recorded success for provider: {}", provider_id);
    }

    /// Record a failed request for a provider
    pub async fn record_failure(&self, provider_id: &str) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_failure();
        debug!("Recorded failure for provider: {}", provider_id);
    }

    /// Record request latency for a provider
    pub async fn record_latency(&self, provider_id: &str, latency_ms: u64) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_latency(latency_ms);
        debug!(
            "Recorded latency {}ms for provider: {}",
            latency_ms, provider_id
        );
    }

    /// Record request completion (success or failure with latency)
    pub async fn record_request_completion(
        &self,
        provider_id: &str,
        success: bool,
        latency_ms: u64,
        model_provider_group: Option<String>,
    ) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);

        // Update group information if provided
        if model_provider_group.is_some() {
            window.model_provider_group = model_provider_group;
        }

        if success {
            window.record_success();
        } else {
            window.record_failure();
        }
        window.record_latency(latency_ms);

        debug!(
            "Recorded request completion for provider {}: success={}, latency={}ms, group={:?}",
            provider_id, success, latency_ms, window.model_provider_group
        );
    }

    /// Record request completion with token usage
    pub async fn record_request_completion_with_tokens(
        &self,
        provider_id: &str,
        success: bool,
        latency_ms: u64,
        tokens: u64,
        model_provider_group: Option<String>,
    ) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);

        // Update group information if provided
        if model_provider_group.is_some() {
            window.model_provider_group = model_provider_group;
        }

        if success {
            window.record_success();
        } else {
            window.record_failure();
        }
        window.record_latency(latency_ms);
        window.record_tokens(tokens);

        debug!(
            "Recorded request completion for provider {}: success={}, latency={}ms, tokens={}, group={:?}",
            provider_id, success, latency_ms, tokens, window.model_provider_group
        );
    }

    /// Record tokens for a provider
    pub async fn record_tokens(&self, provider_id: &str, tokens: u64) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_tokens(tokens);
        debug!("Recorded {} tokens for provider: {}", tokens, provider_id);
    }

    /// Record a server error (5xx) for a provider
    pub async fn record_server_error(&self, provider_id: &str) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_server_error();
        debug!("Recorded server error for provider: {}", provider_id);
    }

    /// Record a timeout error for a provider
    pub async fn record_timeout_error(&self, provider_id: &str) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_timeout_error();
        debug!("Recorded timeout error for provider: {}", provider_id);
    }

    /// Record a rate limit error (429) for a provider
    pub async fn record_rate_limit_error(&self, provider_id: &str) {
        let mut windows = self.metrics_windows.write().await;
        let window = windows
            .entry(provider_id.to_string())
            .or_insert_with(ProviderMetricsWindow::new);
        window.record_rate_limit_error();
        debug!("Recorded rate limit error for provider: {}", provider_id);
    }

    /// Record error by type for a provider
    pub async fn record_error_by_type(&self, provider_id: &str, error_type: ErrorType) {
        match error_type {
            ErrorType::ServerError => self.record_server_error(provider_id).await,
            ErrorType::Timeout => self.record_timeout_error(provider_id).await,
            ErrorType::RateLimit => self.record_rate_limit_error(provider_id).await,
            ErrorType::Other => self.record_failure(provider_id).await,
        }
    }

    /// Start the background reporting task
    pub async fn start_reporting_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let reporter = Arc::clone(&self);

        tokio::spawn(async move {
            let mut interval = interval(reporter.report_interval);

            info!(
                "Starting metrics reporting task with interval: {:?}",
                reporter.report_interval
            );

            loop {
                interval.tick().await;

                if let Err(e) = reporter.report_metrics().await {
                    error!("Failed to report metrics: {}", e);
                }
            }
        })
    }

    /// Report all collected metrics to Redis and reset windows
    async fn report_metrics(&self) -> Result<()> {
        let mut windows = self.metrics_windows.write().await;

        if windows.is_empty() {
            // debug!("No metrics to report");
            return Ok(());
        }

        // debug!("Reporting metrics for {} providers", windows.len());

        // Collect all metrics and reset windows
        let mut gateway_metrics_to_report = Vec::new();
        for (provider_id, window) in windows.iter_mut() {
            // Only report if there's activity
            if window.success_count > 0 || window.fail_count > 0 {
                let gateway_metrics = window.to_gateway_metrics(
                    self.gateway_id.clone(),
                    provider_id.clone(),
                    window.model_provider_group.clone(),
                );
                gateway_metrics_to_report.push(gateway_metrics);
                window.reset();
            }
        }

        // Release the lock before making Redis calls
        drop(windows);

        // Report gateway instance metrics to Redis
        for metrics in gateway_metrics_to_report {
            if let Err(e) = self.redis_service.report_provider_metrics(&metrics).await {
                error!(
                    "Failed to report gateway metrics for gateway {} provider {}: {}",
                    metrics.gateway_id, metrics.provider_id, e
                );
                // Continue reporting other providers even if one fails
            }
        }

        // debug!("Completed metrics reporting cycle");
        Ok(())
    }

    /// Get current metrics snapshot (for debugging/monitoring)
    pub async fn get_metrics_snapshot(&self) -> HashMap<String, ProviderMetricsWindow> {
        let windows = self.metrics_windows.read().await;
        windows.clone()
    }

    /// Clear all metrics windows
    pub async fn clear_metrics(&self) {
        let mut windows = self.metrics_windows.write().await;
        windows.clear();
        info!("Cleared all metrics windows");
    }

    /// Get the number of providers being tracked
    pub async fn get_tracked_provider_count(&self) -> usize {
        let windows = self.metrics_windows.read().await;
        windows.len()
    }
}
