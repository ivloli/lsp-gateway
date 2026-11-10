/// Tier Failover Logic
///
/// Implements the logic to determine when to failover from one tier to the next
/// based on provider health and utilization metrics.
use crate::models::ModelProvider;
use crate::storage::ProviderHealthInfo;
use std::collections::HashMap;
use tracing::info;

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

/// Thresholds for tier failover decisions
pub struct TierFailoverThresholds {
    /// Minimum availability to consider a provider healthy
    pub healthy_availability: f64,
    /// Maximum utilization before considering a provider overloaded
    pub max_utilization: f64,
    /// Probe traffic percentage (0-100) for single-provider tiers to prevent starvation
    pub probe_traffic_percentage: u32,
}

impl Default for TierFailoverThresholds {
    fn default() -> Self {
        Self {
            healthy_availability: 0.90,
            max_utilization: 1.0,
            probe_traffic_percentage: 1, // 1% probe traffic
        }
    }
}

/// Provider metrics for tier failover evaluation
#[derive(Debug, Clone)]
pub struct ProviderMetrics {
    pub provider_id: String,
    pub tier: i32,
    pub availability: f64,
    pub utilization: f64,
}

impl ProviderMetrics {
    /// Check if this provider is healthy based on availability
    pub fn is_healthy(&self, thresholds: &TierFailoverThresholds) -> bool {
        self.availability >= thresholds.healthy_availability
    }

    /// Check if this provider is overloaded based on utilization
    pub fn is_overloaded(&self, thresholds: &TierFailoverThresholds) -> bool {
        self.utilization >= thresholds.max_utilization
    }
}

/// Tier failover evaluator
pub struct TierFailoverEvaluator {
    thresholds: TierFailoverThresholds,
}

impl TierFailoverEvaluator {
    pub fn new() -> Self {
        Self {
            thresholds: TierFailoverThresholds::default(),
        }
    }

    pub fn with_thresholds(thresholds: TierFailoverThresholds) -> Self {
        Self { thresholds }
    }

    /// Determine if a tier should failover to the next tier
    ///
    /// Returns true if ANY of the following conditions are met:
    /// 1. All providers are unhealthy (availability < 0.90)
    /// 2. All providers are overloaded (utilization >= 1.0)
    pub fn should_failover(&self, tier_metrics: &[ProviderMetrics]) -> bool {
        if tier_metrics.is_empty() {
            return true;
        }

        // Condition 1: Check if all providers are unhealthy
        let healthy_count = tier_metrics
            .iter()
            .filter(|p| p.is_healthy(&self.thresholds))
            .count();

        if healthy_count == 0 {
            return true;
        }

        // Condition 2: Check if all providers are overloaded
        let not_overloaded_count = tier_metrics
            .iter()
            .filter(|p| !p.is_overloaded(&self.thresholds))
            .count();

        if not_overloaded_count == 0 {
            return true;
        }

        false
    }

    /// Filter providers by tier and apply failover logic
    ///
    /// Returns providers from the appropriate tier(s) based on failover conditions.
    /// Starts with the lowest tier and progressively checks higher tiers if needed.
    ///
    /// **Multi-tier support**: Continues checking all tiers until finding a healthy one.
    /// If all tiers are unhealthy, applies probe traffic mechanism and returns lowest tier.
    pub fn filter_providers_with_failover(
        &self,
        providers: Vec<ModelProvider>,
        health_cache: &HashMap<String, Option<ProviderHealthInfo>>,
        rps_cache: &HashMap<String, f64>,
    ) -> Vec<ModelProvider> {
        if providers.is_empty() {
            return providers;
        }

        // Group providers by tier
        let mut providers_by_tier: HashMap<i32, Vec<ModelProvider>> = HashMap::new();
        for provider in providers {
            providers_by_tier
                .entry(provider.tier)
                .or_insert_with(Vec::new)
                .push(provider);
        }

        // Get sorted tiers (lowest to highest)
        let mut tiers: Vec<i32> = providers_by_tier.keys().copied().collect();
        tiers.sort();

        // Phase 1: Try to find a healthy tier
        for &tier in tiers.iter() {
            let tier_providers = providers_by_tier.get(&tier).unwrap();

            // Calculate metrics for this tier
            let tier_metrics: Vec<ProviderMetrics> = tier_providers
                .iter()
                .map(|p| self.get_provider_metrics(p, health_cache, rps_cache))
                .collect();

            // Build compact metrics summary for logging
            let metrics_summary: Vec<String> = tier_metrics
                .iter()
                .map(|m| {
                    format!(
                        "{}(avail={:.0}%,util={:.0}%)",
                        m.provider_id,
                        m.availability * 100.0,
                        m.utilization * 100.0
                    )
                })
                .collect();

            // Check if this tier is healthy
            if !self.should_failover(&tier_metrics) {
                // Found a healthy tier, use it
                info!(
                    "✅ Tier{} selected (healthy): [{}]",
                    tier,
                    format_list_truncated(&metrics_summary, 5)
                );
                return tier_providers.clone();
            }

            // This tier is unhealthy, continue to next tier
            info!(
                "⚠️ Tier{} unhealthy, checking next tier: [{}]",
                tier,
                format_list_truncated(&metrics_summary, 5)
            );

            if self.thresholds.probe_traffic_percentage > 0 {
                use rand::{thread_rng, Rng};
                let mut rng = thread_rng();
                let probe_chance = rng.gen_range(0..100);

                if probe_chance < self.thresholds.probe_traffic_percentage {
                    let tier_metrics: Vec<ProviderMetrics> = tier_providers
                        .iter()
                        .map(|p| self.get_provider_metrics(p, health_cache, rps_cache))
                        .collect();

                    let metrics_summary: Vec<String> = tier_metrics
                        .iter()
                        .map(|m| {
                            format!(
                                "{}(avail={:.0}%,util={:.0}%)",
                                m.provider_id,
                                m.availability * 100.0,
                                m.utilization * 100.0
                            )
                        })
                        .collect();

                    info!(
                        "🔬 Probe traffic ({}% chance) to Tier{} (single provider): [{}]",
                        self.thresholds.probe_traffic_percentage,
                        tier,
                        format_list_truncated(&metrics_summary, 5)
                    );
                    return tier_providers.clone();
                }
            }
        }
        // If no healthy tier found, return the lowest tier
        let lowest_tier = tiers.first().unwrap();
        let lowest_tier_providers = providers_by_tier.get(lowest_tier).unwrap();
        info!(
            "🔄 No healthy tier found, using lowest tier: Tier{}",
            lowest_tier
        );
        return lowest_tier_providers.clone();
    }

    /// Get provider metrics from caches
    fn get_provider_metrics(
        &self,
        provider: &ModelProvider,
        health_cache: &HashMap<String, Option<ProviderHealthInfo>>,
        rps_cache: &HashMap<String, f64>,
    ) -> ProviderMetrics {
        // Get availability from health cache
        let availability = health_cache
            .get(&provider.id)
            .and_then(|opt| opt.as_ref())
            .map(|h| h.availability)
            .unwrap_or(1.0); // Default to healthy if no data

        // Calculate utilization from RPS cache
        let current_rps = rps_cache.get(&provider.id).copied().unwrap_or(0.0);
        let utilization = if provider.max_rps > 0 {
            current_rps / provider.max_rps as f64
        } else {
            0.0
        };

        ProviderMetrics {
            provider_id: provider.id.clone(),
            tier: provider.tier,
            availability,
            utilization,
        }
    }
}
