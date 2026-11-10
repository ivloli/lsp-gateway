use super::config::{ScorerType, ScoringConfig};
use super::scorers::{ErrorRateScorer, LatencyScorer, TokenStreamScorer, UtilizationScorer};
use super::traits::{HealthScorer, ScoringContext, ScoringResult};
use crate::monitoring::ProviderHealthMetrics;
use std::collections::HashMap;

use tracing::{debug, info, warn};

/// Composite health scorer that combines multiple scoring components
pub struct CompositeHealthScorer {
    /// Individual scorers
    scorers: HashMap<String, Box<dyn HealthScorer>>,
    /// Scoring configuration
    config: ScoringConfig,
}

/// Final health scoring result
#[derive(Debug, Clone)]
pub struct HealthScoringResult {
    pub provider_id: String,
    pub overall_score: f64,
    pub base_weight: u32,
    pub individual_scores: HashMap<String, ScoringResult>,
    pub compound_penalty: f64,
    pub last_updated: u64,
}

impl CompositeHealthScorer {
    /// Create a new composite health scorer with default configuration
    pub fn new() -> Self {
        Self::with_config(ScoringConfig::default())
    }

    /// Create a new composite health scorer with custom configuration
    pub fn with_config(config: ScoringConfig) -> Self {
        let mut scorer = Self {
            scorers: HashMap::new(),
            config,
        };

        scorer.initialize_scorers();
        scorer
    }

    /// Initialize scorers based on configuration
    fn initialize_scorers(&mut self) {
        self.scorers.clear();

        for (name, scorer_config) in &self.config.scorers {
            if !scorer_config.enabled {
                continue;
            }

            let scorer: Box<dyn HealthScorer> = match &scorer_config.scorer_type {
                ScorerType::Latency => {
                    let mut scorer = LatencyScorer::default();
                    if let Err(e) = scorer.update_config(scorer_config.config.clone()) {
                        warn!("Failed to configure latency scorer: {}", e);
                    }
                    Box::new(scorer)
                }
                ScorerType::ErrorRate => {
                    let mut scorer = ErrorRateScorer::default();
                    if let Err(e) = scorer.update_config(scorer_config.config.clone()) {
                        warn!("Failed to configure error rate scorer: {}", e);
                    }
                    Box::new(scorer)
                }
                ScorerType::Utilization => {
                    let mut scorer = UtilizationScorer::default();
                    if let Err(e) = scorer.update_config(scorer_config.config.clone()) {
                        warn!("Failed to configure utilization scorer: {}", e);
                    }
                    Box::new(scorer)
                }
                ScorerType::TokenStream => {
                    let mut scorer = TokenStreamScorer::default();
                    if let Err(e) = scorer.update_config(scorer_config.config.clone()) {
                        warn!("Failed to configure token stream scorer: {}", e);
                    }
                    Box::new(scorer)
                }
                ScorerType::Custom(custom_type) => {
                    warn!(
                        "Custom scorer type '{}' not implemented, skipping",
                        custom_type
                    );
                    continue;
                }
            };

            self.scorers.insert(name.clone(), scorer);
        }

        info!("Initialized {} health scorers", self.scorers.len());
    }

