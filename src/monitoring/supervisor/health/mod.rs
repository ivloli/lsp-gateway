//! Health Assessment Domain
//!
//! This domain handles all aspects of provider health evaluation and scoring.
//! It provides a modular scoring system with pluggable scorers and configurable weights.

pub mod scoring;

// Re-exports for domain API
pub use scoring::{CompositeHealthScorer, ScoringConfig};

use crate::monitoring::ProviderHealthMetrics;
use std::collections::HashMap;
use tracing::debug;

/// Provider health information for Redis storage
#[derive(Debug, Clone)]
pub struct ProviderHealthInfo {
    pub provider_id: String,
    pub health_score: f64, // 综合健康评分 (0.0-1.0, 越高越健康)
    pub base_weight: u32,  // 原始权重
    pub last_updated: u64, // 更新时间戳
    pub metrics: ProviderHealthMetrics,
}

/// Domain service for health assessment
pub struct HealthService {
    composite_scorer: CompositeHealthScorer,
}

impl HealthService {
    pub fn new(composite_scorer: CompositeHealthScorer) -> Self {
        Self { composite_scorer }
    }

    /// Evaluate provider health using the scoring system
    pub fn evaluate_health(
        &self,
        health_metrics: &[ProviderHealthMetrics],
        base_weights: &HashMap<String, u32>,
    ) -> Vec<ProviderHealthInfo> {
        let scoring_results = self
            .composite_scorer
            .calculate_batch_health_scores(health_metrics, base_weights);

        // Convert scoring results to ProviderHealthInfo format
        let mut results = Vec::new();
        for scoring_result in scoring_results {
            let provider_health_info = ProviderHealthInfo {
                provider_id: scoring_result.provider_id.clone(),
                health_score: scoring_result.overall_score,
                base_weight: scoring_result.base_weight,
                last_updated: scoring_result.last_updated,
                metrics: health_metrics
                    .iter()
                    .find(|m| m.provider_id == scoring_result.provider_id)
                    .cloned()
                    .unwrap_or_else(|| ProviderHealthMetrics {
                        provider_id: scoring_result.provider_id.clone(),
                        latency_p95_ms: 0,
                        error_rate: 0.0,
                        availability: 1.0,
                        utilization: 0.0,
                        token_stream_health: 0.0,
                    }),
            };
            results.push(provider_health_info);
        }

        if !results.is_empty() {
            debug!(
                "Calculated health scores for {} providers using modular scoring system",
                results.len()
            );
        }

        results
    }
}
