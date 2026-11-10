use crate::models::ModelConfig;
use crate::routing::frp_route::FrpNode;
use moka::future::Cache;
use std::time::Duration;
use tracing::{debug, info};

#[derive(Clone)]
pub struct CacheService {
    model_config_cache: Cache<String, ModelConfig>,
    pub frp_node_cache: Cache<String, FrpNode>,
}

impl CacheService {
    pub fn new(max_capacity: u64, time_to_live_seconds: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(time_to_live_seconds))
            .build();

        let frpcache = Cache::builder().max_capacity(max_capacity).build();

        debug!(
            "Initialized cache with max_capacity={}, ttl={}s",
            max_capacity, time_to_live_seconds
        );

        Self {
            model_config_cache: cache,
            frp_node_cache: frpcache,
        }
    }

    pub async fn get_model_config(&self, id: &str) -> Option<ModelConfig> {
        let result = self.model_config_cache.get(id).await;
        if result.is_some() {
            debug!("Cache hit for model config: {}", id);
        } else {
            debug!("Cache miss for model config: {}", id);
        }
        result
    }

    pub async fn set_model_config(&self, id: String, config: ModelConfig) {
        debug!("Caching model config: {}", id);
        self.model_config_cache.insert(id, config).await;
    }

    pub async fn invalidate_model_config(&self, id: &str) {
        debug!("Invalidating cache for model config: {}", id);
        self.model_config_cache.invalidate(id).await;
    }

    pub async fn clear_all(&self) {
        info!("Clearing all cache entries");
        self.model_config_cache.invalidate_all();
    }

    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.model_config_cache.entry_count(),
            weighted_size: self.model_config_cache.weighted_size(),
        }
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub entry_count: u64,
    pub weighted_size: u64,
}
