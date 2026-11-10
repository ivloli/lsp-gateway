use crate::error::{AppError, Result};
use redis::{aio::ConnectionManager, AsyncCommands, RedisResult};
use std::collections::HashMap;
use tracing::{debug, error, info};

/// Gateway instance metrics data structure for Redis reporting
#[derive(Debug, Clone)]
pub struct GatewayMetrics {
    pub gateway_id: String,
    pub provider_id: String,
    pub model_provider_group: Option<String>,
    pub success_count: u64,
    pub fail_count: u64,
    pub latency_p95_ms: u64,
    pub token_count: u64,
    pub token_throughput_tps: f64,
    pub timestamp: u64,
}

/// Provider metrics data structure for Redis reporting (legacy support)
#[derive(Debug, Clone)]
pub struct ProviderMetrics {
    pub provider_id: String,
    pub model_provider_group: Option<String>,
    pub success_count: u64,
    pub fail_count: u64,
    pub latency_p95_ms: u64,
    pub token_count: u64,
    pub token_throughput_tps: f64,
    pub timestamp: u64,
}

/// Service for handling metrics reporting to Redis
#[derive(Clone)]
pub struct StorageMetricsService {
    connection_manager: ConnectionManager,
}

impl StorageMetricsService {
    pub fn new(connection_manager: ConnectionManager) -> Self {
        Self { connection_manager }
    }

