use crate::error::{AppError, Result};
use redis::{aio::ConnectionManager, AsyncCommands, RedisResult};
use std::collections::HashMap;
use tracing::error;

/// Provider health information for Redis storage
#[derive(Debug, Clone)]
pub struct ProviderHealthInfo {
    pub provider_id: String,
    pub health_score: f64,
    pub circuit_state: String,
    pub availability: f64,
    pub last_updated: u64,
    pub state_changed_at: u64,
    pub error_rate_trend: Option<String>,
    // 🆕 权重相关字段
    pub supervisor_weight: Option<i32>, // Supervisor 计算的新权重
    pub weight_update_reason: Option<String>, // 权重调整原因
    pub weight_updated_at: Option<u64>, // 权重更新时间
    pub weight_version: Option<u64>,    // 权重版本号，防止冲突
    // 🆕 利用率字段
    pub utilization: Option<f64>, // Provider利用率 (current_rps / max_rps)
}

/// Provider health status (legacy)
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub provider_id: String,
    pub status: String,
    pub last_success: u64,
    pub consecutive_fails: u32,
    pub response_time: u64,
    pub error_rate: f64,
    pub updated_at: u64,
}

/// Service for handling provider health management in Redis
#[derive(Clone)]
pub struct StorageHealthService {
    connection_manager: ConnectionManager,
}

impl StorageHealthService {
    pub fn new(connection_manager: ConnectionManager) -> Self {
        Self { connection_manager }
    }

    /// Get provider health information
    pub async fn get_provider_health(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderHealthInfo>> {
        let key = format!("lsp:routing:provider:{}", provider_id);

        let mut conn = self.connection_manager.clone();

        let result: RedisResult<HashMap<String, String>> = conn.hgetall(&key).await;

        match result {
            Ok(data) => {
                if data.is_empty() {
                    return Ok(None);
                }

                let health_score = data
                    .get("health_score")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let circuit_state = data
                    .get("circuit_state")
                    .unwrap_or(&"closed".to_string())
                    .clone();

                let availability = data
                    .get("availability")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(1.0);

                let last_updated = data
                    .get("last_updated")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);

                let state_changed_at = data
                    .get("state_changed_at")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(last_updated);

                let error_rate_trend = data.get("error_rate_trend").cloned();

                // 🆕 解析权重相关字段
                let supervisor_weight = data
                    .get("supervisor_weight")
                    .and_then(|v| v.parse::<i32>().ok());

                let weight_update_reason = data.get("weight_update_reason").cloned();

                let weight_updated_at = data
                    .get("weight_updated_at")
                    .and_then(|v| v.parse::<u64>().ok());

                let weight_version = data
                    .get("weight_version")
                    .and_then(|v| v.parse::<u64>().ok());

                let utilization = data.get("utilization").and_then(|v| v.parse::<f64>().ok());

                Ok(Some(ProviderHealthInfo {
                    provider_id: provider_id.to_string(),
                    health_score,
                    circuit_state,
                    availability,
                    last_updated,
                    state_changed_at,
                    error_rate_trend,
                    supervisor_weight,
                    weight_update_reason,
                    weight_updated_at,
                    weight_version,
                    utilization,
                }))
            }
            Err(e) => {
                error!(
                    "Failed to get health info for provider {}: {}",
                    provider_id, e
                );
                Err(AppError::Redis(format!("Failed to get health info: {}", e)))
            }
        }
    }

    /// Batch store health information for multiple providers
    pub async fn batch_store_provider_health(
        &self,
        health_infos: &[ProviderHealthInfo],
    ) -> Result<()> {
        if health_infos.is_empty() {
            return Ok(());
        }

        let mut conn = self.connection_manager.clone();
        let mut pipe = redis::pipe();

        for health_info in health_infos {
            let key = format!("lsp:routing:provider:{}", health_info.provider_id);
            pipe.cmd("HSET")
                .arg(&key)
                .arg("health_score")
                .arg(health_info.health_score)
                .cmd("HSET")
                .arg(&key)
                .arg("circuit_state")
                .arg(&health_info.circuit_state)
                .cmd("HSET")
                .arg(&key)
                .arg("availability")
                .arg(health_info.availability)
                .cmd("HSET")
                .arg(&key)
                .arg("last_updated")
                .arg(health_info.last_updated)
                .cmd("HSET")
                .arg(&key)
                .arg("state_changed_at")
                .arg(health_info.state_changed_at);

            // 只有在有趋势信息时才存储
            if let Some(ref trend) = health_info.error_rate_trend {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("error_rate_trend")
                    .arg(trend);
            }

            // 🆕 权重更新字段（可选）
            if let Some(supervisor_weight) = health_info.supervisor_weight {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("supervisor_weight")
                    .arg(supervisor_weight);
            }

            if let Some(ref reason) = health_info.weight_update_reason {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("weight_update_reason")
                    .arg(reason);
            }

            if let Some(weight_updated_at) = health_info.weight_updated_at {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("weight_updated_at")
                    .arg(weight_updated_at);
            }

            if let Some(weight_version) = health_info.weight_version {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("weight_version")
                    .arg(weight_version);
            }

            if let Some(utilization) = health_info.utilization {
                pipe.cmd("HSET")
                    .arg(&key)
                    .arg("utilization")
                    .arg(utilization);
            }

            pipe.cmd("EXPIRE").arg(&key).arg(60u64);
        }

        let result: RedisResult<()> = pipe.atomic().query_async(&mut conn).await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("Failed to batch store health info: {}", e);
                Err(AppError::Redis(format!(
                    "Failed to batch store health info: {}",
                    e
                )))
            }
        }
    }
}
