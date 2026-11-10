use prometheus::{
    CounterVec, Error as PrometheusError, GaugeVec, HistogramVec, Opts, Registry, TextEncoder,
};
use std::sync::OnceLock;
use tracing::{debug, info, warn};

/// Global Prometheus metrics instance
static GLOBAL_PROMETHEUS_METRICS: OnceLock<PrometheusMetrics> = OnceLock::new();

/// Simplified Prometheus metrics for provider monitoring
pub struct PrometheusMetrics {
    registry: Registry,

    requests_total: CounterVec,

    // Request duration histogram with status
    request_duration_seconds: HistogramVec,

    // Error details counter
    request_errors_total: CounterVec,

    // Token usage counter
    tokens_total: CounterVec,

    // Unified rate limiting counter
    rate_limit_total: CounterVec,

    // Unified provider status gauge
    provider_status: GaugeVec,

    // === NEW METRICS FOR ROUTING OBSERVABILITY ===

    // Available providers count by tier and group
    available_providers: GaugeVec,

    // Provider utilization (RPS usage ratio)
    provider_utilization: GaugeVec,

    // Tier failover events counter
    tier_failover_total: CounterVec,

    // Provider selection counter
    provider_selection_total: CounterVec,

    // Time to first token (TTFT) histogram for streaming requests
    ttft_seconds: HistogramVec,

    // Time to first byte to client (TTFB to client) histogram for streaming requests
    ttfb_to_client_seconds: HistogramVec,
}

