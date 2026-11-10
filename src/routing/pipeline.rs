use crate::error::Result;
use crate::models::ModelProvider;
use crate::monitoring::{
    record_provider_selection_global, record_tier_failover_global,
    update_available_providers_global, update_provider_utilization_global,
};
use crate::routing::weight_adjuster::WeightAdjuster;
use crate::{
    config::RoutingConfig,
    routing::{
        filters::RateLimitFilter,
        routers::strategy::{create_routing_strategy, RoutingStrategy},
    },
    storage::{ProviderHealthInfo, RedisService},
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Context to cache Redis data during pipeline execution
pub struct PipelineContext {
    pub provider_health_cache: HashMap<String, Option<ProviderHealthInfo>>,
    pub provider_rps_cache: HashMap<String, f64>,
    pub retry_counter: u32,
}

impl PipelineContext {
    fn new() -> Self {
        Self {
            provider_health_cache: HashMap::new(),
            provider_rps_cache: HashMap::new(),
            retry_counter: 0,
        }
    }

    /// Check if this is a retry request
    /// Returns true if retry_counter > 0
    pub fn is_retry(&self) -> bool {
        self.retry_counter > 0
    }

    /// Increment retry counter
    pub fn increment_retry_counter(&mut self) {
        self.retry_counter += 1;
    }

    /// Batch load health info for all providers to avoid redundant Redis queries
    async fn load_provider_health(
        &mut self,
        provider_ids: &[String],
        redis_service: &Arc<RedisService>,
    ) -> Result<()> {
        for provider_id in provider_ids {
            if !self.provider_health_cache.contains_key(provider_id) {
                let health_info = redis_service.get_provider_health(provider_id).await?;
                self.provider_health_cache
                    .insert(provider_id.clone(), health_info);
            }
        }
        Ok(())
    }

    /// Batch load RPS data for all providers to avoid redundant Redis queries
    async fn load_provider_rps(
        &mut self,
        providers: &[ModelProvider],
        redis_service: &Arc<RedisService>,
    ) -> Result<()> {
        use crate::routing::filters::RateLimitFilter;

        let rate_filter = RateLimitFilter::new(Arc::clone(redis_service));

        for provider in providers {
            if !self.provider_rps_cache.contains_key(&provider.id) {
                let rps = rate_filter
                    .get_provider_current_usage(&provider.id, provider.max_rps as u32)
                    .await
                    .unwrap_or(0.0);
                self.provider_rps_cache.insert(provider.id.clone(), rps);
            }
        }
        Ok(())
    }

    pub fn get_provider_health(&self, provider_id: &str) -> Option<&Option<ProviderHealthInfo>> {
        self.provider_health_cache.get(provider_id)
    }

    pub fn get_provider_rps(&self, provider_id: &str) -> f64 {
        self.provider_rps_cache
            .get(provider_id)
            .copied()
            .unwrap_or(0.0)
    }
}

pub struct ProviderPipeline {
    redis_service: Option<Arc<RedisService>>,
    routing_strategy: RoutingStrategy,
    frp_enabled: bool,
    surge_percentage: f64,
}

impl ProviderPipeline {
    pub fn new(
        redis_service: Option<Arc<RedisService>>,
        routing_config: &RoutingConfig,
        frp_enabled: bool,
    ) -> Self {
        let routing_strategy = create_routing_strategy(&routing_config.strategy);
        Self {
            redis_service,
            routing_strategy,
            frp_enabled,
            surge_percentage: 0.3, // Default 30% surge capacity for retry requests
        }
    }

    /// Set surge percentage for retry requests (default 0.3 = 30%)
    pub fn with_surge_percentage(mut self, surge_percentage: f64) -> Self {
        self.surge_percentage = surge_percentage;
        self
    }

    pub fn is_frp_enabled(&self) -> bool {
        self.frp_enabled
    }

    /// Group providers by tier, returning a sorted list of (tier, providers) tuples
    /// Lower tier number = higher priority
    fn group_providers_by_tier(providers: Vec<ModelProvider>) -> Vec<(i32, Vec<ModelProvider>)> {
        let mut tier_map: HashMap<i32, Vec<ModelProvider>> = HashMap::new();

        for provider in providers {
            tier_map
                .entry(provider.tier)
                .or_insert_with(Vec::new)
                .push(provider);
        }

        // Sort by tier (ascending - lower tier = higher priority)
        let mut tiers: Vec<(i32, Vec<ModelProvider>)> = tier_map.into_iter().collect();
        tiers.sort_by_key(|(tier, _)| *tier);

        tiers
    }

    pub async fn select_provider(
        &self,
        providers: Vec<ModelProvider>,
        group: &str,
    ) -> Result<Option<ModelProvider>> {
        if providers.is_empty() {
            return Ok(None);
        }

        // Phase 1: Apply non-rate-limit filters (FRP, circuit breaker, dynamic weights)
        // NOTE: No tier failover pre-judgment here - tier switching is driven by actual rate limit results
        let available_providers = self.filter_available_providers(providers).await?;

        if available_providers.is_empty() {
            warn!(
                "❌ Group '{}': no providers available after filtering",
                group
            );
            return Ok(None);
        }

        // Phase 2: Group providers by tier
        let tiered_providers = Self::group_providers_by_tier(available_providers);

        // 📊 METRIC: Record available providers count per tier
        for (tier, tier_ps) in &tiered_providers {
            update_available_providers_global(*tier, group, tier_ps.len());
        }

        let tier_summary: Vec<String> = tiered_providers
            .iter()
            .map(|(tier, ps)| format!("tier{}:{}", tier, ps.len()))
            .collect();
        debug!("Group '{}' tiers: [{}]", group, tier_summary.join(", "));

        // Phase 3: Iterate through tiers, try to select and consume token
        // Only move to next tier when ALL providers in current tier are rate limited
        // This matches the old project's behavior exactly
        if let Some(redis_service) = self.redis_service.as_ref() {
            let rate_filter = RateLimitFilter::new(Arc::clone(redis_service));

            // Create a shared context across all tiers to track retry counter
            let mut global_context = PipelineContext::new();

            for (tier, mut tier_providers) in tiered_providers {
                // Increment retry counter for each tier (Tier 0: counter=0, Tier 1: counter=1, etc.)
                if tier > 0 {
                    global_context.increment_retry_counter();
                    debug!(
                        "🔄 Group '{}': entering tier{} (retry_counter={}, is_retry={})",
                        group,
                        tier,
                        global_context.retry_counter,
                        global_context.is_retry()
                    );
                }

                // Load health and RPS data for this tier's providers
                let provider_ids: Vec<String> =
                    tier_providers.iter().map(|p| p.id.clone()).collect();
                global_context
                    .load_provider_health(&provider_ids, redis_service)
                    .await?;
                global_context
                    .load_provider_rps(&tier_providers, redis_service)
                    .await?;

                // 📊 METRIC: Record provider utilization for each provider in this tier
                for provider in &tier_providers {
                    let current_rps = global_context.get_provider_rps(&provider.id);
                    let utilization = if provider.max_rps > 0 {
                        current_rps / provider.max_rps as f64
                    } else {
                        0.0
                    };
                    update_provider_utilization_global(&provider.id, group, utilization);
                }

                // Try to select a provider within this tier
                while !tier_providers.is_empty() {
                    // Use routing strategy to select one provider
                    let target = match self
                        .routing_strategy
                        .select(&tier_providers, &global_context)
                        .await
                    {
                        Some(t) => t,
                        None => break,
                    };

                    // Find the selected provider
                    let pos = match tier_providers.iter().position(|p| p.id == target.id) {
                        Some(p) => p,
                        None => break,
                    };

                    let candidate = &tier_providers[pos];

                    // Try to consume token immediately (atomic check-and-consume)
                    // Apply surge capacity if this is a retry (tier > 0)
                    match rate_filter
                        .consume_token(
                            candidate,
                            global_context.is_retry(),
                            self.surge_percentage,
                            group,
                        )
                        .await
                    {
                        Ok(true) => {
                            // Success: token consumed, return this provider
                            info!(
                                "🎯 Group '{}': selected {} (tier={},weight={},is_retry={})",
                                group,
                                candidate.id,
                                candidate.tier,
                                candidate.weight,
                                global_context.is_retry()
                            );

                            // 📊 METRIC: Record provider selection event
                            record_provider_selection_global(&candidate.id, candidate.tier, group);

                            return Ok(Some(candidate.clone()));
                        }
                        Ok(false) => {
                            // Rate limited: remove from tier and retry with remaining providers in same tier
                            debug!(
                                "⚠️ Group '{}': {} rate limited, trying next in tier{}",
                                group, candidate.id, tier
                            );
                            tier_providers.remove(pos);
                        }
                        Err(e) => {
                            // Error: remove and continue
                            warn!(
                                "❌ Group '{}': token error for {}: {}, trying next",
                                group, candidate.id, e
                            );
                            tier_providers.remove(pos);
                        }
                    }
                }

                // All providers in this tier exhausted, move to next tier
                // 📊 METRIC: Record tier failover event (from current tier to next)
                let next_tier = tier + 1;
                record_tier_failover_global(tier, next_tier, group);

                debug!(
                    "⚠️ Group '{}': tier{} exhausted (all rate limited), trying next tier",
                    group, tier
                );
            }

            // All tiers exhausted
            warn!(
                "❌ Group '{}': all providers in all tiers rate limited",
                group
            );
            Err(crate::error::AppError::AllProvidersRateLimited(
                group.to_string(),
            ))
        } else {
            // No Redis service, just select from first tier with providers
            for (_tier, tier_providers) in tiered_providers {
                let context = PipelineContext::new();
                if let Some(target) = self
                    .routing_strategy
                    .select(&tier_providers, &context)
                    .await
                {
                    if let Some(provider) = tier_providers.iter().find(|p| p.id == target.id) {
                        info!(
                            "🎯 Group '{}': selected {} (tier={},weight={}) - no rate limiting",
                            group, provider.id, provider.tier, provider.weight
                        );
                        return Ok(Some(provider.clone()));
                    }
                }
            }
            Ok(None)
        }
    }

    /// Apply non-rate-limit filters
    /// Rate limiting is handled atomically during selection to avoid TOCTOU issues
    /// NOTE: Tier failover is NOT done here - tier switching is driven by actual rate limit results
    async fn filter_available_providers(
        &self,
        mut providers: Vec<ModelProvider>,
    ) -> Result<Vec<ModelProvider>> {
        let initial_count = providers.len();

        // Filter 1: FRP model availability check
        providers = providers
            .into_iter()
            .filter(|provider| {
                if provider.has_frpmodel() && !self.frp_enabled {
                    debug!(
                        "Filtering out provider {} because it requires FRP but FRP is disabled",
                        provider.id
                    );
                    false
                } else {
                    true
                }
            })
            .collect();

        if let Some(redis_service) = &self.redis_service {
            // Create pipeline context and batch load all Redis data once
            let mut context = PipelineContext::new();
            let provider_ids: Vec<String> = providers.iter().map(|p| p.id.clone()).collect();
            context
                .load_provider_health(&provider_ids, redis_service)
                .await?;
            context.load_provider_rps(&providers, redis_service).await?;

            // Filter 2: Circuit breaker state
            providers = self.filter_circuit_breakers(providers, &context).await?;

            // Filter 3: Apply dynamic weights based on health scores
            providers = self.apply_dynamic_weights(providers, &context).await?;

            // NOTE: No tier failover pre-judgment here!
            // Tier switching is now driven by actual rate limit results in select_provider()
            // This matches the old project's behavior where tier is only switched when
            // ALL providers in current tier are actually rate limited (not pre-judged)
            //
            // Health and RPS data are still loaded above for:
            // 1. Circuit breaker filtering
            // 2. Dynamic weight adjustment
            // 3. Observability/logging
            // 4. Future strategy adjustments
        }

        if providers.is_empty() {
            debug!("Pipeline filtering: {} → 0 providers", initial_count);
        }

        Ok(providers)
    }

    /// Filter providers by circuit breaker state using cached data
    async fn filter_circuit_breakers(
        &self,
        providers: Vec<ModelProvider>,
        context: &PipelineContext,
    ) -> Result<Vec<ModelProvider>> {
        let mut filtered_providers = Vec::new();
        let mut circuit_open_ids = Vec::new();

        for provider in providers {
            match context.get_provider_health(&provider.id) {
                Some(Some(health_info)) if health_info.circuit_state == "open" => {
                    // Circuit breaker is open - filter out this provider
                    circuit_open_ids.push(provider.id.clone());
                }
                _ => {
                    // Allow through if healthy, no health info, or Redis error (fail-open)
                    filtered_providers.push(provider);
                }
            }
        }

        if !circuit_open_ids.is_empty() {
            debug!("🔌 Circuit breaker open: [{}]", circuit_open_ids.join(", "));
        }

        Ok(filtered_providers)
    }

    /// Apply dynamic weights based on health scores using cached data
    async fn apply_dynamic_weights(
        &self,
        providers: Vec<ModelProvider>,
        context: &PipelineContext,
    ) -> Result<Vec<ModelProvider>> {
        if providers.is_empty() {
            return Ok(providers);
        }

        let weight_adjuster = WeightAdjuster::new();
        Ok(weight_adjuster.adjust_weights(providers, &context.provider_health_cache))
    }
}
