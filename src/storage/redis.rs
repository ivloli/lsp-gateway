use crate::config::RedisConfig;
use crate::error::{AppError, Result};
use redis::{aio::ConnectionManager, Client};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use super::{
    health_service::{ProviderHealthInfo, StorageHealthService},
    lock_service::LockService,
    metrics_service::{GatewayMetrics, ProviderMetrics, StorageMetricsService},
};

/// Unified Redis service that delegates to specialized services
#[derive(Clone)]
pub struct RedisService {
    pub connection_manager: ConnectionManager,
    pub metrics_service: Arc<StorageMetricsService>,
    pub health_service: Arc<StorageHealthService>,
    pub lock_service: Arc<LockService>,
}

impl RedisService {
    /// Create a new Redis service with connection manager and specialized services
    pub async fn new(config: &RedisConfig) -> Result<Self> {
        info!("Connecting to Redis at: {}", config.url);

        let client = Client::open(config.url.as_str())
            .map_err(|e| AppError::Redis(format!("Failed to create Redis client: {}", e)))?;

        let connection_manager = ConnectionManager::new(client)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to create connection manager: {}", e)))?;

        info!("Successfully connected to Redis");

        // Create specialized services
        let metrics_service = Arc::new(StorageMetricsService::new(connection_manager.clone()));
        let health_service = Arc::new(StorageHealthService::new(connection_manager.clone()));
        let lock_service = Arc::new(LockService::new(connection_manager.clone()));

        Ok(Self {
            connection_manager,
            metrics_service,
            health_service,
            lock_service,
        })
    }

    // ========== Delegate Methods for Backward Compatibility ==========

    /// Gateway metrics method with gateway instance ID in key
    pub async fn report_provider_metrics(&self, metrics: &GatewayMetrics) -> Result<()> {
        self.metrics_service.report_provider_metrics(metrics).await
    }

    /// Get all provider metrics grouped by provider_id
    /// Returns Map<provider_id, Vec<ProviderMetrics>>
    pub async fn get_all_provider_metrics(&self) -> Result<HashMap<String, Vec<ProviderMetrics>>> {
        self.metrics_service.get_all_provider_metrics().await
    }

    /// Get all provider metrics from gateway|provider combinations (using latest from time series)
    pub async fn get_all_provider_latest_metrics(&self) -> Result<Vec<ProviderMetrics>> {
        self.metrics_service.get_all_provider_latest_metrics().await
    }

    /// Get provider health information
    pub async fn get_provider_health(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderHealthInfo>> {
        self.health_service.get_provider_health(provider_id).await
    }

    /// Batch store health information for multiple providers
    pub async fn batch_store_provider_health(
        &self,
        health_infos: &[ProviderHealthInfo],
    ) -> Result<()> {
        self.health_service
            .batch_store_provider_health(health_infos)
            .await
    }

    /// Acquire distributed lock for supervisor functionality
    pub async fn acquire_supervisor_lock(&self, instance_id: &str, ttl_secs: u64) -> Result<bool> {
        self.lock_service
            .acquire_supervisor_lock(instance_id, ttl_secs)
            .await
    }

    /// Renew supervisor lock (extend TTL)
    pub async fn renew_supervisor_lock(&self, instance_id: &str, ttl_secs: u64) -> Result<bool> {
        self.lock_service
            .renew_supervisor_lock(instance_id, ttl_secs)
            .await
    }

    /// Release supervisor lock
    pub async fn release_supervisor_lock(&self, instance_id: &str) -> Result<bool> {
        self.lock_service.release_supervisor_lock(instance_id).await
    }

    /// Check current supervisor lock holder
    pub async fn get_supervisor_lock_holder(&self) -> Result<Option<String>> {
        self.lock_service.get_supervisor_lock_holder().await
    }

    /// Get string value from Redis
    #[tracing::instrument(skip(self), fields(operation = "redis.get"))]
    pub async fn get_string(&self, key: &str) -> Result<String> {
        let mut conn = self.connection_manager.clone();
        let result: redis::RedisResult<String> =
            redis::cmd("GET").arg(key).query_async(&mut conn).await;

        match result {
            Ok(value) => Ok(value),
            Err(e) => Err(AppError::Redis(format!(
                "Failed to get string for key {}: {}",
                key, e
            ))),
        }
    }

    /// Set string value in Redis with optional expiration
    #[tracing::instrument(skip(self, value), fields(operation = "redis.set", ttl = ?expire_secs))]
    pub async fn set_string(&self, key: &str, value: &str, expire_secs: Option<u64>) -> Result<()> {
        let mut conn = self.connection_manager.clone();

        let mut cmd = redis::cmd("SET");
        cmd.arg(key).arg(value);

        if let Some(ttl) = expire_secs {
            cmd.arg("EX").arg(ttl);
        }

        let result: redis::RedisResult<()> = cmd.query_async(&mut conn).await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(AppError::Redis(format!(
                "Failed to set string for key {}: {}",
                key, e
            ))),
        }
    }

    /// Health check for Redis connection
    pub async fn health_check(&self) -> Result<()> {
        let mut conn = self.connection_manager.clone();

        let result: redis::RedisResult<String> = redis::cmd("PING").query_async(&mut conn).await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(AppError::Redis(format!("Redis health check failed: {}", e))),
        }
    }

    /// Generic get operation for string values
    #[tracing::instrument(skip(self), fields(operation = "redis.get"))]
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        use redis::AsyncCommands;
        let mut conn = self.connection_manager.clone();
        let result: Option<String> = conn
            .get(key)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to get key {}: {}", key, e)))?;
        Ok(result)
    }

    /// Generic set operation for string values with optional TTL
    #[tracing::instrument(skip(self, value), fields(operation = "redis.set", ttl = ?ttl_secs))]
    pub async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<()> {
        use redis::AsyncCommands;
        let mut conn = self.connection_manager.clone();

        if let Some(ttl) = ttl_secs {
            conn.set_ex::<_, _, ()>(key, value, ttl).await
        } else {
            conn.set::<_, _, ()>(key, value).await
        }
        .map_err(|e| AppError::Redis(format!("Failed to set key {}: {}", key, e)))?;

        Ok(())
    }

    /// Generic hdel operation for hash fields
    pub async fn hdel(&self, key: &str, field: &str) -> Result<()> {
        use redis::AsyncCommands;
        let mut conn = self.connection_manager.clone();
        let _: i32 = conn.hdel(key, field).await.map_err(|e| {
            AppError::Redis(format!("Failed to hdel key {} field {}: {}", key, field, e))
        })?;
        Ok(())
    }

    /// Generic hget operation for hash fields
    #[tracing::instrument(skip(self), fields(operation = "redis.hget"))]
    pub async fn hget(&self, key: &str, field: &str) -> Result<Option<String>> {
        use redis::AsyncCommands;
        let mut conn = self.connection_manager.clone();
        let result: Option<String> = conn.hget(key, field).await.map_err(|e| {
            AppError::Redis(format!("Failed to hget key {} field {}: {}", key, field, e))
        })?;
        Ok(result)
    }

    /// Generic hset operation for hash fields
    #[tracing::instrument(skip(self, value), fields(operation = "redis.hset"))]
    pub async fn hset(&self, key: &str, field: &str, value: &str) -> Result<()> {
        use redis::AsyncCommands;
        let mut conn = self.connection_manager.clone();
        let _: i32 = conn.hset(key, field, value).await.map_err(|e| {
            AppError::Redis(format!(
                "Failed to hset key {} field {} value {}: {}",
                key, field, value, e
            ))
        })?;
        Ok(())
    }
}
