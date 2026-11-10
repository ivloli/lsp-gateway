use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for individual scorers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScorerConfig {
    /// Type of scorer
    pub scorer_type: ScorerType,
    /// Weight of this scorer in the final calculation (0.0-1.0)
    pub weight: f64,
    /// Whether this scorer is enabled
    pub enabled: bool,
    /// Scorer-specific configuration
    pub config: serde_json::Value,
}

/// Available scorer types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScorerType {
    Latency,
    ErrorRate,
    Utilization,
    TokenStream,
    Custom(String),
}

/// Main scoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfig {
    /// Individual scorer configurations
    pub scorers: HashMap<String, ScorerConfig>,
    /// Global scoring parameters
    pub global_params: GlobalScoringParams,
}

/// Global parameters for scoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalScoringParams {
    /// Maximum score (for normalization)
    pub max_score: f64,
    /// Enable compound penalty for multiple issues
    pub enable_compound_penalty: bool,
    /// Compound penalty multiplier
    pub compound_penalty_multiplier: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        let mut scorers = HashMap::new();

        // Latency scorer
        scorers.insert(
            "latency".to_string(),
            ScorerConfig {
                scorer_type: ScorerType::Latency,
                weight: 0.2,
                enabled: false,
                config: serde_json::json!({
                    "target_latency_ms": 1000.0,
                    "penalty_threshold_multiplier": 2.0,
                    "severe_penalty_multiplier": 0.2
                }),
            },
        );

        // Error rate scorer
        scorers.insert(
            "error_rate".to_string(),
            ScorerConfig {
                scorer_type: ScorerType::ErrorRate,
                weight: 1.0,
                enabled: true,
                config: serde_json::json!({
                    "acceptable_error_rate": 0.01,
                    "severe_error_threshold": 0.5,
                    "complete_failure_penalty": 0.3
                }),
            },
        );

        // Utilization scorer - 关闭
        scorers.insert(
            "utilization".to_string(),
            ScorerConfig {
                scorer_type: ScorerType::Utilization,
                weight: 0.2,
                enabled: false,
                config: serde_json::json!({
                    "optimal_utilization": 0.7,
                    "overload_threshold": 0.9,
                    "overload_penalty_multiplier": 1.5
                }),
            },
        );

        // Token stream scorer - 关闭
        scorers.insert(
            "token_stream".to_string(),
            ScorerConfig {
                scorer_type: ScorerType::TokenStream,
                weight: 0.0,    // 权重设为0
                enabled: false, // 关闭
                config: serde_json::json!({
                    "min_acceptable_health": 0.5,
                    "excellent_health_threshold": 0.95,
                    "token_generation_bonus": 0.15
                }),
            },
        );

        Self {
            scorers,
            global_params: GlobalScoringParams::default(),
        }
    }
}

impl Default for GlobalScoringParams {
    fn default() -> Self {
        Self {
            max_score: 1.0,
            enable_compound_penalty: true,
            compound_penalty_multiplier: 0.2,
        }
    }
}

impl ScoringConfig {
    /// Validate the scoring configuration
    pub fn validate(&self) -> Result<(), String> {
        // Check that weights sum to approximately 1.0
        let total_weight: f64 = self
            .scorers
            .values()
            .filter(|s| s.enabled)
            .map(|s| s.weight)
            .sum();

        if (total_weight - 1.0).abs() > 0.01 {
            return Err(format!(
                "Enabled scorer weights do not sum to 1.0: actual sum = {:.3}",
                total_weight
            ));
        }

        // Validate individual scorer weights
        for (name, scorer) in &self.scorers {
            if scorer.weight < 0.0 || scorer.weight > 1.0 {
                return Err(format!(
                    "Scorer '{}' has invalid weight: {:.3} (must be 0.0-1.0)",
                    name, scorer.weight
                ));
            }
        }

        if self.global_params.max_score <= 0.0 {
            return Err("max_score must be positive".to_string());
        }

        Ok(())
    }
}
