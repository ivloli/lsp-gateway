use crate::models::ModelProvider;
use crate::storage::ProviderHealthInfo;
use std::collections::HashMap;
use tracing::{debug, info};

/// Default reason for supervisor weight updates
const DEFAULT_SUPERVISOR_REASON: &str = "supervisor_update";

/// Unified weight adjuster for gradual provider weight adjustments
/// Gateway 层只负责执行 Supervisor 计算的权重调整结果
pub struct WeightAdjuster {}

impl WeightAdjuster {
    pub fn new() -> Self {
        Self {}
    }

    /// Apply Supervisor-calculated weight adjustments
    pub fn adjust_weights(
        &self,
        mut providers: Vec<ModelProvider>,
        health_cache: &HashMap<String, Option<ProviderHealthInfo>>,
    ) -> Vec<ModelProvider> {
        if providers.is_empty() {
            return providers;
        }

        debug!(
            "Applying Supervisor-calculated weights for {} providers",
            providers.len()
        );

        // Apply weight updates using cached health data
        for provider in providers.iter_mut() {
            let original_weight = provider.weight;
            let health_info = health_cache.get(&provider.id).and_then(|opt| opt.as_ref());

            let supervisor_weight = health_info.and_then(|info| info.supervisor_weight);

            // Apply supervisor weight update if available
            if let Some(new_weight) = supervisor_weight {
                if provider.weight != new_weight {
                    let reason = health_info
                        .and_then(|info| info.weight_update_reason.as_deref())
                        .unwrap_or(DEFAULT_SUPERVISOR_REASON);

                    info!(
                        "Applying Supervisor weight update for {}: {} -> {} (reason: {})",
                        provider.id, provider.weight, new_weight, reason
                    );

                    provider.weight = new_weight;
                    crate::monitoring::update_provider_weight_global(
                        &provider.id,
                        &provider.group_name,
                        new_weight,
                    );
                }
            }

            // Always update dynamic weights in Prometheus (original from DB, supervisor override if exists)
            crate::monitoring::update_dynamic_weights_global(
                &provider.id,
                &provider.group_name,
                original_weight,
                supervisor_weight,
            );
        }

        providers
    }
}
