use super::traits::{HealthScorer, ScoringContext, ScoringResult};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Latency-based health scorer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyScorer {
    /// Target latency in milliseconds
    pub target_latency_ms: f64,
    /// Multiplier for penalty threshold (e.g., 2.0 means 2x target is penalty threshold)
    pub penalty_threshold_multiplier: f64,
    /// Additional penalty for severe latency issues
    pub severe_penalty_multiplier: f64,
}

impl Default for LatencyScorer {
    fn default() -> Self {
        Self {
            target_latency_ms: 1000.0,
            penalty_threshold_multiplier: 2.0,
            severe_penalty_multiplier: 0.2,
        }
    }
}

impl HealthScorer for LatencyScorer {
    fn calculate_score(&self, context: &ScoringContext) -> ScoringResult {
        let latency_ms = context.metrics.latency_p95_ms as f64;
        let latency_ratio = latency_ms / self.target_latency_ms;

        // Base score: 1.0 for target latency, decreasing as latency increases
        let base_score = if latency_ratio <= 1.0 {
            1.0 // Perfect score for latency at or below target
        } else {
            // Exponential decay for latency above target
            (1.0 / latency_ratio).min(1.0)
        };

        // Apply severe penalty for very high latency
        let penalty = if latency_ratio > self.penalty_threshold_multiplier {
            self.severe_penalty_multiplier * (latency_ratio - self.penalty_threshold_multiplier)
        } else {
            0.0
        };

        let final_score = (base_score - penalty).max(0.0).min(1.0);

        debug!(
            "Latency scoring for {}: {}ms (ratio: {:.2}), base_score: {:.3}, penalty: {:.3}, final: {:.3}",
            context.metrics.provider_id, latency_ms, latency_ratio, base_score, penalty, final_score
        );

        ScoringResult {
            score: final_score,
            weight: 1.0, // Weight is handled by the composite scorer
            details: Some(format!(
                "latency={}ms, target={}ms, ratio={:.2}, penalty={:.3}",
                latency_ms, self.target_latency_ms, latency_ratio, penalty
            )),
        }
    }

    fn name(&self) -> &'static str {
        "latency"
    }

    fn get_config(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn update_config(&mut self, config: serde_json::Value) -> Result<(), String> {
        if let Ok(new_config) = serde_json::from_value::<LatencyScorer>(config) {
            *self = new_config;
            Ok(())
        } else {
            Err("Invalid latency scorer configuration".to_string())
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.target_latency_ms <= 0.0 {
            return Err("target_latency_ms must be positive".to_string());
        }
        if self.penalty_threshold_multiplier <= 1.0 {
            return Err("penalty_threshold_multiplier must be greater than 1.0".to_string());
        }
        Ok(())
    }
}

/// Error rate-based health scorer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRateScorer {
    /// Acceptable error rate (0.0-1.0)
    pub acceptable_error_rate: f64,
    /// Threshold for severe error rate
    pub severe_error_threshold: f64,
    /// Additional penalty for complete failure
    pub complete_failure_penalty: f64,
}

impl Default for ErrorRateScorer {
    fn default() -> Self {
        Self {
            acceptable_error_rate: 0.01,   // 1% error rate is acceptable
            severe_error_threshold: 0.5,   // 50% error rate is severe
            complete_failure_penalty: 0.3, // Additional 30% penalty for complete failure
        }
    }
}

