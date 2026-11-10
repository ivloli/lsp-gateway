use crate::config::env_utils;
use crate::error::Result;
use crate::models::ModelProvider;
use crate::routing::filters::RateLimitFilter;
use crate::storage::{ProviderHealthInfo, RedisService};

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

fn format_list_truncated(items: &[String], max_items: usize) -> String {
    if items.is_empty() {
        return String::new();
    }

    if items.len() <= max_items {
        return items.join(", ");
    }

    let shown: Vec<_> = items.iter().take(max_items).cloned().collect();
    format!(
        "{}, ... and {} more",
        shown.join(", "),
        items.len() - max_items
    )
}

// ============================================================================
// Constants - Unified
// ============================================================================

const MIN_WEIGHT: i32 = 1;
const COOLDOWN_SECONDS: u64 = 5;

// Availability thresholds
const CRITICAL_THRESHOLD: f64 = 0.80;
const SEVERE_THRESHOLD: f64 = 0.90;
const MODERATE_THRESHOLD: f64 = 0.95;

// Weight multipliers
const SEVERE_MULTIPLIER: f64 = 0.5;
const MODERATE_MULTIPLIER: f64 = 0.8;
const RECOVERY_MULTIPLIER: f64 = 1.25;

// History constants
const MAX_HISTORY_SIZE: usize = 20;
const HISTORY_TTL_SECONDS: u64 = 30;

// EWMA smoothing factor (0.0-1.0)
// Higher value = more weight on recent data
// 0.3 means: 30% current value, 70% historical average
const EWMA_ALPHA: f64 = 0.3;

// ============================================================================
// Type Aliases
// ============================================================================

pub type ProviderId = String;
pub type Weight = i32;
pub type Timestamp = u64;
pub type Availability = f64;
pub type Utilization = f64;

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
pub struct WeightConfig {
    pub healthy_threshold: f64,
    pub max_safe_utilization: f64,
    pub state_ttl_seconds: u64,
    pub enable_aggressive_reallocation: bool,
    pub aggressive_min_weight_ratio: f64,
}

impl Default for WeightConfig {
    fn default() -> Self {
        Self {
            healthy_threshold: 0.5,
            max_safe_utilization: 0.9,
            state_ttl_seconds: 3600,
            enable_aggressive_reallocation: env_utils::env_or_default(
                "AGGRESSIVE_WEIGHT_REALLOCATION",
                false,
            ),
            aggressive_min_weight_ratio: 0.1,
        }
    }
}

// ============================================================================
// Core Data Structures
// ============================================================================

#[derive(Debug, Clone)]
pub struct ProviderData {
    pub id: String,
    pub group: String,
    pub tier: i32,
    pub original_weight: i32,
    pub current_weight: i32,
    pub max_rps: u32,
    pub current_rps: f64,
    pub availability: f64,
    pub availability_history: Vec<f64>,
    pub last_adjustment_time: Option<u64>,
}

impl ProviderData {
    pub fn utilization(&self) -> f64 {
        if self.max_rps > 0 {
            // Ensure utilization is capped at 1.0
            (self.current_rps / self.max_rps as f64).min(1.0)
        } else {
            0.0
        }
    }

    pub fn is_healthy(&self, config: &WeightConfig) -> bool {
        self.availability > config.healthy_threshold
    }

    pub fn is_overloaded(&self, config: &WeightConfig) -> bool {
        self.utilization() > config.max_safe_utilization
    }

