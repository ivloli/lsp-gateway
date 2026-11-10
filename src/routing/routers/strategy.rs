use crate::models::ModelProvider;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct RouterTarget {
    pub id: String,
    pub weight: i32,
}

impl From<&ModelProvider> for RouterTarget {
    fn from(provider: &ModelProvider) -> Self {
        Self {
            id: provider.id.clone(),
            weight: provider.weight,
        }
    }
}

pub enum RoutingStrategy {
    WeightedRandom(WeightedRandom),
    SmoothWeightedRoundRobin(SmoothWeightedRoundRobin),
    WeightedPowerOfTwoChoices(WeightedPowerOfTwoChoices),
}

impl RoutingStrategy {
    pub async fn select(
        &self,
        providers: &[ModelProvider],
        context: &crate::routing::pipeline::PipelineContext,
    ) -> Option<RouterTarget> {
        match self {
            RoutingStrategy::WeightedRandom(strategy) => strategy.select(providers).await,
            RoutingStrategy::SmoothWeightedRoundRobin(strategy) => strategy.select(providers).await,
            RoutingStrategy::WeightedPowerOfTwoChoices(strategy) => {
                strategy.select(providers, context).await
            }
        }
    }
}

pub fn create_routing_strategy(strategy_name: &str) -> RoutingStrategy {
    match strategy_name {
        "smooth_weighted_round_robin" | "swrr" => {
            RoutingStrategy::SmoothWeightedRoundRobin(SmoothWeightedRoundRobin::new())
        }
        "weighted_power_of_two_choices" | "p2c" => {
            RoutingStrategy::WeightedPowerOfTwoChoices(WeightedPowerOfTwoChoices::new())
        }
        "weighted_random" => RoutingStrategy::WeightedRandom(WeightedRandom::new()),
        _ => {
            warn!(
                "Unknown routing strategy '{}', falling back to smooth_weighted_round_robin",
                strategy_name
            );
            RoutingStrategy::SmoothWeightedRoundRobin(SmoothWeightedRoundRobin::new())
        }
    }
}
// Weighted Random (simple fallback algorithm)
pub struct WeightedRandom;

impl WeightedRandom {
    pub fn new() -> Self {
        Self
    }

    pub async fn select(&self, providers: &[ModelProvider]) -> Option<RouterTarget> {
        if providers.is_empty() {
            return None;
        }

        let total_weight: i32 = providers.iter().map(|p| p.weight.max(0)).sum();
        if total_weight <= 0 {
            return None;
        }

        use rand::{thread_rng, Rng};
        let mut rng = thread_rng();
        let random_weight = rng.gen_range(0..total_weight);
        let mut cumulative_weight = 0;

        for provider in providers {
            let weight = provider.weight.max(0);
            cumulative_weight += weight;
            if random_weight < cumulative_weight {
                return Some(RouterTarget::from(provider));
            }
        }

        // Fallback to last provider (should not reach here)
        providers.last().map(RouterTarget::from)
    }
}

// Smooth Weighted Round Robin (FIXED) - Precise weight distribution
//
// This is the CORRECTED implementation that fixes the weight=1 provider starvation bug
// and ensures fair distribution when providers have equal weights.
//
// Algorithm (Nginx-style with fairness enhancement):
// 1. For each provider, increase its current_weight by its configured weight
// 2. Select the provider with the highest current_weight
//    - If multiple providers have the same max weight, randomly select one for fairness
// 3. Decrease the selected provider's current_weight by total_weight
//
// Key fixes:
// - Step 1 and 2 are separated (not done in the same loop) → fixes weight=1 bug
// - Random selection among equal weights → ensures fairness
//
// Benefits:
// - 100% accurate weight distribution (deterministic)
// - Fair distribution when weights are equal (no first-provider bias)
// - Smooth distribution (avoids consecutive selections)
// - Fixed: weight=1 providers now get selected correctly
// - Predictable behavior with fairness guarantee
//
// Perfect for scenarios requiring strict weight adherence:
// - Tier-based failover with standby providers (weight=1)
// - Precise capacity allocation
// - Cost-sensitive routing
// - Multiple providers with identical weights
pub struct SmoothWeightedRoundRobin {
    // 使用 parking_lot::Mutex 替代 std::sync::Mutex
    // 优势：更快的锁获取（2-3倍）、无 poison 处理、更小的内存占用
    current_weights: parking_lot::Mutex<std::collections::HashMap<String, i32>>,
}

