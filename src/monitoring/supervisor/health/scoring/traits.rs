use crate::monitoring::ProviderHealthMetrics;
use serde::{Deserialize, Serialize};

/// Context information for scoring calculations
#[derive(Debug, Clone)]
pub struct ScoringContext {
    /// Provider metrics to score
    pub metrics: ProviderHealthMetrics,
    /// Base weight for the provider
    pub base_weight: u32,
    /// Additional context data
    pub context_data: std::collections::HashMap<String, f64>,
}

/// Scoring result from a health scorer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringResult {
    /// Normalized score (0.0 = worst, 1.0 = best)
    pub score: f64,
    /// Weight/importance of this score in final calculation
    pub weight: f64,
    /// Optional details about the scoring calculation
    pub details: Option<String>,
}

/// Trait for health scoring components
pub trait HealthScorer: Send + Sync {
    /// Calculate a health score for the given context
    fn calculate_score(&self, context: &ScoringContext) -> ScoringResult;

    /// Get the name/identifier of this scorer
    fn name(&self) -> &'static str;

    /// Validate the scorer configuration
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Get scorer-specific configuration as JSON value
    fn get_config(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Update scorer configuration from JSON value
    fn update_config(&mut self, _config: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}