    /// Calculate stable availability using Exponential Weighted Moving Average (EWMA)
    ///
    /// This gives more weight to recent data while smoothing out short-term fluctuations.
    /// Formula: EWMA = α * current + (1 - α) * previous_ewma
    ///
    /// Benefits:
    /// - Recent data has higher weight (responds faster to real changes)
    /// - Old data gradually decays (prevents stale data from affecting decisions)
    /// - Smooth transitions (avoids oscillation near thresholds)
    pub fn stable_availability(&self) -> f64 {
        // if self.availability_history.len() >= 3 {
        //     let mut total = self.availability_history.iter().sum::<f64>();
        //     total += self.availability;
        //     total / (self.availability_history.len() + 1) as f64
        // } else {
        //     self.availability
        // }
        if self.availability_history.is_empty() {
            return self.availability;
        }

        // Calculate EWMA from historical data
        let mut ewma = self.availability_history[0];
        for &value in &self.availability_history[1..] {
            ewma = EWMA_ALPHA * value + (1.0 - EWMA_ALPHA) * ewma;
        }

        // Apply current value with EWMA
        ewma = EWMA_ALPHA * self.availability + (1.0 - EWMA_ALPHA) * ewma;

        ewma
    }

    pub fn is_in_cooldown(&self) -> bool {
        if let Some(last_time) = self.last_adjustment_time {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            (now - last_time) < COOLDOWN_SECONDS
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PerformanceLevel {
    Poor,
    Normal,
    Excellent,
}

#[derive(Debug, Clone)]
pub enum AdjustmentReason {
    HealthDegraded(Availability),
    CapacityReallocation(Utilization),
    Recovery,
    Rebalancing,
}

impl std::fmt::Display for AdjustmentReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdjustmentReason::HealthDegraded(avail) => {
                write!(f, "health_degraded:availability={:.2}", avail)
            }
            AdjustmentReason::CapacityReallocation(util) => {
                write!(f, "capacity_reallocation:utilization={:.2}", util)
            }
            AdjustmentReason::Recovery => write!(f, "recovery"),
            AdjustmentReason::Rebalancing => write!(f, "rebalancing"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeightAdjustment {
    pub provider_id: ProviderId,
    pub from_weight: Weight,
    pub to_weight: Weight,
    pub reason: AdjustmentReason,
    pub timestamp: Timestamp,
}

impl WeightAdjustment {
    pub fn new(
        provider_id: ProviderId,
        from_weight: Weight,
        to_weight: Weight,
        reason: AdjustmentReason,
    ) -> Self {
        Self {
            provider_id,
            from_weight,
            to_weight,
            reason,
            timestamp: crate::utils::TimeUtils::current_timestamp_secs(),
        }
    }

    pub fn weight_change(&self) -> Weight {
        self.to_weight - self.from_weight
    }

    pub fn is_increase(&self) -> bool {
        self.to_weight > self.from_weight
    }

    pub fn is_decrease(&self) -> bool {
        self.to_weight < self.from_weight
    }
}

// Keep for backward compatibility
pub type WeightUpdate = WeightAdjustment;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupState {
    pub group_name: String,
    pub total_weight: i32,
    pub target_weights: HashMap<String, i32>,
    pub cycle_id: u64,
}

impl GroupState {
    pub fn new(group_name: String, providers: &[ProviderData]) -> Self {
        let total_weight = providers.iter().map(|p| p.original_weight).sum();
        let target_weights = providers
            .iter()
            .map(|p| (p.id.clone(), p.original_weight))
            .collect();

        Self {
            group_name,
            total_weight,
            target_weights,
            cycle_id: 0,
        }
    }

    pub fn next_cycle(&mut self) {
        self.cycle_id += 1;
    }
}

// ============================================================================
// Weight Calculation Logic - Simplified
// ============================================================================

/// Calculate weight multiplier based on availability
fn weight_multiplier(availability: f64) -> f64 {
    if availability < CRITICAL_THRESHOLD {
        0.0 // Will be clamped to MIN_WEIGHT
    } else if availability < SEVERE_THRESHOLD {
        SEVERE_MULTIPLIER
    } else if availability < MODERATE_THRESHOLD {
        MODERATE_MULTIPLIER
    } else {
        RECOVERY_MULTIPLIER
    }
}

/// Calculate new weight for a provider
fn calculate_provider_weight(provider: &ProviderData) -> i32 {
    if provider.is_in_cooldown() {
        return provider.current_weight;
    }

    let avail = provider.stable_availability();
    let multiplier = weight_multiplier(avail);

    let new_weight = if multiplier == 0.0 {
        MIN_WEIGHT
    } else {
        let adjusted = (provider.current_weight as f64 * multiplier) as i32;
        adjusted.min(provider.original_weight).max(MIN_WEIGHT)
    };

    new_weight
}

/// Distribute weight difference to healthy providers
fn redistribute_weights(
    providers: &[ProviderData],
    weights: &[i32],
    total_weight: i32,
) -> Vec<i32> {
    let current_total: i32 = weights.iter().sum();
    let mut diff = total_weight - current_total;

    if diff == 0 {
        return weights.to_vec();
    }

    let mut result = weights.to_vec();
    let healthy_indices: Vec<usize> = providers
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            if p.stable_availability() >= MODERATE_THRESHOLD {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    if healthy_indices.is_empty() {
        // Emergency: give to largest
        if let Some((i, _)) = result.iter().enumerate().max_by_key(|(_, &w)| w) {
            result[i] += diff;
        }
        return result;
    }

    // Distribute evenly
    let per_provider = diff / healthy_indices.len() as i32;
    let remainder = diff % healthy_indices.len() as i32;

    for (idx, &i) in healthy_indices.iter().enumerate() {
        let extra = per_provider + if idx < remainder as usize { 1 } else { 0 };

        // First try to fill within original_weight limit
        if result[i] < providers[i].original_weight {
            let can_add = (providers[i].original_weight - result[i]).min(extra);
            result[i] += can_add;
            diff -= can_add;
        }
    }

    // Distribute remaining (may exceed original_weight)
    if diff > 0 && !healthy_indices.is_empty() {
        let per_provider = diff / healthy_indices.len() as i32;
        let remainder = diff % healthy_indices.len() as i32;
        for (idx, &i) in healthy_indices.iter().enumerate() {
            result[i] += per_provider + if idx < remainder as usize { 1 } else { 0 };
        }
    }

    result
}

/// Group providers by tier
fn group_by_tier(providers: &[ProviderData]) -> HashMap<i32, Vec<ProviderData>> {
    let mut grouped = HashMap::new();
    for provider in providers {
        grouped
            .entry(provider.tier)
            .or_insert_with(Vec::new)
            .push(provider.clone());
    }
    grouped
}

/// Find first tier with healthy providers
fn find_active_tier(
    providers_by_tier: &HashMap<i32, Vec<ProviderData>>,
    sorted_tiers: &[i32],
    config: &WeightConfig,
) -> Option<i32> {
    for &tier in sorted_tiers {
        if let Some(tier_providers) = providers_by_tier.get(&tier) {
            if tier_providers.iter().any(|p| p.is_healthy(config)) {
                return Some(tier);
            }
        }
    }
    None
}

/// Calculate weights for a tier
fn calculate_tier_weights(providers: &[ProviderData], tier_total: i32) -> HashMap<String, i32> {
    let availability_weights: Vec<i32> = providers.iter().map(calculate_provider_weight).collect();

    let redistributed = redistribute_weights(providers, &availability_weights, tier_total);

    providers
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), redistributed[i]))
        .collect()
}

/// Calculate target weights for all providers
fn calculate_target_weights(
    providers: &[ProviderData],
    config: &WeightConfig,
) -> HashMap<ProviderId, Weight> {
    let providers_by_tier = group_by_tier(providers);
    let mut sorted_tiers: Vec<i32> = providers_by_tier.keys().cloned().collect();
    sorted_tiers.sort();

    let active_tier = find_active_tier(&providers_by_tier, &sorted_tiers, config);

    let mut weights = HashMap::new();

    for (&tier, tier_providers) in &providers_by_tier {
        if Some(tier) == active_tier {
            let tier_total: i32 = tier_providers.iter().map(|p| p.original_weight).sum();
            let tier_weights = calculate_tier_weights(tier_providers, tier_total);
            weights.extend(tier_weights);
        } else {
            // Standby tiers keep original weights
            for provider in tier_providers {
                weights.insert(provider.id.clone(), provider.original_weight);
            }
        }
    }

    // Fallback: if no healthy tier, return original weights
    if weights.is_empty() {
        for provider in providers {
            weights.insert(provider.id.clone(), provider.original_weight);
        }
    }

    weights
}

// ============================================================================
// State Management
// ============================================================================

pub struct StateManager {
    redis: Arc<RedisService>,
    config: WeightConfig,
}

impl StateManager {
    pub fn new(redis: Arc<RedisService>, config: WeightConfig) -> Self {
        Self { redis, config }
    }

    pub async fn load_state(&self, group: &str, providers: &[ProviderData]) -> Result<GroupState> {
        let key = format!("lsp:group_state:{}", group);

        if let Ok(data) = self.redis.get_string(&key).await {
            if let Ok(mut state) = serde_json::from_str::<GroupState>(&data) {
                let current_total: i32 = providers.iter().map(|p| p.original_weight).sum();

                if state.total_weight != current_total {
                    warn!(
                        "Total weight mismatch for group {}: cached={}, actual={}, updating cache",
                        group, state.total_weight, current_total
                    );
                    state.total_weight = current_total;

                    for provider in providers {
                        state
                            .target_weights
                            .insert(provider.id.clone(), provider.original_weight);
                    }

                    self.save_state(&state).await?;
                }

                debug!(
                    "Loaded existing state for group {} (total_weight: {})",
                    group, state.total_weight
                );
                return Ok(state);
            }
        }

        debug!("Creating new state for group {}", group);
        Ok(GroupState::new(group.to_string(), providers))
    }

    pub async fn save_state(&self, state: &GroupState) -> Result<()> {
        let key = format!("lsp:group_state:{}", state.group_name);
        let data = serde_json::to_string(state)
            .map_err(|e| crate::error::AppError::InvalidRequest(e.to_string()))?;

        self.redis
            .set_string(&key, &data, Some(self.config.state_ttl_seconds))
            .await?;
        debug!("Saved state for group {}", state.group_name);
        Ok(())
    }

    pub async fn save_last_adjustment_time(&self, provider_id: &str) -> Result<()> {
        let key = format!("lsp:routing:provider:{}", provider_id);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.redis
            .hset(&key, "weight_adjustment_time", &now.to_string())
            .await?;
        Ok(())
    }

    pub async fn load_last_adjustment_time(&self, provider_id: &str) -> Result<Option<u64>> {
        let key = format!("lsp:routing:provider:{}", provider_id);

        match self.redis.hget(&key, "weight_adjustment_time").await {
            Ok(Some(data)) => data.parse::<u64>().map(Some).or(Ok(None)),
            Ok(None) | Err(_) => Ok(None),
        }
    }
}

// ============================================================================
// Logging - Separated from business logic
// ============================================================================

pub struct WeightAdjustmentLogger;

impl WeightAdjustmentLogger {
    pub fn log_calculation_start(group: &str, provider_count: usize, total_weight: Weight) {
        info!(
            "⚖️ Weight calc started: group='{}', providers={}, total_weight={}",
            group, provider_count, total_weight
        );
    }

    pub fn log_tier_analysis(tier_stats: &HashMap<i32, (usize, usize)>, active_tier: Option<i32>) {
        let mut sorted_tiers: Vec<_> = tier_stats.keys().collect();
        sorted_tiers.sort();

        let tier_summary: Vec<String> = sorted_tiers
            .iter()
            .map(|&&tier| {
                let (total, healthy) = tier_stats[&tier];
                let health_rate = if total > 0 {
                    (healthy as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                format!("T{}({}/{}={:.0}%)", tier, healthy, total, health_rate)
            })
            .collect();

        let active_tier_str = active_tier
            .map(|t| format!("T{}", t))
            .unwrap_or_else(|| "emergency".to_string());
        info!(
            "📊 Tiers: [{}], active={}",
            tier_summary.join(", "),
            active_tier_str
        );
    }

    pub fn log_weight_adjustments(adjustments: &[WeightAdjustment], providers: &[ProviderData]) {
        if adjustments.is_empty() {
            debug!("No weight changes required");
            return;
        }

        let provider_map: HashMap<&str, &ProviderData> =
            providers.iter().map(|p| (p.id.as_str(), p)).collect();

        let adjustment_summary: Vec<String> = adjustments
            .iter()
            .map(|adj| {
                let change = adj.weight_change();
                let change_pct = if adj.from_weight > 0 {
                    (change as f64 / adj.from_weight as f64) * 100.0
                } else {
                    0.0
                };

                let direction = if change > 0 {
                    "↑"
                } else if change < 0 {
                    "↓"
                } else {
                    "="
                };

                let context = provider_map
                    .get(adj.provider_id.as_str())
                    .map(|p| {
                        format!(
                            "t{}:a{:.0}%:u{:.0}%",
                            p.tier,
                            p.availability * 100.0,
                            p.utilization() * 100.0
                        )
                    })
                    .unwrap_or_default();

                let reason_short = match &adj.reason {
                    AdjustmentReason::HealthDegraded(_) => "health",
                    AdjustmentReason::CapacityReallocation(_) => "realloc",
                    AdjustmentReason::Recovery => "recovery",
                    AdjustmentReason::Rebalancing => "rebalance",
                };

                format!(
                    "{}{}:{}→{}({:+.0}%,{})",
                    direction,
                    adj.provider_id,
                    adj.from_weight,
                    adj.to_weight,
                    change_pct,
                    if context.is_empty() {
                        reason_short.to_string()
                    } else {
                        format!("{},{}", context, reason_short)
                    }
                )
            })
            .collect();

        info!(
            "⚖️ Weight adjustments({}): [{}]",
            adjustments.len(),
            format_list_truncated(&adjustment_summary, 8)
        );
    }

    pub fn log_calculation_summary(
        group: &str,
        adjustments: &[WeightAdjustment],
        providers: &[ProviderData],
        original_total: Weight,
        final_total: Weight,
    ) {
        let total_weight_moved = adjustments
            .iter()
            .map(|adj| adj.weight_change().abs())
            .sum::<Weight>();

        let increases = adjustments.iter().filter(|adj| adj.is_increase()).count();
        let decreases = adjustments.iter().filter(|adj| adj.is_decrease()).count();

        let conservation_status = if original_total == final_total {
            "✓".to_string()
        } else {
            format!("✗drift{:+}", final_total - original_total)
        };

        info!(
            "✅ Weight calc done: group='{}', changed={}/{}, ↑{}↓{}, moved={}, total={}→{}({})",
            group,
            adjustments.len(),
            providers.len(),
            increases,
            decreases,
            total_weight_moved,
            original_total,
            final_total,
            conservation_status
        );
    }
}

// ============================================================================
// Weight Calculation Service
// ============================================================================

pub struct WeightCalculationService {
    redis: Arc<RedisService>,
    db_pool: Arc<sqlx::PgPool>,
    config: WeightConfig,
    state_manager: StateManager,
}

impl WeightCalculationService {
    pub fn new(redis: Arc<RedisService>, db_pool: Arc<sqlx::PgPool>) -> Self {
        let config = WeightConfig::default();
        let state_manager = StateManager::new(Arc::clone(&redis), config.clone());

        Self {
            redis,
            db_pool,
            config,
            state_manager,
        }
    }

    pub async fn calculate_and_store_group_weights(
        &self,
        group: &str,
        health_infos: &mut [ProviderHealthInfo],
    ) -> Result<()> {
        let providers = self.collect_provider_data(group).await?;
        if providers.is_empty() {
            debug!("No providers found for group {}", group);
            return Ok(());
        }

        let original_total: Weight = providers.iter().map(|p| p.original_weight).sum();

        WeightAdjustmentLogger::log_calculation_start(group, providers.len(), original_total);

        let mut state = self.state_manager.load_state(group, &providers).await?;

        // Calculate tier stats for logging
        let providers_by_tier = group_by_tier(&providers);
        let mut tier_stats = HashMap::new();
        for (&tier, tier_providers) in &providers_by_tier {
            let total_count = tier_providers.len();
            let healthy_count = tier_providers
                .iter()
                .filter(|p| p.is_healthy(&self.config))
                .count();
            tier_stats.insert(tier, (total_count, healthy_count));
        }

        let mut sorted_tiers: Vec<i32> = providers_by_tier.keys().cloned().collect();
        sorted_tiers.sort();
        let active_tier = find_active_tier(&providers_by_tier, &sorted_tiers, &self.config);
        WeightAdjustmentLogger::log_tier_analysis(&tier_stats, active_tier);

        let targets = calculate_target_weights(&providers, &self.config);

        let new_weights: Vec<i32> = providers
            .iter()
            .map(|provider| {
                targets
                    .get(&provider.id)
                    .copied()
                    .unwrap_or(provider.current_weight)
            })
            .collect();

        let updates = self.generate_adjustments(&providers, &new_weights, &targets);
        WeightAdjustmentLogger::log_weight_adjustments(&updates, &providers);

        // Fix weight conservation
        let targets_total: i32 = targets.values().sum();
        let mut final_targets = targets;

        if targets_total != original_total {
            warn!(
                "Fixing weight conservation: targets_total={}, original_total={}",
                targets_total, original_total
            );
            if let Some((largest_id, _)) = final_targets.iter().max_by_key(|(_, &weight)| weight) {
                let adjustment = original_total - targets_total;
                final_targets = final_targets
                    .iter()
                    .map(|(id, &weight)| {
                        if id == largest_id {
                            (id.clone(), weight + adjustment)
                        } else {
                            (id.clone(), weight)
                        }
                    })
                    .collect();
            }
        }

        state.target_weights = final_targets;
        state.total_weight = original_total;
        state.next_cycle();
        self.state_manager.save_state(&state).await?;

        for update in &updates {
            if let Err(e) = self
                .state_manager
                .save_last_adjustment_time(&update.provider_id)
                .await
            {
                warn!(
                    "Failed to save adjustment time for {}: {}",
                    update.provider_id, e
                );
            }
        }

        self.apply_adjustments_to_health_infos(health_infos, &updates);
        self.apply_utilization_to_health_infos(health_infos, &providers);

        let final_total: Weight = new_weights.iter().sum();
        WeightAdjustmentLogger::log_calculation_summary(
            group,
            &updates,
            &providers,
            original_total,
            final_total,
        );

        Ok(())
    }

    async fn collect_provider_data(&self, group: &str) -> Result<Vec<ProviderData>> {
        let providers = ModelProvider::find_active_by_group(&self.db_pool, group)
            .await
            .map_err(crate::error::AppError::Database)?;

        let mut provider_data = Vec::new();

        for provider in providers {
            let current_rps = self
                .get_current_rps(&provider.id, provider.max_rps as u32)
                .await;
            let health_info = self.redis.get_provider_health(&provider.id).await?;
            let availability = health_info.as_ref().map(|h| h.availability).unwrap_or(1.0);

            let current_weight = health_info
                .as_ref()
                .and_then(|h| h.supervisor_weight)
                .unwrap_or(provider.weight);

            let last_adjustment_time = self
                .state_manager
                .load_last_adjustment_time(&provider.id)
                .await
                .ok()
                .flatten();

            let availability_history = self.get_availability_history(&provider.id).await;

            provider_data.push(ProviderData {
                id: provider.id.clone(),
                group: provider.group_name,
                tier: provider.tier,
                original_weight: provider.weight,
                current_weight,
                max_rps: provider.max_rps as u32,
                current_rps,
                availability,
                availability_history,
                last_adjustment_time,
            });

            self.update_availability_history(&provider.id, availability)
                .await;
        }

        Ok(provider_data)
    }

    async fn get_current_rps(&self, provider_id: &str, max_rps: u32) -> f64 {
        let rate_filter = RateLimitFilter::new(Arc::clone(&self.redis));
        rate_filter
            .get_provider_current_usage(provider_id, max_rps)
            .await
            .unwrap_or(0.0)
    }

    async fn get_availability_history(&self, provider_id: &str) -> Vec<f64> {
        let key = format!("lsp:availability_history:{}", provider_id);

        match self.redis.get_string(&key).await {
            Ok(data) => serde_json::from_str::<Vec<f64>>(&data).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    async fn update_availability_history(&self, provider_id: &str, availability: f64) {
        let key = format!("lsp:availability_history:{}", provider_id);
        let mut history = self.get_availability_history(provider_id).await;
        history.push(availability);

        if history.len() > MAX_HISTORY_SIZE {
            let skip_count = history.len() - MAX_HISTORY_SIZE;
            history = history.into_iter().skip(skip_count).collect();
        }

        if let Ok(data) = serde_json::to_string(&history) {
            if let Err(e) = self
                .redis
                .set_string(&key, &data, Some(HISTORY_TTL_SECONDS))
                .await
            {
                warn!(
                    "Failed to update availability history for {}: {}",
                    provider_id, e
                );
            }
        }
    }

    fn generate_adjustments(
        &self,
        providers: &[ProviderData],
        new_weights: &[Weight],
        _targets: &HashMap<ProviderId, Weight>,
    ) -> Vec<WeightAdjustment> {
        providers
            .iter()
            .zip(new_weights.iter())
            .filter_map(|(provider, &new_weight)| {
                if new_weight != provider.current_weight {
                    let reason = self.determine_adjustment_reason(provider, new_weight);
                    Some(WeightAdjustment::new(
                        provider.id.clone(),
                        provider.current_weight,
                        new_weight,
                        reason,
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    fn determine_adjustment_reason(
        &self,
        provider: &ProviderData,
        new_weight: i32,
    ) -> AdjustmentReason {
        let stable_avail = provider.stable_availability();

        if new_weight < provider.current_weight {
            if stable_avail < MODERATE_THRESHOLD {
                AdjustmentReason::HealthDegraded(stable_avail)
            } else {
                AdjustmentReason::CapacityReallocation(provider.utilization())
            }
        } else {
            AdjustmentReason::Recovery
        }
    }

    fn apply_adjustments_to_health_infos(
        &self,
        health_infos: &mut [ProviderHealthInfo],
        adjustments: &[WeightAdjustment],
    ) {
        let adjustment_map: HashMap<&str, &WeightAdjustment> = adjustments
            .iter()
            .map(|adj| (adj.provider_id.as_str(), adj))
            .collect();

        for health_info in health_infos.iter_mut() {
            if let Some(adjustment) = adjustment_map.get(health_info.provider_id.as_str()) {
                health_info.supervisor_weight = Some(adjustment.to_weight);
                health_info.weight_update_reason = Some(adjustment.reason.to_string());
                health_info.weight_updated_at = Some(adjustment.timestamp);
                health_info.weight_version = Some(adjustment.timestamp);
            }
        }
    }

    fn apply_utilization_to_health_infos(
        &self,
        health_infos: &mut [ProviderHealthInfo],
        providers: &[ProviderData],
    ) {
        for health_info in health_infos.iter_mut() {
            if let Some(provider) = providers.iter().find(|p| p.id == health_info.provider_id) {
                health_info.utilization = Some(provider.utilization());
            }
        }
    }
}
