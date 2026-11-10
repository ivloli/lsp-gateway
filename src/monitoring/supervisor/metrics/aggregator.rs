use crate::error::Result;
use crate::storage::{ProviderMetrics, RedisService};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// Gateway-specific provider metrics (detailed view)
#[derive(Debug, Clone)]
pub struct GatewayProviderMetrics {
    pub gateway_id: String,
    pub provider_id: String,
    pub metrics: ProviderMetrics,
}

/// Aggregated metrics for a provider (simplified - already aggregated by Redis)
#[derive(Debug, Clone, Default)]
pub struct AggregatedMetrics {
    pub provider_id: String,
    pub model_provider_group: Option<String>,
    pub total_success: u64,
    pub total_fail: u64,
    pub latency_p95_ms: u64,       // P95 latency
    pub total_token_count: u64,    // Total tokens processed
    pub token_throughput_tps: f64, // Average token throughput
    pub success_rate: f64,
    pub last_updated: u64,
}

pub struct MetricsAggregator {
    redis_service: Arc<RedisService>,
}

impl MetricsAggregator {
    pub fn new(
        redis_service: Arc<RedisService>,
        _aggregation_interval_secs: u64,
        _gateway_timeout_secs: u64,
    ) -> Self {
        Self { redis_service }
    }

    /// Get all provider metrics and aggregate by provider_id (multiple gateways per provider)
    pub async fn aggregate_all_metrics(&self) -> Result<HashMap<String, AggregatedMetrics>> {
        let provider_metrics = self.redis_service.get_all_provider_latest_metrics().await?;

        if provider_metrics.is_empty() {
            return Ok(HashMap::new());
        }

        let total_gateway_instances = provider_metrics.len();
        let mut aggregated = HashMap::new();

        // Group metrics by provider_id and aggregate across gateways
        for metric in provider_metrics {
            let provider_id = &metric.provider_id;

            // Get or create aggregated metric for this provider
            let aggregated_metric =
                aggregated
                    .entry(provider_id.clone())
                    .or_insert_with(|| AggregatedMetrics {
                        provider_id: provider_id.clone(),
                        model_provider_group: metric.model_provider_group.clone(),
                        total_success: 0,
                        total_fail: 0,
                        latency_p95_ms: 0,
                        total_token_count: 0,
                        token_throughput_tps: 0.0,
                        success_rate: 0.0,
                        last_updated: 0,
                    });

            // Accumulate metrics from this gateway
            aggregated_metric.total_success += metric.success_count;
            aggregated_metric.total_fail += metric.fail_count;
            aggregated_metric.total_token_count += metric.token_count;

            // For P95 latency, take the max across gateways (worst case)
            if metric.latency_p95_ms > aggregated_metric.latency_p95_ms {
                aggregated_metric.latency_p95_ms = metric.latency_p95_ms;
            }

            // For token throughput, sum across gateways
            aggregated_metric.token_throughput_tps += metric.token_throughput_tps;

            // Keep the latest timestamp
            if metric.timestamp > aggregated_metric.last_updated {
                aggregated_metric.last_updated = metric.timestamp;
            }
        }

        // Calculate derived metrics for each provider
        for (_, aggregated_metric) in aggregated.iter_mut() {
            // Calculate success rate
            let total_requests = aggregated_metric.total_success + aggregated_metric.total_fail;
            aggregated_metric.success_rate = if total_requests > 0 {
                aggregated_metric.total_success as f64 / total_requests as f64
            } else {
                0.0
            };
        }

        info!(
            "Aggregated metrics for {} providers from {} gateway instances",
            aggregated.len(),
            total_gateway_instances
        );
        Ok(aggregated)
    }
}