impl PrometheusMetrics {
    /// Initialize Prometheus metrics
    pub fn new() -> Result<Self, PrometheusError> {
        let registry = Registry::new();

        // Create unified request counter with status label
        let requests_total = CounterVec::new(
            Opts::new(
                "lsp_requests_total",
                "Total number of requests by provider, model, status and group",
            ),
            &["provider", "model", "status", "group"],
        )?;

        // Create request duration histogram with status
        let request_duration_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "lsp_request_duration_seconds",
                "Request duration histogram by provider, model, status, base_url and group",
            ),
            &["provider", "model", "status", "base_url", "group"],
        )?;

        // Create error details counter
        let request_errors_total = CounterVec::new(
            Opts::new(
                "lsp_request_errors_total",
                "Total number of errors by provider, model, error type and group",
            ),
            &["provider", "model", "error_type", "group"],
        )?;

        // Create token usage counter
        let tokens_total = CounterVec::new(
            Opts::new(
                "lsp_tokens_total",
                "Total number of tokens processed by provider, model and group",
            ),
            &["provider", "model", "group"],
        )?;

        // Create unified rate limiting counter
        let rate_limit_total = CounterVec::new(
            Opts::new(
                "lsp_rate_limit_total",
                "Total rate limiting actions by provider, action and group",
            ),
            &["provider", "action", "group"],
        )?;

        // Create unified provider status gauge
        let provider_status = GaugeVec::new(
            Opts::new(
                "lsp_provider_status",
                "Provider status metrics (health, availability, circuit state, weight)",
            ),
            &["provider", "metric", "group"],
        )?;

        // === NEW METRICS FOR ROUTING OBSERVABILITY ===

        // Available providers count by tier and group
        let available_providers = GaugeVec::new(
            Opts::new(
                "lsp_available_providers",
                "Number of available providers by tier and group",
            ),
            &["tier", "group"],
        )?;

        // Provider utilization (RPS usage ratio 0.0-1.0)
        let provider_utilization = GaugeVec::new(
            Opts::new(
                "lsp_provider_utilization",
                "Provider RPS utilization ratio (0.0-1.0)",
            ),
            &["provider", "group"],
        )?;

        // Tier failover events counter
        let tier_failover_total = CounterVec::new(
            Opts::new(
                "lsp_tier_failover_total",
                "Total tier failover events by source tier, target tier and group",
            ),
            &["from_tier", "to_tier", "group"],
        )?;

        // Provider selection counter
        let provider_selection_total = CounterVec::new(
            Opts::new(
                "lsp_provider_selection_total",
                "Total provider selections by provider, tier and group",
            ),
            &["provider", "tier", "group"],
        )?;

        // Time to first token (TTFT) histogram for streaming requests
        let ttft_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "lsp_ttft_seconds",
                "Time to first token in seconds for streaming requests",
            )
            .buckets(vec![0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0]),
            &["provider", "model", "group"],
        )?;

        // Time to first byte to client (TTFB to client) histogram for streaming requests
        let ttfb_to_client_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "lsp_ttfb_to_client_seconds",
                "Time to first byte sent to client in seconds for streaming requests",
            )
            .buckets(vec![0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0]),
            &["provider", "model", "group"],
        )?;

        // Register all metrics
        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(request_duration_seconds.clone()))?;
        registry.register(Box::new(request_errors_total.clone()))?;
        registry.register(Box::new(tokens_total.clone()))?;
        registry.register(Box::new(rate_limit_total.clone()))?;
        registry.register(Box::new(provider_status.clone()))?;
        registry.register(Box::new(available_providers.clone()))?;
        registry.register(Box::new(provider_utilization.clone()))?;
        registry.register(Box::new(tier_failover_total.clone()))?;
        registry.register(Box::new(provider_selection_total.clone()))?;
        registry.register(Box::new(ttft_seconds.clone()))?;
        registry.register(Box::new(ttfb_to_client_seconds.clone()))?;

        info!("Prometheus metrics initialized successfully");

        Ok(Self {
            registry,
            requests_total,
            request_duration_seconds,
            request_errors_total,
            tokens_total,
            rate_limit_total,
            provider_status,
            available_providers,
            provider_utilization,
            tier_failover_total,
            provider_selection_total,
            ttft_seconds,
            ttfb_to_client_seconds,
        })
    }

    /// Record a successful request
    pub fn record_success(
        &self,
        provider_id: &str,
        model_id: &str,
        base_url: &str,
        group: &str,
        latency_ms: u64,
        tokens: Option<u64>,
    ) {
        // Record request with success status
        self.requests_total
            .with_label_values(&[provider_id, model_id, "success", group])
            .inc();

        // Record latency with success status and base_url
        self.request_duration_seconds
            .with_label_values(&[provider_id, model_id, "success", base_url, group])
            .observe(latency_ms as f64 / 1000.0);

        // Record token usage if available
        if let Some(token_count) = tokens {
            self.tokens_total
                .with_label_values(&[provider_id, model_id, group])
                .inc_by(token_count as f64);
        }

        debug!("Recorded success for provider: {}, model: {}, base_url: {}, group: {}, latency: {}ms, tokens: {:?}", 
               provider_id, model_id, base_url, group, latency_ms, tokens);
    }

    /// Record a failed request with error type
    pub fn record_failure(
        &self,
        provider_id: &str,
        model_id: &str,
        base_url: &str,
        group: &str,
        error_type: &str,
        latency_ms: u64,
    ) {
        // Record request with failed status
        self.requests_total
            .with_label_values(&[provider_id, model_id, "failed", group])
            .inc();

        // Record error details
        self.request_errors_total
            .with_label_values(&[provider_id, model_id, error_type, group])
            .inc();

        // Record latency with failed status and base_url
        self.request_duration_seconds
            .with_label_values(&[provider_id, model_id, "failed", base_url, group])
            .observe(latency_ms as f64 / 1000.0);

        debug!("Recorded failure for provider: {}, model: {}, base_url: {}, group: {}, error_type: {}, latency: {}ms", 
               provider_id, model_id, base_url, group, error_type, latency_ms);
    }

    /// Update provider health metrics
    pub fn update_health_metrics(&self, provider_id: &str, health_score: f64, availability: f64) {
        self.provider_status
            .with_label_values(&[provider_id, "health_score", ""])
            .set(health_score);

        self.provider_status
            .with_label_values(&[provider_id, "availability", ""])
            .set(availability);

        debug!(
            "Updated health metrics for provider: {}, health: {:.3}, availability: {:.3}",
            provider_id, health_score, availability
        );
    }

    /// Update circuit breaker state
    pub fn update_circuit_state(&self, provider_id: &str, circuit_state: &str) {
        let state_value = match circuit_state {
            "closed" => 0.0,
            "halfopen" => 1.0,
            "open" => 2.0,
            _ => -1.0,
        };

        self.provider_status
            .with_label_values(&[provider_id, "circuit_state", ""])
            .set(state_value);

        debug!(
            "Updated circuit state for provider: {}, state: {} ({})",
            provider_id, circuit_state, state_value
        );
    }

    /// Update provider weight
    pub fn update_provider_weight(&self, provider_id: &str, group: &str, weight: i32) {
        self.provider_status
            .with_label_values(&[provider_id, "weight", group])
            .set(weight as f64);

        debug!(
            "Updated provider weight for {}: weight={}, group={}",
            provider_id, weight, group
        );
    }

    /// Update dynamic weights (original and supervisor weights)
    pub fn update_dynamic_weights(
        &self,
        provider_id: &str,
        group: &str,
        original_weight: i32,
        supervisor_weight: Option<i32>,
    ) {
        // Always update original weight
        self.provider_status
            .with_label_values(&[provider_id, "original_weight", group])
            .set(original_weight as f64);

        // Update supervisor weight if present, otherwise set to 0 to indicate no override
        let supervisor_value = supervisor_weight.unwrap_or(0) as f64;
        self.provider_status
            .with_label_values(&[provider_id, "supervisor_weight", group])
            .set(supervisor_value);

        debug!(
            "Updated dynamic weights for {}: original_weight={}, supervisor_weight={:?}, group={}",
            provider_id, original_weight, supervisor_weight, group
        );
    }

    /// Increment allowed requests counter for a provider
    pub fn increment_requests_allowed(&self, provider_id: &str, group: &str) {
        self.rate_limit_total
            .with_label_values(&[provider_id, "allowed", group])
            .inc();

        debug!(
            "Incremented allowed requests for {} in group {}",
            provider_id, group
        );
    }

    /// Increment rejected requests counter for a provider
    pub fn increment_requests_rejected(&self, provider_id: &str, group: &str) {
        self.rate_limit_total
            .with_label_values(&[provider_id, "rejected", group])
            .inc();

        debug!(
            "Incremented rejected requests for {} in group {}",
            provider_id, group
        );
    }

    // === NEW METHODS FOR ROUTING OBSERVABILITY ===

    /// Update available providers count for a tier and group
    pub fn update_available_providers(&self, tier: i32, group: &str, count: usize) {
        self.available_providers
            .with_label_values(&[&tier.to_string(), group])
            .set(count as f64);

        debug!(
            "Updated available providers: tier={}, group={}, count={}",
            tier, group, count
        );
    }

    /// Update provider utilization (RPS usage ratio)
    pub fn update_provider_utilization(&self, provider_id: &str, group: &str, utilization: f64) {
        self.provider_utilization
            .with_label_values(&[provider_id, group])
            .set(utilization);

        debug!(
            "Updated provider utilization: provider={}, group={}, utilization={:.2}",
            provider_id, group, utilization
        );
    }

    /// Record a tier failover event
    pub fn record_tier_failover(&self, from_tier: i32, to_tier: i32, group: &str) {
        self.tier_failover_total
            .with_label_values(&[&from_tier.to_string(), &to_tier.to_string(), group])
            .inc();

        info!(
            "Recorded tier failover: {} -> {} for group {}",
            from_tier, to_tier, group
        );
    }

    /// Record a provider selection event
    pub fn record_provider_selection(&self, provider_id: &str, tier: i32, group: &str) {
        self.provider_selection_total
            .with_label_values(&[provider_id, &tier.to_string(), group])
            .inc();

        debug!(
            "Recorded provider selection: provider={}, tier={}, group={}",
            provider_id, tier, group
        );
    }

    /// Record time to first token (TTFT) for streaming requests
    pub fn record_ttft(&self, provider_id: &str, model_id: &str, group: &str, ttft_seconds: f64) {
        self.ttft_seconds
            .with_label_values(&[provider_id, model_id, group])
            .observe(ttft_seconds);

        debug!(
            "Recorded TTFT: provider={}, model={}, group={}, ttft={:.3}s",
            provider_id, model_id, group, ttft_seconds
        );
    }

    /// Record time to first byte to client (TTFB to client) for streaming requests
    pub fn record_ttfb_to_client(
        &self,
        provider_id: &str,
        model_id: &str,
        group: &str,
        ttfb_seconds: f64,
    ) {
        self.ttfb_to_client_seconds
            .with_label_values(&[provider_id, model_id, group])
            .observe(ttfb_seconds);

        debug!(
            "Recorded TTFB to client: provider={}, model={}, group={}, ttfb={:.3}s",
            provider_id, model_id, group, ttfb_seconds
        );
    }

    /// Get metrics in Prometheus format
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();

        match encoder.encode_to_string(&metric_families) {
            Ok(output) => output,
            Err(e) => {
                warn!("Failed to encode Prometheus metrics: {}", e);
                String::new()
            }
        }
    }

    /// Get the registry for custom integrations
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for PrometheusMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to initialize Prometheus metrics")
    }
}

