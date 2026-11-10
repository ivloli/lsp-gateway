pub mod health;
pub mod metrics;
pub mod prometheus;
pub mod rate_limiter_metrics;
pub mod supervisor;

pub use health::{HealthMonitor, ProviderHealthTracker};
pub use metrics::{ErrorType, MetricsReporter, ProviderHealthMetrics, ProviderMetricsWindow};
pub use prometheus::{
    get_global_prometheus_metrics,
    increment_requests_allowed,
    increment_requests_rejected,
    init_global_prometheus_metrics,
    record_failure_global,
    record_provider_selection_global,
    // Retry metrics
    record_retry_attempt_global,
    record_retry_exhausted_global,
    record_retry_success_global,
    record_success_global,
    record_tier_failover_global,
    render_metrics_global,
    // New routing observability metrics
    update_available_providers_global,
    update_circuit_state_global,
    update_dynamic_weights_global,
    update_health_metrics_global,
    update_provider_utilization_global,
    update_provider_weight_global,
    PrometheusMetrics,
    ProviderErrorType,
};
pub use rate_limiter_metrics::{
    increment_requests as increment_rate_limiter_requests,
    init_metrics as init_rate_limiter_metrics, record_check_latency as record_rate_limiter_latency,
    update_usage_ratio as update_rate_limiter_usage,
};
