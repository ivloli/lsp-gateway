//! Traffic Control Domain
//!
//! This domain handles circuit breaker functionality and error rate switching for fault tolerance.

pub mod breaker;
pub mod error_rate_switcher;

// Re-exports for domain API
pub use breaker::CircuitBreaker;
pub use error_rate_switcher::ErrorRateSwitcher;

use crate::monitoring::supervisor::{
    config::SupervisorConfig, health::ProviderHealthInfo as HealthProviderInfo,
};
use crate::storage::{ProviderHealthInfo as StorageHealthInfo, RedisService};
use std::sync::Arc;
use tracing::debug;

/// Utility for converting between different health info types
struct HealthInfoConverter;

impl HealthInfoConverter {
    /// Convert from health domain to storage domain
    fn health_to_storage(health_info: &HealthProviderInfo) -> StorageHealthInfo {
        StorageHealthInfo {
            provider_id: health_info.provider_id.clone(),
            health_score: health_info.health_score,
            circuit_state: "CLOSED".to_string(), // Will be updated by circuit breaker
            availability: health_info.health_score,
            last_updated: health_info.last_updated,
            state_changed_at: health_info.last_updated,
            error_rate_trend: None, // Will be set by error rate switcher
            supervisor_weight: None,
            weight_update_reason: None,
            weight_updated_at: None,
            weight_version: None,
            utilization: None,
        }
    }

    /// Update health domain object from storage domain
    fn update_health_from_storage(
        health_info: &mut HealthProviderInfo,
        storage_info: &StorageHealthInfo,
    ) {
        health_info.health_score = storage_info.health_score;
        // Note: We don't update other fields as they belong to different domains
    }
}

/// Domain service for traffic control
pub struct TrafficService<'a> {
    config: &'a SupervisorConfig,
    redis_service: Arc<RedisService>,
    error_rate_switcher: ErrorRateSwitcher,
}

impl<'a> TrafficService<'a> {
    pub fn new(config: &'a SupervisorConfig, redis_service: Arc<RedisService>) -> Self {
        let error_rate_switcher =
            ErrorRateSwitcher::from_supervisor_config(Arc::clone(&redis_service), config);
        Self {
            config,
            redis_service,
            error_rate_switcher,
        }
    }

    /// Complete traffic management pipeline
    pub async fn manage_traffic(
        &mut self,
        health_infos: Vec<HealthProviderInfo>,
    ) -> Vec<StorageHealthInfo> {
        // 1. Apply error rate switching logic first
        let mut modified_health_infos = health_infos;
        let trend_map = if let Ok(trends) = self
            .apply_error_rate_switching(&mut modified_health_infos)
            .await
        {
            trends
        } else {
            debug!("Error rate switching failed");
            std::collections::HashMap::new()
        };

        // 2. Apply circuit breaker logic with trend information
        let circuit_breaker = CircuitBreaker::new(self.config, Arc::clone(&self.redis_service));
        circuit_breaker
            .apply_circuit_breaker_logic(modified_health_infos, trend_map)
            .await
    }

    /// Apply error rate switching to health domain objects
    async fn apply_error_rate_switching(
        &mut self,
        health_infos: &mut Vec<HealthProviderInfo>,
    ) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>>
    {
        // Convert to storage format for processing
        let mut storage_health_infos: Vec<StorageHealthInfo> = health_infos
            .iter()
            .map(HealthInfoConverter::health_to_storage)
            .collect();

        // Apply error rate switching using the new framework
        self.error_rate_switcher
            .process_error_rate_switching(&mut storage_health_infos)
            .await?;

        // Extract trend information mapping
        let mut trend_map = std::collections::HashMap::new();
        for storage_health in &storage_health_infos {
            if let Some(ref trend) = storage_health.error_rate_trend {
                trend_map.insert(storage_health.provider_id.clone(), trend.clone());
            }
        }

        // Update health domain objects with results
        for (health_info, storage_health) in
            health_infos.iter_mut().zip(storage_health_infos.iter())
        {
            HealthInfoConverter::update_health_from_storage(health_info, storage_health);
        }

        Ok(trend_map)
    }
}
