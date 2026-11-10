//! Metrics Processing Domain
//!
//! This domain handles all aspects of metrics collection, aggregation and transformation.
//! It provides a clean interface for converting raw metrics data into structured health metrics.

pub mod aggregator;
pub mod converter;

// Re-exports for domain API
pub use aggregator::{AggregatedMetrics, MetricsAggregator};
pub use converter::MetricsConverter;

use crate::error::Result;
use crate::monitoring::ProviderHealthMetrics;
use std::collections::HashMap;

/// Domain service for metrics processing
pub struct MetricsService {
    pub aggregator: MetricsAggregator,
    pub converter: MetricsConverter,
}

impl MetricsService {
    pub fn new(aggregator: MetricsAggregator, converter: MetricsConverter) -> Self {
        Self {
            aggregator,
            converter,
        }
    }

    /// Complete metrics processing pipeline
    pub async fn process_metrics(
        &self,
    ) -> Result<(Vec<ProviderHealthMetrics>, HashMap<String, u32>)> {
        // Step 1: Aggregate metrics from all sources
        let aggregated_metrics = self.aggregator.aggregate_all_metrics().await?;

        // Step 2: Convert to health metrics format
        let (health_metrics, base_weights) = self
            .converter
            .convert_to_health_metrics(&aggregated_metrics)
            .await;

        Ok((health_metrics, base_weights))
    }
}
