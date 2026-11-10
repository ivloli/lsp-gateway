use crate::monitoring::supervisor::{config::SupervisorConfig, health};
use crate::storage::{ProviderHealthInfo, RedisService};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Circuit breaker logic for provider health management
pub struct CircuitBreaker<'a> {
    config: &'a SupervisorConfig,
    redis_service: Arc<RedisService>,
}

impl<'a> CircuitBreaker<'a> {
    pub fn new(config: &'a SupervisorConfig, redis_service: Arc<RedisService>) -> Self {
        Self {
            config,
            redis_service,
        }
    }

    /// Apply circuit breaker logic to health information
    pub async fn apply_circuit_breaker_logic(
        &self,
        health_infos: Vec<health::ProviderHealthInfo>,
        trend_map: std::collections::HashMap<String, String>,
    ) -> Vec<ProviderHealthInfo> {
        let mut redis_health_infos = Vec::new();
        let now = crate::utils::TimeUtils::current_timestamp_secs();

        let open_threshold = self.config.unhealthy_threshold + self.config.health_buffer;
        let closed_threshold = self.config.unhealthy_threshold - self.config.health_buffer;

        for info in health_infos.into_iter() {
            let current_state = self
                .redis_service
                .get_provider_health(&info.provider_id)
                .await
                .unwrap_or(None);

            // 如果Circuit Breaker被禁用，始终返回closed状态
            let circuit_state = if self.config.circuit_breaker_enabled {
                self.determine_circuit_state_with_time(
                    &info,
                    &current_state,
                    now,
                    open_threshold,
                    closed_threshold,
                )
            } else {
                debug!(
                    "Circuit breaker disabled, keeping provider {} in closed state",
                    info.provider_id
                );
                "closed".to_string()
            };

            let state_changed_at = Self::get_state_changed_timestamp(
                &current_state,
                &circuit_state,
                now,
                &info.provider_id,
            );

            let availability = info.metrics.availability;

            // 使用传入的趋势信息，如果没有则从Redis获取现有的
            let error_rate_trend = trend_map.get(&info.provider_id).cloned().or_else(|| {
                current_state
                    .as_ref()
                    .and_then(|state| state.error_rate_trend.clone())
            });

            let redis_health_info = ProviderHealthInfo {
                provider_id: info.provider_id.clone(),
                health_score: info.health_score,
                circuit_state: circuit_state.clone(),
                // circuit_state: "closed".to_owned(),
                availability,
                last_updated: now,
                state_changed_at,
                error_rate_trend,
                supervisor_weight: None,
                weight_update_reason: None,
                weight_updated_at: None,
                weight_version: None,
                utilization: None,
            };

            // Update Prometheus metrics globally
            crate::monitoring::update_health_metrics_global(
                &info.provider_id,
                info.health_score,
                availability,
            );
            crate::monitoring::update_circuit_state_global(&info.provider_id, &circuit_state);

            redis_health_infos.push(redis_health_info);
        }

        redis_health_infos
    }

    /// Determine circuit state with time-based constraints
    fn determine_circuit_state_with_time(
        &self,
        info: &health::ProviderHealthInfo,
        current_state: &Option<ProviderHealthInfo>,
        now: u64,
        open_threshold: f64,
        closed_threshold: f64,
    ) -> String {
        let current_circuit_state = current_state
            .as_ref()
            .map(|s| s.circuit_state.as_str())
            .unwrap_or("closed");
        let time_since_state_change = current_state
            .as_ref()
            .map(|s| now.saturating_sub(s.state_changed_at))
            .unwrap_or(0);

        match current_circuit_state {
            "open" => self.handle_open_state(
                info,
                time_since_state_change,
                closed_threshold,
                open_threshold,
            ),
            "halfopen" => self.handle_halfopen_state(
                info,
                time_since_state_change,
                open_threshold,
                closed_threshold,
            ),
            "closed" | _ => self.handle_closed_state(info, open_threshold, closed_threshold),
        }
    }