    /// Report provider metrics for a specific gateway instance
    pub async fn report_provider_metrics(&self, metrics: &GatewayMetrics) -> Result<()> {
        let key = format!(
            "lsp:metrics:gateway:{}:provider:{}",
            metrics.gateway_id, metrics.provider_id
        );
        let gateway_provider_combo = format!("{}|{}", metrics.gateway_id, metrics.provider_id);

        // Create JSON value for sorted set
        let json_value = serde_json::json!({
            "success_count": metrics.success_count,
            "fail_count": metrics.fail_count,
            "latency_p95_ms": metrics.latency_p95_ms,
            "token_count": metrics.token_count,
            "token_throughput_tps": metrics.token_throughput_tps,
            "gateway_id": metrics.gateway_id,
            "model_provider_group": metrics.model_provider_group,
            "timestamp": metrics.timestamp,
        })
        .to_string();

        let mut conn = self.connection_manager.clone();

        // Use timestamp in milliseconds as score for sorted set
        let score = metrics.timestamp * 1000;
        let cutoff_time = score - 60000; // 1 minute ago

        let result: RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("ZADD")
            .arg(&key)
            .arg(score)
            .arg(&json_value)
            .cmd("ZREMRANGEBYSCORE")
            .arg(&key)
            .arg(0)
            .arg(cutoff_time)
            .cmd("EXPIRE")
            .arg(&key)
            .arg(600u64) // 10 minutes TTL for safety
            .cmd("SADD")
            .arg("lsp:metrics:provider")
            .arg(&gateway_provider_combo)
            .cmd("EXPIRE")
            .arg("lsp:metrics:provider")
            .arg(300u64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                error!(
                    "Failed to report gateway metrics for gateway {} provider {}: {}",
                    metrics.gateway_id, metrics.provider_id, e
                );
                Err(AppError::Redis(format!(
                    "Failed to report gateway metrics: {}",
                    e
                )))
            }
        }
    }

    /// Get all provider metrics from gateway|provider combinations (using latest from time series)
    pub async fn get_all_provider_latest_metrics(&self) -> Result<Vec<ProviderMetrics>> {
        let mut conn = self.connection_manager.clone();

        let gateway_provider_combos: Vec<String> =
            conn.smembers("lsp:metrics:provider").await.map_err(|e| {
                AppError::Redis(format!(
                    "Failed to get gateway|provider combinations: {}",
                    e
                ))
            })?;

        if gateway_provider_combos.is_empty() {
            return Ok(Vec::new());
        }

        let mut metrics = Vec::new();
        let mut invalid_combos = Vec::new();

        for combo in gateway_provider_combos {
            if let Some((gateway_id, provider_id)) = self.parse_gateway_provider_combo(&combo) {
                // 获取最新的一个时间序列数据点
                if let Ok(time_series) = self
                    .get_gateway_provider_metrics_time_series(&gateway_id, &provider_id, 1)
                    .await
                {
                    if let Some(latest_metric) = time_series.into_iter().last() {
                        metrics.push(latest_metric);
                    } else {
                        // 时间序列为空，标记为无效
                        invalid_combos.push(combo);
                    }
                } else {
                    // 获取失败，标记为无效
                    invalid_combos.push(combo);
                }
            }
        }

        // 清理无效的组合
        if !invalid_combos.is_empty() {
            self.cleanup_invalid_provider_combos(&invalid_combos)
                .await?;
        }

        info!("Retrieved latest metrics for {} gateway|provider combinations, cleaned {} invalid combos", 
              metrics.len(), invalid_combos.len());
        Ok(metrics)
    }

    /// Clean up invalid gateway|provider combinations from lsp:metrics:provider set
    async fn cleanup_invalid_provider_combos(&self, invalid_combos: &[String]) -> Result<()> {
        if invalid_combos.is_empty() {
            return Ok(());
        }

        let mut conn = self.connection_manager.clone();

        // Remove invalid combinations from the set
        let result: redis::RedisResult<i64> = redis::cmd("SREM")
            .arg("lsp:metrics:provider")
            .arg(invalid_combos)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(removed_count) => {
                info!(
                    "Cleaned up {} invalid gateway|provider combinations from lsp:metrics:provider",
                    removed_count
                );
                Ok(())
            }
            Err(e) => {
                error!("Failed to cleanup invalid combinations: {}", e);
                Err(AppError::Redis(format!(
                    "Failed to cleanup invalid combinations: {}",
                    e
                )))
            }
        }
    }

    /// Get all provider metrics grouped by provider_id
    /// Returns Map<provider_id, Vec<ProviderMetrics>>
    pub async fn get_all_provider_metrics(&self) -> Result<HashMap<String, Vec<ProviderMetrics>>> {
        let mut conn = self.connection_manager.clone();

        let gateway_provider_combos: Vec<String> =
            conn.smembers("lsp:metrics:provider").await.map_err(|e| {
                AppError::Redis(format!(
                    "Failed to get gateway|provider combinations: {}",
                    e
                ))
            })?;

        if gateway_provider_combos.is_empty() {
            return Ok(HashMap::new());
        }

        let mut provider_metrics_map: HashMap<String, Vec<ProviderMetrics>> = HashMap::new();
        let mut invalid_combos = Vec::new();
        let total_combos = gateway_provider_combos.len();

        for combo in gateway_provider_combos {
            if let Some((gateway_id, provider_id)) = self.parse_gateway_provider_combo(&combo) {
                let key = format!(
                    "lsp:metrics:gateway:{}:provider:{}",
                    gateway_id, provider_id
                );

                // Get all entries from sorted set (ordered by timestamp, newest first)
                match conn.zrevrange(&key, 0, -1).await {
                    Ok(entries) => {
                        let entries: Vec<String> = entries;
                        if entries.is_empty() {
                            // 时间序列为空，标记为无效
                            invalid_combos.push(combo);
                            continue;
                        }

                        // Process all entries, not just the first one
                        for json_str in entries {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_str)
                            {
                                let success_count = parsed["success_count"].as_u64().unwrap_or(0);
                                let fail_count = parsed["fail_count"].as_u64().unwrap_or(0);
                                let latency_p95_ms = parsed["latency_p95_ms"].as_u64().unwrap_or(0);
                                let token_count = parsed["token_count"].as_u64().unwrap_or(0);
                                let token_throughput_tps =
                                    parsed["token_throughput_tps"].as_f64().unwrap_or(0.0);
                                let timestamp = parsed["timestamp"].as_u64().unwrap_or(0);
                                let model_provider_group = parsed["model_provider_group"]
                                    .as_str()
                                    .map(|s| s.to_string());

                                let provider_metric = ProviderMetrics {
                                    provider_id: provider_id.clone(),
                                    model_provider_group,
                                    success_count,
                                    fail_count,
                                    latency_p95_ms,
                                    token_count,
                                    token_throughput_tps,
                                    timestamp,
                                };

                                // Group by provider_id
                                provider_metrics_map
                                    .entry(provider_id.clone())
                                    .or_insert_with(Vec::new)
                                    .push(provider_metric);
                            }
                        }
                    }
                    Err(_) => {
                        // 获取失败，标记为无效
                        invalid_combos.push(combo);
                    }
                }
            }
        }

        // 清理无效的组合
        if !invalid_combos.is_empty() {
            self.cleanup_invalid_provider_combos(&invalid_combos)
                .await?;
        }

        info!("Retrieved metrics for {} providers from {} gateway|provider combinations, cleaned {} invalid combos", 
              provider_metrics_map.len(), total_combos, invalid_combos.len());
        Ok(provider_metrics_map)
    }

    pub async fn get_gateway_provider_metrics_time_series(
        &self,
        gateway_id: &str,
        provider_id: &str,
        count: usize,
    ) -> Result<Vec<ProviderMetrics>> {
        let mut conn = self.connection_manager.clone();
        let key = format!(
            "lsp:metrics:gateway:{}:provider:{}",
            gateway_id, provider_id
        );

        // Get the latest N entries from sorted set (ordered by timestamp)
        let entries: Vec<String> = conn
            .zrevrange(&key, 0, (count as isize) - 1)
            .await
            .map_err(|e| {
                AppError::Redis(format!(
                    "Failed to get gateway provider metrics time series: {}",
                    e
                ))
            })?;

        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let mut metrics = Vec::new();

        for json_str in entries.iter().rev() {
            // Reverse to get oldest first (chronological order)
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                let success_count = parsed["success_count"].as_u64().unwrap_or(0);
                let fail_count = parsed["fail_count"].as_u64().unwrap_or(0);
                let latency_p95_ms = parsed["latency_p95_ms"].as_u64().unwrap_or(0);
                let token_count = parsed["token_count"].as_u64().unwrap_or(0);
                let token_throughput_tps = parsed["token_throughput_tps"].as_f64().unwrap_or(0.0);
                let timestamp = parsed["timestamp"].as_u64().unwrap_or(0);
                let model_provider_group = parsed["model_provider_group"]
                    .as_str()
                    .map(|s| s.to_string());

                metrics.push(ProviderMetrics {
                    provider_id: provider_id.to_string(),
                    model_provider_group,
                    success_count,
                    fail_count,
                    latency_p95_ms,
                    token_count,
                    token_throughput_tps,
                    timestamp,
                });
            }
        }

        debug!(
            "Retrieved {} time series metrics for {}:{}",
            metrics.len(),
            gateway_id,
            provider_id
        );
        Ok(metrics)
    }

    /// Parse gateway|provider combination string
    fn parse_gateway_provider_combo(&self, combo: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = combo.split('|').collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }
}