impl HealthScorer for ErrorRateScorer {
    fn calculate_score(&self, context: &ScoringContext) -> ScoringResult {
        let error_rate = context.metrics.error_rate;

        // Base score: 1.0 for acceptable error rate, decreasing as error rate increases
        let base_score = if error_rate <= self.acceptable_error_rate {
            1.0
        } else {
            // Linear decay from acceptable rate to 100% error
            let error_excess = error_rate - self.acceptable_error_rate;
            let max_excess = 1.0 - self.acceptable_error_rate;
            1.0 - (error_excess / max_excess)
        };

        // Apply severe penalty for high error rates
        let severe_penalty = if error_rate > self.severe_error_threshold {
            0.2 // Additional 20% penalty for severe error rate
        } else {
            0.0
        };

        // Apply complete failure penalty
        let failure_penalty = if error_rate >= 1.0 {
            self.complete_failure_penalty
        } else {
            0.0
        };

        let final_score = (base_score - severe_penalty - failure_penalty)
            .max(0.0)
            .min(1.0);

        debug!(
            "Error rate scoring for {}: {:.3} error rate, base_score: {:.3}, severe_penalty: {:.3}, failure_penalty: {:.3}, final: {:.3}",
            context.metrics.provider_id, error_rate, base_score, severe_penalty, failure_penalty, final_score
        );

        ScoringResult {
            score: final_score,
            weight: 1.0,
            details: Some(format!(
                "error_rate={:.3}, acceptable={:.3}, severe_penalty={:.3}, failure_penalty={:.3}",
                error_rate, self.acceptable_error_rate, severe_penalty, failure_penalty
            )),
        }
    }

    fn name(&self) -> &'static str {
        "error_rate"
    }

    fn get_config(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn update_config(&mut self, config: serde_json::Value) -> Result<(), String> {
        if let Ok(new_config) = serde_json::from_value::<ErrorRateScorer>(config) {
            *self = new_config;
            Ok(())
        } else {
            Err("Invalid error rate scorer configuration".to_string())
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.acceptable_error_rate < 0.0 || self.acceptable_error_rate > 1.0 {
            return Err("acceptable_error_rate must be between 0.0 and 1.0".to_string());
        }
        if self.severe_error_threshold < 0.0 || self.severe_error_threshold > 1.0 {
            return Err("severe_error_threshold must be between 0.0 and 1.0".to_string());
        }
        Ok(())
    }
}

/// Utilization-based health scorer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtilizationScorer {
    /// Optimal utilization rate (0.0-1.0)
    pub optimal_utilization: f64,
    /// Threshold for overload condition
    pub overload_threshold: f64,
    /// Penalty multiplier for overload
    pub overload_penalty_multiplier: f64,
}

impl Default for UtilizationScorer {
    fn default() -> Self {
        Self {
            optimal_utilization: 0.7,         // 70% utilization is optimal
            overload_threshold: 0.9,          // 90% utilization is overload
            overload_penalty_multiplier: 1.5, // 1.5x penalty for overload
        }
    }
}

impl HealthScorer for UtilizationScorer {
    fn calculate_score(&self, context: &ScoringContext) -> ScoringResult {
        let utilization = context.metrics.utilization;

        // Score based on distance from optimal utilization
        let base_score = if utilization <= self.optimal_utilization {
            // Linear increase from 0 to optimal
            utilization / self.optimal_utilization
        } else {
            // Linear decrease from optimal to 100%
            let excess = utilization - self.optimal_utilization;
            let max_excess = 1.0 - self.optimal_utilization;
            1.0 - (excess / max_excess)
        };

        // Apply overload penalty
        let overload_penalty = if utilization > self.overload_threshold {
            let excess_overload = utilization - self.overload_threshold;
            excess_overload * self.overload_penalty_multiplier
        } else {
            0.0
        };

        let final_score = (base_score - overload_penalty).max(0.0).min(1.0);

        debug!(
            "Utilization scoring for {}: {:.3} utilization, optimal: {:.3}, base_score: {:.3}, overload_penalty: {:.3}, final: {:.3}",
            context.metrics.provider_id, utilization, self.optimal_utilization, base_score, overload_penalty, final_score
        );

        ScoringResult {
            score: final_score,
            weight: 1.0,
            details: Some(format!(
                "utilization={:.3}, optimal={:.3}, overload_penalty={:.3}",
                utilization, self.optimal_utilization, overload_penalty
            )),
        }
    }

