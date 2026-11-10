use super::aggregator::AggregatedMetrics;
use crate::models::ModelProvider;
use crate::monitoring::ProviderHealthMetrics;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

/// Converts aggregated metrics to health metrics format
pub struct MetricsConverter {
    db_pool: Arc<PgPool>,
}

impl MetricsConverter {
    pub fn new(db_pool: Arc<PgPool>) -> Self {
        Self { db_pool }
    }

    /// Convert aggregated metrics to health metrics format
    pub async fn convert_to_health_metrics(
        &self,
        aggregated_metrics: &HashMap<String, AggregatedMetrics>,
    ) -> (Vec<ProviderHealthMetrics>, HashMap<String, u32>) {
        let mut health_metrics = Vec::new();
        let mut base_weights = HashMap::new();

        for (provider_id, metrics) in aggregated_metrics {
            if let Ok(Some(provider)) = ModelProvider::find_by_id(&self.db_pool, provider_id).await
            {
                let health_metric = ProviderHealthMetrics {
                    provider_id: provider_id.clone(),
                    latency_p95_ms: metrics.latency_p95_ms,
                    error_rate: Self::calculate_error_rate(metrics),
                    availability: Self::calculate_availability(metrics),
                    utilization: Self::calculate_utilization(metrics, provider.max_rps),
                    token_stream_health: Self::calculate_token_stream_health(metrics),
                };

                health_metrics.push(health_metric);
                base_weights.insert(provider_id.clone(), provider.weight as u32);
            } else {
                debug!("Provider {} not found in database, skipping", provider_id);
            }
        }

        (health_metrics, base_weights)
    }

    /// Calculate error rate from aggregated metrics
    fn calculate_error_rate(metrics: &AggregatedMetrics) -> f64 {
        let total_requests = metrics.total_success + metrics.total_fail;
        if total_requests > 0 {
            metrics.total_fail as f64 / total_requests as f64
        } else {
            0.0
        }
    }

    /// Calculate availability from aggregated metrics
    fn calculate_availability(metrics: &AggregatedMetrics) -> f64 {
        let total_requests = metrics.total_success + metrics.total_fail;
        if total_requests > 0 {
            metrics.total_success as f64 / total_requests as f64
        } else {
            1.0 // 没有请求时假设可用
        }
    }

    /// Calculate utilization (raw value, 0.0-1.0)
    fn calculate_utilization(metrics: &AggregatedMetrics, max_rps: i32) -> f64 {
        let total_requests = metrics.total_success + metrics.total_fail;
        if max_rps > 0 {
            let window_seconds = 30;
            let max_requests_in_window = max_rps as u64 * window_seconds;
            let util = (total_requests as f64) / (max_requests_in_window as f64);
            // Ensure utilization is capped at 1.0
            util.min(1.0)
        } else {
            0.0
        }
    }

    /// Calculate token stream health based on success ratio and token generation
    fn calculate_token_stream_health(metrics: &AggregatedMetrics) -> f64 {
        let total_requests = metrics.total_success + metrics.total_fail;
        match total_requests {
            0 => 0.5,                               // No data, neutral
            _ if metrics.total_success == 0 => 0.0, // Complete failure
            _ if metrics.total_fail == 0 => 0.95,   // Perfect success
            _ => {
                // Partial success with token generation bonus
                let success_ratio = metrics.total_success as f64 / total_requests as f64;
                let base_health = success_ratio * 0.8;
                let token_bonus = if metrics.total_success > 0 { 0.15 } else { 0.0 };
                (base_health + token_bonus).min(0.95)
            }
        }
    }
}