/// Initialize global Prometheus metrics
pub fn init_global_prometheus_metrics() -> Result<(), PrometheusError> {
    let metrics = PrometheusMetrics::new()?;
    GLOBAL_PROMETHEUS_METRICS.set(metrics).map_err(|_| {
        PrometheusError::Msg("Global Prometheus metrics already initialized".into())
    })?;
    info!("Global Prometheus metrics initialized successfully");
    Ok(())
}

/// Get global Prometheus metrics instance
pub fn get_global_prometheus_metrics() -> Option<&'static PrometheusMetrics> {
    GLOBAL_PROMETHEUS_METRICS.get()
}

/// Update provider weight globally
pub fn update_provider_weight_global(provider_id: &str, group: &str, weight: i32) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_provider_weight(provider_id, group, weight);
    }
}

/// Update dynamic weights globally
pub fn update_dynamic_weights_global(
    provider_id: &str,
    group: &str,
    original_weight: i32,
    supervisor_weight: Option<i32>,
) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_dynamic_weights(provider_id, group, original_weight, supervisor_weight);
    }
}

/// Increment allowed requests counter globally
pub fn increment_requests_allowed(provider_id: &str, group: &str) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.increment_requests_allowed(provider_id, group);
    }
}

/// Increment rejected requests counter globally
pub fn increment_requests_rejected(provider_id: &str, group: &str) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.increment_requests_rejected(provider_id, group);
    }
}

