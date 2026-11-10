use crate::error::{AppError, Result};
use crate::models::{ModelConfig, ModelProvider};
use crate::monitoring::record_provider_selection_global;
use crate::{
    config::RoutingConfig,
    monitoring::{HealthMonitor, MetricsReporter},
    routing::{frp_service::FrpManager, pipeline::ProviderPipeline},
    storage::{CacheService, RedisService},
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct SelectedProvider {
    pub id: String,
    pub base_url: String,
    pub provider: String,
    pub model_config: ModelConfig,
    pub override_model_name: String,
    pub selected_model_provider: Option<ModelProvider>,
}

impl SelectedProvider {
    /// Check if the selected_model_provider has a frpmodel configured
    pub fn has_frpmodel(&self) -> bool {
        self.selected_model_provider
            .as_ref()
            .and_then(|mp| mp.frpmodel.as_ref())
            .is_some()
    }
}

pub struct ProviderService {
    pool: sqlx::PgPool,
    cache: Arc<CacheService>,
    metrics_reporter: Option<Arc<MetricsReporter>>,
    health_monitor: Option<Arc<HealthMonitor>>,
    redis_service: Option<Arc<RedisService>>,
    routing_config: Option<RoutingConfig>,
    pub frp_manager: Option<Arc<FrpManager>>,
}

impl ProviderService {
    pub fn new(pool: sqlx::PgPool, cache: Arc<CacheService>) -> Self {
        Self {
            pool,
            cache,
            metrics_reporter: None,
            health_monitor: None,
            redis_service: None,
            routing_config: None,
            frp_manager: None,
        }
    }

    /// Get the cache service
    pub fn get_cache(&self) -> &Arc<CacheService> {
        &self.cache
    }

    /// Set the metrics reporter for Redis metrics
    pub fn with_metrics_reporter(mut self, metrics_reporter: Arc<MetricsReporter>) -> Self {
        self.metrics_reporter = Some(metrics_reporter);
        self
    }

    /// Set the health monitor for provider health tracking
    pub fn with_health_monitor(mut self, health_monitor: Arc<HealthMonitor>) -> Self {
        self.health_monitor = Some(health_monitor);
        self
    }

    /// Set the Redis service for dynamic weight retrieval
    pub fn with_redis_service(mut self, redis_service: Arc<RedisService>) -> Self {
        self.redis_service = Some(redis_service);
        self
    }

    /// Set the routing configuration
    pub fn with_routing_config(mut self, routing_config: RoutingConfig) -> Self {
        self.routing_config = Some(routing_config);
        self
    }

    pub fn with_frp_manager(mut self, frp_manager: Arc<FrpManager>) -> Self {
        self.frp_manager = Some(frp_manager);
        self
    }

    pub fn is_frpmgr_none(&self) -> bool {
        self.frp_manager.is_none()
    }

    /// Get database pool reference for weight management APIs
    pub fn get_db_pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// Get Redis service reference for weight management APIs
    pub fn get_redis_service(&self) -> Option<&Arc<RedisService>> {
        self.redis_service.as_ref()
    }

    /// Get model configuration by ID with caching
    pub async fn get_model_config_by_id(&self, id: &str) -> Result<ModelConfig> {
        // Check cache first
        if let Some(cached_config) = self.cache.get_model_config(id).await {
            debug!("Found model config in cache: {}", id);
            return Ok(cached_config);
        }

        // Query database
        let config = ModelConfig::find_by_id(&self.pool, id)
            .await
            .map_err(|db_error| {
                error!(
                    "❌ DB_ERROR: Failed to query model config by ID '{}': {}",
                    id, db_error
                );
                AppError::Database(db_error)
            })?
            .ok_or_else(|| AppError::ModelNotFound(id.to_string()))?;

        // Cache the result
        self.cache
            .set_model_config(id.to_string(), config.clone())
            .await;

        debug!("Loaded and cached model config: {}", id);
        Ok(config)
    }

    pub async fn get_provider_by_group(&self, group: &str) -> Result<Option<ModelProvider>> {
        let providers = ModelProvider::find_active_by_group(&self.pool, group)
            .await
            .map_err(|db_error| {
                error!(
                    "❌ DB_ERROR: Failed to query active providers for group '{}': {}",
                    group, db_error
                );
                AppError::Database(db_error)
            })?;

        if providers.is_empty() {
            warn!(
                "⚠️  PROVIDER_SELECTION: No active providers found for group: {}",
                group
            );
            return Ok(None);
        }

        // 如果组内只有一个provider，直接返回，跳过所有过滤和切换逻辑
        if providers.len() == 1 {
            let single_provider = providers.into_iter().next().ok_or_else(|| {
                AppError::Internal("Expected single provider but found none".to_string())
            })?;
            debug!(
                "🎯 PROVIDER_SELECTION: Group '{}': single provider {} (tier={},weight={})",
                group, single_provider.id, single_provider.tier, single_provider.weight
            );
            return Ok(Some(single_provider));
        }

        let provider_summary: Vec<String> = providers
            .iter()
            .map(|p| format!("{}(t{}:w{})", p.id, p.tier, p.weight))
            .collect();
        debug!(
            "PROVIDER_SELECTION: Group '{}' candidates: [{}]",
            group,
            provider_summary.join(", ")
        );

        let routing_config = self
            .routing_config
            .as_ref()
            .ok_or_else(|| AppError::Internal("Routing config not initialized".to_string()))?;

        // Get surge percentage from Growthbook (with fallback to 0.3)
        let surge_percentage = crate::client::growthbook::get_max_surge_percentage().await;

        let pipeline = ProviderPipeline::new(
            self.redis_service.clone(),
            routing_config,
            !self.is_frpmgr_none(),
        )
        .with_surge_percentage(surge_percentage);

        let result = pipeline.select_provider(providers, group).await?;
        if result.is_none() {
            error!(
                "❌ PROVIDER_SELECTION: Pipeline returned no providers for group: {}",
                group
            );
            return Err(AppError::InvalidRequest(format!(
                "No providers available after pipeline filtering for group: {}",
                group
            )));
        }

        Ok(result)
    }

    /// Main provider selection logic that mirrors the TypeScript implementation
    #[tracing::instrument(
        name = "provider_selection",
        skip(self, conv_id),
        fields(model_id = %model_id)
    )]
    pub async fn select_provider(
        &self,
        model_id: &str,
        conv_id: Option<&str>,
    ) -> Result<SelectedProvider> {
        // Get base model config
        let base_model_config = self.get_model_config_by_id(model_id).await?;

        if !base_model_config.live {
            return Err(AppError::ModelNotFound(format!(
                "Model {} is not live",
                model_id
            )));
        }

        let mut base_url = base_model_config.base_url.clone();
        let mut provider = base_model_config.provider.clone();
        let mut provider_id = None;
        let mut override_model_name = String::new();
        let mut selected_model_provider: Option<ModelProvider> = None;

        // Check if model has a provider group
        if let Some(model_provider_group) = &base_model_config.model_provider_group {
            debug!(
                "Model {} has provider group: {}",
                model_id, model_provider_group
            );

            match self.get_provider_by_group(model_provider_group).await? {
                Some(mut selected_provider) => {
                    // info!(
                    //     "Selected provider {} for model {} from group {}",
                    //     selected_provider.id, model_id, model_provider_group
                    // );
                    if selected_provider.has_frpmodel() {
                        debug!("🔌 FRP: Provider {} uses frpmodel", selected_provider.id);
                        if let Some(frp_selected_provider) = self
                            .frp_manager
                            .as_ref()
                            .unwrap()
                            .select_provider_node(&selected_provider, conv_id)
                            .await
                        {
                            selected_provider = frp_selected_provider;
                            info!("✅ FRP: Node selected: {}", selected_provider.base_url)
                        } else {
                            selected_provider.base_url = base_url;
                            warn!("⚠️  FRP: Node selection failed for provider {}, using config base_url", selected_provider.id);
                        }
                    }

                    base_url = selected_provider.base_url.clone();
                    provider = selected_provider.provider.clone();
                    provider_id = Some(selected_provider.id.clone());
                    if let Some(override_name) = &selected_provider.override_model_name {
                        if !override_name.is_empty() {
                            override_model_name = override_name.clone();
                        }
                    }
                    // Store the selected ModelProvider for API key resolution
                    selected_model_provider = Some(selected_provider);
                }
                None => {
                    warn!("⚠️  PROVIDER_SELECTION: No active providers for group: {}, using ModelConfig fallback", model_provider_group);
                }
            }
        } else {
            debug!("Model {} uses direct provider configuration", model_id);
        }

        // If base_url is empty, try to get default base URL for the provider
        if base_url.is_empty() {
            if let Some(default_url) = crate::client::get_provider_default_base_url(&provider) {
                base_url = default_url;
                debug!(
                    "Using default base URL for provider {}: {}",
                    provider, base_url
                );
            } else {
                return Err(AppError::InvalidRequest(format!(
                    "No valid base URL found for model {} and provider {}",
                    model_id, provider
                )));
            }
        }
        if override_model_name.is_empty() {
            override_model_name = base_model_config.model_name.clone();
        }

        // Determine final provider_id
        let final_provider_id = provider_id.unwrap_or_else(|| format!("direct-{}", provider));

        // 📊 METRIC: Record provider selection for Direct path
        // Use model_provider_group if available, otherwise use "direct-{model_config.id}"
        if selected_model_provider.is_none() {
            let group_for_prometheus = format!("direct-{}", base_model_config.id);
            record_provider_selection_global(&final_provider_id, 0, &group_for_prometheus);
        }

        Ok(SelectedProvider {
            id: final_provider_id,
            base_url,
            provider,
            model_config: base_model_config,
            override_model_name: String::from(override_model_name),
            selected_model_provider,
        })
    }

    /// Clear all caches
    pub async fn clear_cache(&self) {
        self.cache.clear_all().await;
    }

    /// Record request completion for metrics reporting and health monitoring
    pub async fn record_request_completion(
        &self,
        provider_id: &str,
        success: bool,
        latency_ms: u64,
        model_provider_group: Option<String>,
    ) {
        if let Some(metrics_reporter) = &self.metrics_reporter {
            metrics_reporter
                .record_request_completion(
                    provider_id,
                    success,
                    latency_ms,
                    model_provider_group.clone(),
                )
                .await;
        }

        if let Some(health_monitor) = &self.health_monitor {
            health_monitor
                .record_request_completion(provider_id, success, latency_ms)
                .await;
        }
    }

    /// Record request completion with token usage for metrics reporting and health monitoring
    pub async fn record_request_completion_with_tokens(
        &self,
        provider_id: &str,
        success: bool,
        latency_ms: u64,
        tokens: u64,
        model_provider_group: Option<String>,
    ) {
        if let Some(metrics_reporter) = &self.metrics_reporter {
            metrics_reporter
                .record_request_completion_with_tokens(
                    provider_id,
                    success,
                    latency_ms,
                    tokens,
                    model_provider_group.clone(),
                )
                .await;
        }

        if let Some(health_monitor) = &self.health_monitor {
            health_monitor
                .record_request_completion(provider_id, success, latency_ms)
                .await;
        }
    }
}