    /// Calculate health score for a single provider
    pub fn calculate_health_score(
        &self,
        metrics: &ProviderHealthMetrics,
        base_weight: u32,
    ) -> HealthScoringResult {
        let context = ScoringContext {
            metrics: metrics.clone(),
            base_weight,
            context_data: HashMap::new(),
        };

        let mut individual_scores = HashMap::new();
        let mut weighted_score_sum = 0.0;
        let mut total_weight = 0.0;

        // Calculate individual scores
        for (name, scorer) in &self.scorers {
            if let Some(scorer_config) = self.config.scorers.get(name) {
                if scorer_config.enabled {
                    let result = scorer.calculate_score(&context);
                    let weighted_score = result.score * scorer_config.weight;

                    weighted_score_sum += weighted_score;
                    total_weight += scorer_config.weight;

                    debug!(
                        "Scorer '{}' for {}: score={:.3}, weight={:.3}, weighted={:.3}",
                        name,
                        metrics.provider_id,
                        result.score,
                        scorer_config.weight,
                        weighted_score
                    );

                    individual_scores.insert(name.clone(), result);
                }
            }
        }

        // Calculate base overall score
        let base_overall_score = if total_weight > 0.0 {
            weighted_score_sum / total_weight
        } else {
            0.5 // Default neutral score if no scorers
        };

        // Calculate compound penalty
        let compound_penalty = if self.config.global_params.enable_compound_penalty {
            self.calculate_compound_penalty(metrics, &individual_scores)
        } else {
            0.0
        };

        // Apply compound penalty
        let overall_score = (base_overall_score - compound_penalty)
            .max(0.0)
            .min(self.config.global_params.max_score);

        debug!(
            "Health scoring for {}: base={:.3}, compound_penalty={:.3}, final={:.3}, base_weight={}",
            metrics.provider_id, base_overall_score, compound_penalty, overall_score, base_weight
        );

        HealthScoringResult {
            provider_id: metrics.provider_id.clone(),
            overall_score,
            base_weight,
            individual_scores,
            compound_penalty,
            last_updated: crate::utils::TimeUtils::current_timestamp_secs(),
        }
    }

    /// Calculate compound penalty for multiple severe issues
    fn calculate_compound_penalty(
        &self,
        metrics: &ProviderHealthMetrics,
        individual_scores: &HashMap<String, ScoringResult>,
    ) -> f64 {
        let mut penalty = 0.0;
        let mut severe_issues = 0;

        // Check for severe issues in individual scores
        for (name, result) in individual_scores {
            if result.score < 0.3 {
                // Score below 30% is considered severe
                severe_issues += 1;
                penalty += 0.1; // Base penalty for each severe issue

                debug!(
                    "Severe issue detected in scorer '{}' for {}: score={:.3}",
                    name, metrics.provider_id, result.score
                );
            }
        }

        // Additional compound penalty for multiple severe issues
        if severe_issues >= 2 {
            let compound_multiplier = self.config.global_params.compound_penalty_multiplier;
            let additional_penalty = compound_multiplier * (severe_issues - 1) as f64;
            penalty += additional_penalty;

            debug!(
                "Compound penalty for {}: {} severe issues, additional penalty: {:.3}",
                metrics.provider_id, severe_issues, additional_penalty
            );
        }

        penalty
    }

    /// Calculate health scores for a batch of providers
    pub fn calculate_batch_health_scores(
        &self,
        provider_metrics: &[ProviderHealthMetrics],
        base_weights: &HashMap<String, u32>,
    ) -> Vec<HealthScoringResult> {
        let mut results = Vec::new();

        for metrics in provider_metrics {
            let base_weight = base_weights
                .get(&metrics.provider_id)
                .copied()
                .unwrap_or(100);
            let result = self.calculate_health_score(metrics, base_weight);
            results.push(result);
        }

        if !results.is_empty() {
            debug!(
                "Calculated health scores for {} providers using {} scorers",
                results.len(),
                self.scorers.len()
            );
        }

        results
    }

    /// Update the scoring configuration
    pub fn update_config(&mut self, config: ScoringConfig) -> Result<(), String> {
        config.validate()?;
        self.config = config;
        self.initialize_scorers();
        Ok(())
    }

    /// Get current configuration
    pub fn get_config(&self) -> &ScoringConfig {
        &self.config
    }

    /// Get individual scorer configuration
    pub fn get_scorer_config(&self, name: &str) -> Option<serde_json::Value> {
        self.scorers.get(name).map(|scorer| scorer.get_config())
    }

    /// Update individual scorer configuration
    pub fn update_scorer_config(
        &mut self,
        name: &str,
        config: serde_json::Value,
    ) -> Result<(), String> {
        if let Some(scorer) = self.scorers.get_mut(name) {
            scorer.update_config(config)?;
            Ok(())
        } else {
            Err(format!("Scorer '{}' not found", name))
        }
    }

    /// Validate all scorer configurations
    pub fn validate(&self) -> Result<(), String> {
        self.config.validate()?;

        for (name, scorer) in &self.scorers {
            if let Err(e) = scorer.validate() {
                return Err(format!("Scorer '{}' validation failed: {}", name, e));
            }
        }

        Ok(())
    }
}