/// Update health metrics globally
pub fn update_health_metrics_global(provider_id: &str, health_score: f64, availability: f64) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_health_metrics(provider_id, health_score, availability);
    }
}

/// Update circuit state globally
pub fn update_circuit_state_global(provider_id: &str, circuit_state: &str) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_circuit_state(provider_id, circuit_state);
    }
}

/// Record success globally
pub fn record_success_global(
    provider_id: &str,
    model_id: &str,
    base_url: &str,
    group: &str,
    latency_ms: u64,
    tokens: Option<u64>,
) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.record_success(provider_id, model_id, base_url, group, latency_ms, tokens);
    }
}

/// Record failure globally
pub fn record_failure_global(
    provider_id: &str,
    model_id: &str,
    base_url: &str,
    group: &str,
    error_type: &str,
    latency_ms: u64,
) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.record_failure(
            provider_id,
            model_id,
            base_url,
            group,
            error_type,
            latency_ms,
        );
    }
}

/// Render metrics globally
pub fn render_metrics_global() -> String {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.render()
    } else {
        String::new()
    }
}

// === NEW GLOBAL FUNCTIONS FOR ROUTING OBSERVABILITY ===

/// Update available providers count globally
pub fn update_available_providers_global(tier: i32, group: &str, count: usize) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_available_providers(tier, group, count);
    }
}