    fn handle_open_state(
        &self,
        info: &health::ProviderHealthInfo,
        time_since_state_change: u64,
        closed_threshold: f64,
        open_threshold: f64,
    ) -> String {
        if info.health_score >= closed_threshold {
            debug!(
                "✅ Provider {} health score {:.3} ABOVE closed threshold {:.3} (healthy), transitioning OPEN->CLOSED",
                info.provider_id, info.health_score, closed_threshold
            );
            "closed".to_string()
        } else if time_since_state_change >= self.config.circuit_recovery_timeout_secs {
            info!(
                "🟡 Provider {} health score {:.3} improved after {}s recovery timeout, transitioning OPEN->HALFOPEN",
                info.provider_id, info.health_score, time_since_state_change
            );
            "halfopen".to_string()
        } else {
            if time_since_state_change < self.config.circuit_recovery_timeout_secs {
                debug!(
                    "⏳ Provider {} still in recovery timeout ({}s < {}s), staying OPEN",
                    info.provider_id,
                    time_since_state_change,
                    self.config.circuit_recovery_timeout_secs
                );
            } else {
                warn!(
                    "🚨 Provider {} health score {:.3} still BELOW open threshold {:.3}, staying OPEN",
                    info.provider_id, info.health_score, open_threshold
                );
            }
            "open".to_string()
        }
    }

    fn handle_halfopen_state(
        &self,
        info: &health::ProviderHealthInfo,
        time_since_state_change: u64,
        open_threshold: f64,
        closed_threshold: f64,
    ) -> String {
        if time_since_state_change < self.config.min_halfopen_duration_secs {
            debug!(
                "🟡 Provider {} staying HALFOPEN ({}s < {}s min duration)",
                info.provider_id, time_since_state_change, self.config.min_halfopen_duration_secs
            );
            "halfopen".to_string()
        } else if info.health_score < open_threshold {
            warn!(
                "🚨 Provider {} health score {:.3} degraded BELOW open threshold {:.3} (unhealthy), transitioning HALFOPEN->OPEN",
                info.provider_id, info.health_score, open_threshold
            );
            "open".to_string()
        } else if info.health_score >= closed_threshold {
            info!(
                "✅ Provider {} health score {:.3} improved ABOVE closed threshold {:.3} (healthy), transitioning HALFOPEN->CLOSED",
                info.provider_id, info.health_score, closed_threshold
            );
            "closed".to_string()
        } else {
            debug!(
                "🟡 Provider {} health score {:.3} in buffer zone, staying HALFOPEN",
                info.provider_id, info.health_score
            );
            "halfopen".to_string()
        }
    }

    fn handle_closed_state(
        &self,
        info: &health::ProviderHealthInfo,
        open_threshold: f64,
        closed_threshold: f64,
    ) -> String {
        if info.health_score < open_threshold {
            warn!(
                "🚨 Provider {} health score {:.3} BELOW open threshold {:.3} (unhealthy), transitioning CLOSED->OPEN",
                info.provider_id, info.health_score, open_threshold
            );
            "open".to_string()
        } else if info.health_score < closed_threshold {
            info!(
                "🟡 Provider {} health score {:.3} in buffer zone ({:.3}-{:.3}), transitioning CLOSED->HALFOPEN",
                info.provider_id, info.health_score, open_threshold, closed_threshold
            );
            "halfopen".to_string()
        } else {
            debug!(
                "✅ Provider {} health score {:.3} ABOVE closed threshold {:.3} (healthy), staying CLOSED",
                info.provider_id, info.health_score, closed_threshold
            );
            "closed".to_string()
        }
    }

    /// Get timestamp for circuit state changes
    fn get_state_changed_timestamp(
        current_state: &Option<ProviderHealthInfo>,
        new_circuit_state: &str,
        now: u64,
        provider_id: &str,
    ) -> u64 {
        let previous_state = current_state
            .as_ref()
            .map(|s| s.circuit_state.as_str())
            .unwrap_or("closed");

        if new_circuit_state != previous_state {
            info!(
                "🔄 Provider {} circuit state changed: {} → {}",
                provider_id, previous_state, new_circuit_state
            );
            now
        } else {
            current_state
                .as_ref()
                .map(|s| s.state_changed_at)
                .unwrap_or(now)
        }
    }
}