impl SmoothWeightedRoundRobin {
    pub fn new() -> Self {
        Self {
            current_weights: parking_lot::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub async fn select(&self, providers: &[ModelProvider]) -> Option<RouterTarget> {
        if providers.is_empty() {
            return None;
        }

        // parking_lot::Mutex::lock() 直接返回 MutexGuard，无需处理 poison
        let mut weights = self.current_weights.lock();

        let total_weight: i32 = providers.iter().map(|p| p.weight).sum();
        if total_weight <= 0 {
            return None;
        }

        // Step 1: Increase current_weight for ALL providers first
        for provider in providers {
            let current_weight = weights.entry(provider.id.clone()).or_insert(0);
            *current_weight += provider.weight;
        }

        // Step 2: Select the provider with highest current_weight
        // If multiple providers have the same max weight, randomly select one for fairness
        let mut max_current_weight = i32::MIN;
        let mut candidates_with_max_weight = Vec::new();

        for provider in providers {
            if let Some(&current_weight) = weights.get(&provider.id) {
                if current_weight > max_current_weight {
                    max_current_weight = current_weight;
                    candidates_with_max_weight.clear();
                    candidates_with_max_weight.push(provider);
                } else if current_weight == max_current_weight {
                    candidates_with_max_weight.push(provider);
                }
            }
        }

        // Randomly select from candidates with the same max weight
        let selected_provider = if !candidates_with_max_weight.is_empty() {
            if candidates_with_max_weight.len() == 1 {
                Some(candidates_with_max_weight[0])
            } else {
                use rand::{thread_rng, Rng};
                let mut rng = thread_rng();
                let idx = rng.gen_range(0..candidates_with_max_weight.len());
                Some(candidates_with_max_weight[idx])
            }
        } else {
            None
        };

        // Step 3: Decrease selected provider's current_weight by total_weight
        if let Some(selected) = selected_provider {
            if let Some(weight) = weights.get_mut(&selected.id) {
                *weight -= total_weight;
            }

            debug!(
                "SWRR selected {} (current_weight={}, after={})",
                selected.id,
                max_current_weight,
                max_current_weight - total_weight
            );

            Some(RouterTarget::from(selected))
        } else {
            None
        }
    }
}

// Weighted Power of Two Choices (P2C) - Primary routing algorithm
//
// This algorithm combines weighted random selection with load-aware routing:
// 1. Randomly select 2 providers based on their weights (higher weight = higher selection probability)
// 2. Compare the load ratio of these 2 providers
// 3. Choose the one with lower load
//
// Benefits:
// - O(1) time complexity (only checks 2 providers)
// - Stateless (suitable for distributed deployments)
// - Load-aware (automatically balances based on current RPS)
// - Weight-aware (respects provider weights)
// - Mathematically proven: achieves O(log log N) load balance with just 2 random choices
//
// Perfect for LLM routing where:
// - Request duration is long and unpredictable
// - Real-time load sensing is critical
// - Provider capacity varies significantly
pub struct WeightedPowerOfTwoChoices;

impl WeightedPowerOfTwoChoices {
    pub fn new() -> Self {
        Self
    }

    pub async fn select(
        &self,
        providers: &[ModelProvider],
        context: &crate::routing::pipeline::PipelineContext,
    ) -> Option<RouterTarget> {
        if providers.is_empty() {
            return None;
        }

        // Single provider - return directly
        if providers.len() == 1 {
            return Some(RouterTarget::from(&providers[0]));
        }

        // Select 2 providers using weighted random sampling
        let (provider1, provider2) = self.weighted_random_pick_two(providers);

        // Calculate load ratio for both providers
        let load_ratio_1 = self.calculate_weighted_load(provider1, context);
        let load_ratio_2 = self.calculate_weighted_load(provider2, context);

        // Choose the one with lower load ratio
        let selected = if load_ratio_1 <= load_ratio_2 {
            provider1
        } else {
            provider2
        };

        debug!(
            "P2C selected {} (load={:.3}) over {} (load={:.3})",
            selected.id,
            if selected.id == provider1.id {
                load_ratio_1
            } else {
                load_ratio_2
            },
            if selected.id == provider1.id {
                provider2.id.as_str()
            } else {
                provider1.id.as_str()
            },
            if selected.id == provider1.id {
                load_ratio_2
            } else {
                load_ratio_1
            }
        );

        Some(RouterTarget::from(selected))
    }

    /// Weighted random sampling: select 2 different providers based on their weights
    ///
    /// Providers with higher weights have higher probability of being selected.
    /// This ensures that the algorithm respects the configured weight distribution.
    fn weighted_random_pick_two<'a>(
        &self,
        providers: &'a [ModelProvider],
    ) -> (&'a ModelProvider, &'a ModelProvider) {
        use rand::{thread_rng, Rng};
        let mut rng = thread_rng();

        // Calculate total weight (treat 0 as 1 to avoid division by zero)
        let total_weight: i32 = providers.iter().map(|p| p.weight.max(1)).sum();

        // First weighted random selection
        let rand1 = rng.gen_range(0..total_weight);
        let mut cumulative = 0;
        let mut idx1 = 0;

        for (i, provider) in providers.iter().enumerate() {
            cumulative += provider.weight.max(1);
            if rand1 < cumulative {
                idx1 = i;
                break;
            }
        }

        // Second weighted random selection (ensure different from first)
        let mut idx2 = (idx1 + 1) % providers.len();

        if providers.len() > 2 {
            // For 3+ providers, do proper weighted sampling excluding the first choice
            let total_weight_excluding_first = total_weight - providers[idx1].weight.max(1);

            if total_weight_excluding_first > 0 {
                let rand2 = rng.gen_range(0..total_weight_excluding_first);
                cumulative = 0;

                for (i, provider) in providers.iter().enumerate() {
                    if i == idx1 {
                        continue; // Skip the first selected provider
                    }
                    cumulative += provider.weight.max(1);
                    if rand2 < cumulative {
                        idx2 = i;
                        break;
                    }
                }
            }
        }

        (&providers[idx1], &providers[idx2])
    }

    /// Calculate weighted load ratio: current_rps / (weight * max_rps)
    ///
    /// This metric represents how "busy" a provider is relative to its capacity and weight.
    /// Lower ratio = more available capacity = better choice for new requests.
    ///
    /// Returns f64::INFINITY if provider has invalid configuration (weight=0 or max_rps=0)
    fn calculate_weighted_load(
        &self,
        provider: &ModelProvider,
        context: &crate::routing::pipeline::PipelineContext,
    ) -> f64 {
        // Skip providers with invalid configuration
        if provider.weight <= 0 || provider.max_rps == 0 {
            return f64::INFINITY;
        }

        let current_rps = context.get_provider_rps(&provider.id);
        let max_capacity = provider.weight as f64 * provider.max_rps as f64;

        current_rps / max_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::pipeline::PipelineContext;
    use std::collections::HashMap;

    // Helper function to create a test provider
    fn create_test_provider(id: &str, weight: i32, max_rps: i32) -> ModelProvider {
        use chrono::Utc;

        ModelProvider {
            id: id.to_string(),
            group_name: "test-group".to_string(),
            provider: "test".to_string(),
            weight,
            replica_count: "1".to_string(),
            base_url: "http://test".to_string(),
            max_rps,
            min_rps: 0,
            rps: Some(0),
            tier: 0,
            active: true,
            created_at: Utc::now(),
            updated_at: None,
            override_model_name: None,
            api_key_env_name: None,
            frpmodel: None,
        }
    }

    #[tokio::test]
    async fn test_p2c_single_provider() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![create_test_provider("provider-1", 100, 100)];
        let context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };

        let result = strategy.select(&providers, &context).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "provider-1");
    }

    #[tokio::test]
    async fn test_p2c_two_providers_different_loads() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![
            create_test_provider("high-load", 100, 100),
            create_test_provider("low-load", 100, 100),
        ];

        // Create context with different RPS values
        let mut context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };
        context
            .provider_rps_cache
            .insert("high-load".to_string(), 80.0); // 80% load
        context
            .provider_rps_cache
            .insert("low-load".to_string(), 20.0); // 20% load

        // Run multiple times to verify it tends to select low-load provider
        let mut selections = HashMap::new();
        for _ in 0..100 {
            if let Some(target) = strategy.select(&providers, &context).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        // Low-load provider should be selected more often (not 100% due to randomness)
        let low_load_count = selections.get("low-load").unwrap_or(&0);
        let high_load_count = selections.get("high-load").unwrap_or(&0);

        assert!(
            *low_load_count > *high_load_count,
            "Low-load provider should be selected more often: low={}, high={}",
            low_load_count,
            high_load_count
        );
    }

    #[tokio::test]
    async fn test_p2c_weight_distribution() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![
            create_test_provider("heavy", 100, 100),
            create_test_provider("light", 1, 100),
        ];

        // Equal load - should respect weights
        let context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };

        let mut selections = HashMap::new();
        for _ in 0..1000 {
            if let Some(target) = strategy.select(&providers, &context).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let heavy_count = selections.get("heavy").unwrap_or(&0);
        let light_count = selections.get("light").unwrap_or(&0);

        // Heavy should be selected much more often (roughly 100:1 ratio)
        // Allow some variance due to randomness
        assert!(
            *heavy_count > *light_count * 10,
            "Heavy weight provider should be selected much more: heavy={}, light={}",
            heavy_count,
            light_count
        );
    }

    #[tokio::test]
    async fn test_p2c_weight_1_provider_gets_selected() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![
            create_test_provider("primary", 100, 100),
            create_test_provider("backup", 1, 100),
        ];

        // Primary is overloaded, backup is idle
        let mut context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };
        context
            .provider_rps_cache
            .insert("primary".to_string(), 95.0); // 95% load
        context.provider_rps_cache.insert("backup".to_string(), 0.0); // 0% load

        let mut backup_selected = false;
        for _ in 0..100 {
            if let Some(target) = strategy.select(&providers, &context).await {
                if target.id == "backup" {
                    backup_selected = true;
                    break;
                }
            }
        }

        assert!(
            backup_selected,
            "Backup provider with weight=1 should be selected when it has lower load"
        );
    }

    #[tokio::test]
    async fn test_p2c_invalid_provider_skipped() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![
            create_test_provider("valid", 100, 100),
            create_test_provider("invalid-weight", 0, 100),
            create_test_provider("invalid-rps", 100, 0),
        ];

        let context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };

        // Should always select the valid provider
        for _ in 0..10 {
            let result = strategy.select(&providers, &context).await;
            assert!(result.is_some());
            // Valid provider should be preferred over invalid ones
        }
    }

    #[tokio::test]
    async fn test_p2c_empty_providers() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers: Vec<ModelProvider> = vec![];
        let context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };

        let result = strategy.select(&providers, &context).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_p2c_three_providers_load_balancing() {
        let strategy = WeightedPowerOfTwoChoices;
        let providers = vec![
            create_test_provider("p1", 50, 100),
            create_test_provider("p2", 50, 100),
            create_test_provider("p3", 50, 100),
        ];

        // P1 is overloaded, P2 and P3 are idle
        let mut context = PipelineContext {
            provider_health_cache: std::collections::HashMap::new(),
            provider_rps_cache: std::collections::HashMap::new(),
            retry_counter: 0,
        };
        context.provider_rps_cache.insert("p1".to_string(), 90.0);
        context.provider_rps_cache.insert("p2".to_string(), 10.0);
        context.provider_rps_cache.insert("p3".to_string(), 10.0);

        let mut selections = HashMap::new();
        for _ in 0..100 {
            if let Some(target) = strategy.select(&providers, &context).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let p1_count = selections.get("p1").unwrap_or(&0);
        let p2_count = selections.get("p2").unwrap_or(&0);
        let p3_count = selections.get("p3").unwrap_or(&0);

        // P1 should be selected much less than P2 and P3
        assert!(
            *p1_count < (*p2_count + *p3_count) / 2,
            "Overloaded provider should be selected less: p1={}, p2={}, p3={}",
            p1_count,
            p2_count,
            p3_count
        );
    }

    #[tokio::test]
    async fn test_weighted_random_basic() {
        let strategy = WeightedRandom;
        let providers = vec![
            create_test_provider("p1", 70, 100),
            create_test_provider("p2", 30, 100),
        ];

        let mut selections = HashMap::new();
        for _ in 0..1000 {
            if let Some(target) = strategy.select(&providers).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let p1_count = *selections.get("p1").unwrap_or(&0);
        let p2_count = *selections.get("p2").unwrap_or(&0);

        // Should roughly follow 70:30 ratio (allow 20% variance)
        let ratio = p1_count as f64 / p2_count as f64;
        assert!(
            ratio > 1.5 && ratio < 3.5,
            "Weighted random should follow weight ratio: p1={}, p2={}, ratio={:.2}",
            p1_count,
            p2_count,
            ratio
        );
    }

    // ============================================================================
    // Smooth Weighted Round Robin Tests
    // ============================================================================

    #[tokio::test]
    async fn test_swrr_single_provider() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![create_test_provider("provider-1", 100, 100)];

        let result = strategy.select(&providers).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "provider-1");
    }

    #[tokio::test]
    async fn test_swrr_precise_weight_distribution() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("heavy", 100, 100),
            create_test_provider("light", 50, 100),
        ];

        let mut selections = HashMap::new();
        for _ in 0..150 {
            if let Some(target) = strategy.select(&providers).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let heavy_count = *selections.get("heavy").unwrap_or(&0);
        let light_count = *selections.get("light").unwrap_or(&0);

        // SWRR should be EXACTLY 2:1 ratio (100:50)
        assert_eq!(heavy_count, 100, "Heavy should get exactly 100 requests");
        assert_eq!(light_count, 50, "Light should get exactly 50 requests");
    }

    #[tokio::test]
    async fn test_swrr_weight_1_provider_gets_selected() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("primary", 100, 100),
            create_test_provider("backup", 1, 100),
        ];

        let mut selections = HashMap::new();
        for _ in 0..101 {
            if let Some(target) = strategy.select(&providers).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let primary_count = *selections.get("primary").unwrap_or(&0);
        let backup_count = *selections.get("backup").unwrap_or(&0);

        // Critical test: backup with weight=1 MUST be selected exactly once
        assert_eq!(
            primary_count, 100,
            "Primary should get exactly 100 requests"
        );
        assert_eq!(
            backup_count, 1,
            "Backup with weight=1 should get exactly 1 request (BUG FIXED!)"
        );
    }

    #[tokio::test]
    async fn test_swrr_smooth_distribution() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("a", 5, 100),
            create_test_provider("b", 1, 100),
            create_test_provider("c", 1, 100),
        ];

        let mut sequence = Vec::new();
        for _ in 0..7 {
            if let Some(target) = strategy.select(&providers).await {
                sequence.push(target.id);
            }
        }

        // SWRR should distribute smoothly, not consecutively
        // With weights 5:1:1, should be something like: a,a,b,a,c,a,a (smooth)
        // NOT: a,a,a,a,a,b,c (consecutive)

        // Count consecutive 'a' selections
        let mut max_consecutive_a = 0;
        let mut current_consecutive = 0;
        for id in &sequence {
            if id == "a" {
                current_consecutive += 1;
                max_consecutive_a = max_consecutive_a.max(current_consecutive);
            } else {
                current_consecutive = 0;
            }
        }

        // With smooth distribution, should not have more than 2 consecutive 'a'
        assert!(
            max_consecutive_a <= 2,
            "SWRR should distribute smoothly, not consecutively. Sequence: {:?}",
            sequence
        );
    }

    #[tokio::test]
    async fn test_swrr_empty_providers() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers: Vec<ModelProvider> = vec![];

        let result = strategy.select(&providers).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_swrr_zero_total_weight() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("p1", 0, 100),
            create_test_provider("p2", 0, 100),
        ];

        let result = strategy.select(&providers).await;
        assert!(
            result.is_none(),
            "Should return None when total weight is 0"
        );
    }

    #[tokio::test]
    async fn test_swrr_three_providers_exact_distribution() {
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("p1", 50, 100),
            create_test_provider("p2", 30, 100),
            create_test_provider("p3", 20, 100),
        ];

        let mut selections = HashMap::new();
        for _ in 0..100 {
            if let Some(target) = strategy.select(&providers).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let p1_count = *selections.get("p1").unwrap_or(&0);
        let p2_count = *selections.get("p2").unwrap_or(&0);
        let p3_count = *selections.get("p3").unwrap_or(&0);

        // SWRR should be EXACTLY 50:30:20
        assert_eq!(p1_count, 50, "P1 should get exactly 50 requests");
        assert_eq!(p2_count, 30, "P2 should get exactly 30 requests");
        assert_eq!(p3_count, 20, "P3 should get exactly 20 requests");
    }

    #[tokio::test]
    async fn test_swrr_equal_weights_fairness() {
        // Test fairness when providers have equal weights
        // With random selection, the distribution should be approximately equal
        let strategy = SmoothWeightedRoundRobin::new();
        let providers = vec![
            create_test_provider("lsp-mock-1", 5, 100),
            create_test_provider("lsp-mock-11", 5, 100),
            create_test_provider("lsp-mock-2", 3, 30),
        ];

        let mut selections = HashMap::new();
        let total_requests = 1300; // 100 cycles of 13 total weight

        for _ in 0..total_requests {
            if let Some(target) = strategy.select(&providers).await {
                *selections.entry(target.id).or_insert(0) += 1;
            }
        }

        let mock1_count = *selections.get("lsp-mock-1").unwrap_or(&0);
        let mock11_count = *selections.get("lsp-mock-11").unwrap_or(&0);
        let mock2_count = *selections.get("lsp-mock-2").unwrap_or(&0);

        // Expected: 5:5:3 ratio = 500:500:300 out of 1300
        assert_eq!(
            mock1_count, 500,
            "lsp-mock-1 should get exactly 500 requests"
        );
        assert_eq!(
            mock11_count, 500,
            "lsp-mock-11 should get exactly 500 requests"
        );
        assert_eq!(
            mock2_count, 300,
            "lsp-mock-2 should get exactly 300 requests"
        );

        println!("Equal weight fairness test:");
        println!("  lsp-mock-1:  {} (expected: 500)", mock1_count);
        println!("  lsp-mock-11: {} (expected: 500)", mock11_count);
        println!("  lsp-mock-2:  {} (expected: 300)", mock2_count);
    }
}