/// Update provider utilization globally
pub fn update_provider_utilization_global(provider_id: &str, group: &str, utilization: f64) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.update_provider_utilization(provider_id, group, utilization);
    }
}

/// Record tier failover event globally
pub fn record_tier_failover_global(from_tier: i32, to_tier: i32, group: &str) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.record_tier_failover(from_tier, to_tier, group);
    }
}

/// Record provider selection event globally
pub fn record_provider_selection_global(provider_id: &str, tier: i32, group: &str) {
    if let Some(metrics) = get_global_prometheus_metrics() {
        metrics.record_provider_selection(provider_id, tier, group);
    }
}

/// Record a retry attempt globally
pub fn record_retry_attempt_global(
    model_id: &str,
    provider_id: &str,
    attempt: usize,
    error_type: &str,
) {
    // TODO: Implement retry metrics when PrometheusMetrics struct is updated
    // For now, this is a placeholder to satisfy the compiler
    debug!(
        "Retry attempt {} for model {} via provider {} with error type {}",
        attempt, model_id, provider_id, error_type
    );
}

/// Record a retry success globally
pub fn record_retry_success_global(model_id: &str, provider_id: &str, attempt: usize) {
    // TODO: Implement retry metrics when PrometheusMetrics struct is updated
    // For now, this is a placeholder to satisfy the compiler
    debug!(
        "Retry success on attempt {} for model {} via provider {}",
        attempt, model_id, provider_id
    );
}

/// Record retry exhausted globally
pub fn record_retry_exhausted_global(model_id: &str, provider_id: &str, attempts: usize) {
    // TODO: Implement retry metrics when PrometheusMetrics struct is updated
    // For now, this is a placeholder to satisfy the compiler
    debug!(
        "Retry exhausted after {} attempts for model {} via provider {}",
        attempts, model_id, provider_id
    );
}

/// Error types for provider monitoring
pub enum ProviderErrorType {
    ServerError,
    Timeout,
    RateLimit,
    NetworkError,
    ParseError,
    Other,
}

impl ProviderErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderErrorType::ServerError => "server_error",
            ProviderErrorType::Timeout => "timeout",
            ProviderErrorType::RateLimit => "rate_limit",
            ProviderErrorType::NetworkError => "network_error",
            ProviderErrorType::ParseError => "parse_error",
            ProviderErrorType::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_metrics_creation() {
        let metrics = PrometheusMetrics::new();
        assert!(metrics.is_ok());
    }

    #[test]
    fn test_error_type_conversion() {
        assert_eq!(ProviderErrorType::ServerError.as_str(), "server_error");
        assert_eq!(ProviderErrorType::Timeout.as_str(), "timeout");
        assert_eq!(ProviderErrorType::RateLimit.as_str(), "rate_limit");
    }

    #[test]
    fn test_metrics_recording() {
        let metrics = PrometheusMetrics::new().expect("Metrics should initialize successfully");

        // Test request recording
        metrics.record_success(
            "test-provider",
            "test-model",
            "https://api.test.com",
            "test-group",
            100,
            Some(50),
        );
        metrics.record_failure(
            "test-provider",
            "test-model",
            "https://api.test.com",
            "test-group",
            "timeout",
            200,
        );

        // Test health metrics
        metrics.update_health_metrics("test-provider", 0.8, 0.9);
        metrics.update_circuit_state("test-provider", "closed");
        metrics.update_provider_weight("test-provider", "test-group", 100);

        // Test dynamic weights
        metrics.update_dynamic_weights("test-provider", "test-group", 100, Some(80));
        metrics.update_dynamic_weights("test-provider-2", "test-group", 50, None);

        // Test rate limiting
        metrics.increment_requests_allowed("test-provider", "test-group");
        metrics.increment_requests_rejected("test-provider", "test-group");

        // Test rendering
        let output = metrics.render();
        assert!(!output.is_empty());
        assert!(output.contains("lsp_requests_total"));
    }
}