    fn name(&self) -> &'static str {
        "utilization"
    }

    fn get_config(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn update_config(&mut self, config: serde_json::Value) -> Result<(), String> {
        if let Ok(new_config) = serde_json::from_value::<UtilizationScorer>(config) {
            *self = new_config;
            Ok(())
        } else {
            Err("Invalid utilization scorer configuration".to_string())
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.optimal_utilization <= 0.0 || self.optimal_utilization > 1.0 {
            return Err("optimal_utilization must be between 0.0 and 1.0".to_string());
        }
        if self.overload_threshold <= self.optimal_utilization || self.overload_threshold > 1.0 {
            return Err(
                "overload_threshold must be greater than optimal_utilization and <= 1.0"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Token stream health scorer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStreamScorer {
    /// Minimum acceptable token stream health
    pub min_acceptable_health: f64,
    /// Threshold for excellent health
    pub excellent_health_threshold: f64,
    /// Bonus for token generation
    pub token_generation_bonus: f64,
}

impl Default for TokenStreamScorer {
    fn default() -> Self {
        Self {
            min_acceptable_health: 0.5,
            excellent_health_threshold: 0.95,
            token_generation_bonus: 0.15,
        }
    }
}

impl HealthScorer for TokenStreamScorer {
    fn calculate_score(&self, context: &ScoringContext) -> ScoringResult {
        let token_health = context.metrics.token_stream_health;

        // Base score directly from token stream health
        let base_score = token_health;

        // Apply bonus for excellent health
        let excellence_bonus = if token_health >= self.excellent_health_threshold {
            self.token_generation_bonus
        } else {
            0.0
        };

        // Apply penalty for poor health
        let poor_health_penalty = if token_health < self.min_acceptable_health {
            (self.min_acceptable_health - token_health) * 0.5 // 50% penalty for poor health
        } else {
            0.0
        };

        let final_score = (base_score + excellence_bonus - poor_health_penalty)
            .max(0.0)
            .min(1.0);

        debug!(
            "Token stream scoring for {}: {:.3} token health, base_score: {:.3}, excellence_bonus: {:.3}, poor_penalty: {:.3}, final: {:.3}",
            context.metrics.provider_id, token_health, base_score, excellence_bonus, poor_health_penalty, final_score
        );

        ScoringResult {
            score: final_score,
            weight: 1.0,
            details: Some(format!(
                "token_health={:.3}, excellence_bonus={:.3}, poor_penalty={:.3}",
                token_health, excellence_bonus, poor_health_penalty
            )),
        }
    }

    fn name(&self) -> &'static str {
        "token_stream"
    }

    fn get_config(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn update_config(&mut self, config: serde_json::Value) -> Result<(), String> {
        if let Ok(new_config) = serde_json::from_value::<TokenStreamScorer>(config) {
            *self = new_config;
            Ok(())
        } else {
            Err("Invalid token stream scorer configuration".to_string())
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.min_acceptable_health < 0.0 || self.min_acceptable_health > 1.0 {
            return Err("min_acceptable_health must be between 0.0 and 1.0".to_string());
        }
        if self.excellent_health_threshold < 0.0 || self.excellent_health_threshold > 1.0 {
            return Err("excellent_health_threshold must be between 0.0 and 1.0".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::ProviderHealthMetrics;

    fn create_test_context(provider_id: &str) -> ScoringContext {
        ScoringContext {
            metrics: ProviderHealthMetrics {
                provider_id: provider_id.to_string(),
                latency_p95_ms: 500,
                error_rate: 0.01,
                availability: 0.99,
                utilization: 0.7,
                token_stream_health: 0.95,
            },
            base_weight: 100,
            context_data: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn test_latency_scorer() {
        let scorer = LatencyScorer::default();
        let context = create_test_context("test");

        let result = scorer.calculate_score(&context);
        assert!(result.score > 0.0 && result.score <= 1.0);
        assert!(result.details.is_some());
    }

    #[test]
    fn test_error_rate_scorer() {
        let scorer = ErrorRateScorer::default();
        let context = create_test_context("test");

        let result = scorer.calculate_score(&context);
        assert!(result.score > 0.0 && result.score <= 1.0);
    }

    #[test]
    fn test_utilization_scorer() {
        let scorer = UtilizationScorer::default();
        let context = create_test_context("test");

        let result = scorer.calculate_score(&context);
        assert!(result.score > 0.0 && result.score <= 1.0);
    }

    #[test]
    fn test_token_stream_scorer() {
        let scorer = TokenStreamScorer::default();
        let context = create_test_context("test");

        let result = scorer.calculate_score(&context);
        assert!(result.score > 0.0 && result.score <= 1.0);
    }
}
