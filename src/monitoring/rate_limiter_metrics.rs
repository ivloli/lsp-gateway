//! Rate Limiter Metrics
//!
//! Prometheus metrics for monitoring rate limiter performance and behavior.

use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, CounterVec, GaugeVec,
    HistogramOpts, HistogramVec, Opts,
};
use std::sync::OnceLock;

static RATE_LIMITER_LATENCY: OnceLock<HistogramVec> = OnceLock::new();
static RATE_LIMITER_REQUESTS: OnceLock<CounterVec> = OnceLock::new();
static RATE_LIMITER_USAGE: OnceLock<GaugeVec> = OnceLock::new();

/// Initialize rate limiter metrics
pub fn init_metrics() {
    RATE_LIMITER_LATENCY.get_or_init(|| {
        register_histogram_vec!(
            HistogramOpts::new(
                "rate_limiter_check_duration_seconds",
                "Rate limiter check latency in seconds"
            )
            .buckets(vec![
                0.0001, // 100μs
                0.0005, // 500μs
                0.001,  // 1ms
                0.002,  // 2ms
                0.005,  // 5ms
                0.01,   // 10ms
                0.025,  // 25ms
                0.05,   // 50ms
            ]),
            &["algorithm", "provider_id"]
        )
        .expect("Failed to register rate_limiter_check_duration_seconds")
    });

    RATE_LIMITER_REQUESTS.get_or_init(|| {
        register_counter_vec!(
            Opts::new(
                "rate_limiter_requests_total",
                "Total number of rate limiter requests"
            ),
            &["algorithm", "provider_id", "result"]
        )
        .expect("Failed to register rate_limiter_requests_total")
    });

    RATE_LIMITER_USAGE.get_or_init(|| {
        register_gauge_vec!(
            Opts::new(
                "rate_limiter_usage_ratio",
                "Current rate limiter usage ratio (0.0-1.0)"
            ),
            &["algorithm", "provider_id"]
        )
        .expect("Failed to register rate_limiter_usage_ratio")
    });
}

/// Record rate limiter check latency
pub fn record_check_latency(algorithm: &str, provider_id: &str, duration_secs: f64) {
    if let Some(metric) = RATE_LIMITER_LATENCY.get() {
        metric
            .with_label_values(&[algorithm, provider_id])
            .observe(duration_secs);
    }
}

/// Increment rate limiter request counter
pub fn increment_requests(algorithm: &str, provider_id: &str, result: &str) {
    if let Some(metric) = RATE_LIMITER_REQUESTS.get() {
        metric
            .with_label_values(&[algorithm, provider_id, result])
            .inc();
    }
}

/// Update rate limiter usage gauge
pub fn update_usage_ratio(algorithm: &str, provider_id: &str, ratio: f64) {
    if let Some(metric) = RATE_LIMITER_USAGE.get() {
        metric
            .with_label_values(&[algorithm, provider_id])
            .set(ratio);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_initialization() {
        init_metrics();

        // Verify metrics are initialized
        assert!(RATE_LIMITER_LATENCY.get().is_some());
        assert!(RATE_LIMITER_REQUESTS.get().is_some());
        assert!(RATE_LIMITER_USAGE.get().is_some());
    }
}
