use crate::config::env_utils;
use serde::{Deserialize, Serialize};

/// Supervisor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
    /// Control loop interval in seconds
    pub control_interval_secs: u64,

    /// Metrics aggregation interval in seconds
    pub aggregation_interval_secs: u64,

    /// Gateway timeout for considering a gateway inactive (seconds)
    pub gateway_timeout_secs: u64,

    /// Capacity protection ratio (e.g., 0.8 for 80% capacity limit)
    pub capacity_protection_ratio: f64,

    /// Maximum adjustment ratio per cycle (e.g., 0.2 for 20% max change)
    pub max_adjustment_ratio: f64,

    /// Enable cross-gateway load balancing
    pub cross_gateway_balancing: bool,

    /// Batch size for Redis operations
    pub batch_size: usize,

    /// Error backoff time in seconds
    pub error_backoff_secs: u64,

    /// Maximum number of Redis connections
    pub redis_max_connections: u32,

    /// Redis operation timeout in seconds
    pub redis_timeout_secs: u64,

    /// Maximum number of database connections
    pub db_max_connections: u32,

    /// Health score threshold for circuit breaker (below this = unhealthy)
    pub unhealthy_threshold: f64,

    /// Health score buffer for halfopen state (±buffer around threshold)
    pub health_buffer: f64,

    /// Circuit breaker recovery timeout in seconds (time to wait before OPEN->HALFOPEN)
    pub circuit_recovery_timeout_secs: u64,

    /// Minimum halfopen duration in seconds (avoid rapid state changes)
    pub min_halfopen_duration_secs: u64,

    /// Whether supervisor is enabled
    pub enabled: bool,

    /// Whether circuit breaker is enabled
    pub circuit_breaker_enabled: bool,

    /// Distributed lock configuration
    /// Interval for attempting to acquire supervisor lock (seconds)
    pub lock_attempt_interval_secs: u64,

    /// Interval for renewing supervisor lock (seconds)
    pub lock_renewal_interval_secs: u64,

    /// TTL for supervisor lock (seconds)
    pub lock_ttl_secs: u64,

    /// Gradual traffic adjustment configuration
    /// Traffic increase percentage per adjustment cycle when latency is acceptable
    pub traffic_increase_rate: f64,

    /// Traffic decrease percentage per adjustment cycle when latency is unacceptable  
    pub traffic_decrease_rate: f64,

    /// Recovery time window in seconds for gradual recovery
    pub recovery_window_secs: u64,

    /// Minimum time between weight adjustments (seconds)
    pub min_adjustment_interval_secs: u64,

    /// Error rate threshold for triggering traffic reduction (0.0-1.0)
    pub error_rate_threshold: f64,

    /// Error rate recovery threshold for traffic increase (0.0-1.0)
    pub error_rate_recovery_threshold: f64,

    /// Relative change threshold for error rate trend detection (e.g., 0.5 = 50%)
    pub error_rate_rel_threshold: f64,

    /// Absolute change threshold for error rate trend detection (e.g., 0.02 = 2%)
    pub error_rate_abs_threshold: f64,
}

impl SupervisorConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            control_interval_secs: env_utils::env_or_default("SUPERVISOR_CONTROL_INTERVAL_SECS", 1),
            aggregation_interval_secs: env_utils::env_or_default(
                "SUPERVISOR_AGGREGATION_INTERVAL_SECS",
                3,
            ),
            gateway_timeout_secs: env_utils::env_or_default("SUPERVISOR_GATEWAY_TIMEOUT_SECS", 60),
            capacity_protection_ratio: env_utils::env_or_default(
                "SUPERVISOR_CAPACITY_PROTECTION_RATIO",
                0.8,
            ),
            max_adjustment_ratio: env_utils::env_or_default("SUPERVISOR_MAX_ADJUSTMENT_RATIO", 0.5),
            cross_gateway_balancing: env_utils::env_or_default(
                "SUPERVISOR_CROSS_GATEWAY_BALANCING",
                true,
            ),
            batch_size: env_utils::env_or_default("SUPERVISOR_BATCH_SIZE", 100),
            error_backoff_secs: env_utils::env_or_default("SUPERVISOR_ERROR_BACKOFF_SECS", 5),
            redis_max_connections: env_utils::env_or_default(
                "SUPERVISOR_REDIS_MAX_CONNECTIONS",
                10,
            ),
            redis_timeout_secs: env_utils::env_or_default("SUPERVISOR_REDIS_TIMEOUT_SECS", 5),
            db_max_connections: env_utils::env_or_default("SUPERVISOR_DB_MAX_CONNECTIONS", 10),
            unhealthy_threshold: env_utils::env_or_default("SUPERVISOR_UNHEALTHY_THRESHOLD", 0.2),
            health_buffer: env_utils::env_or_default("SUPERVISOR_HEALTH_BUFFER", 0.1),
            circuit_recovery_timeout_secs: env_utils::env_or_default(
                "SUPERVISOR_CIRCUIT_RECOVERY_TIMEOUT_SECS",
                30,
            ),
            min_halfopen_duration_secs: env_utils::env_or_default(
                "SUPERVISOR_MIN_HALFOPEN_DURATION_SECS",
                10,
            ),
            enabled: env_utils::env_or_default("SUPERVISOR_ENABLED", true),
            circuit_breaker_enabled: env_utils::env_or_default("CIRCUIT_BREAKER_ENABLED", true),
            lock_attempt_interval_secs: env_utils::env_or_default(
                "SUPERVISOR_LOCK_ATTEMPT_INTERVAL_SECS",
                15,
            ),
            lock_renewal_interval_secs: env_utils::env_or_default(
                "SUPERVISOR_LOCK_RENEWAL_INTERVAL_SECS",
                10,
            ),
            lock_ttl_secs: env_utils::env_or_default("SUPERVISOR_LOCK_TTL_SECS", 30),
            traffic_increase_rate: env_utils::env_or_default(
                "SUPERVISOR_TRAFFIC_INCREASE_RATE",
                0.1,
            ),
            traffic_decrease_rate: env_utils::env_or_default(
                "SUPERVISOR_TRAFFIC_DECREASE_RATE",
                0.2,
            ),
            recovery_window_secs: env_utils::env_or_default("SUPERVISOR_RECOVERY_WINDOW_SECS", 600),
            min_adjustment_interval_secs: env_utils::env_or_default(
                "SUPERVISOR_MIN_ADJUSTMENT_INTERVAL_SECS",
                1,
            ),
            error_rate_threshold: env_utils::env_or_default(
                "SUPERVISOR_ERROR_RATE_THRESHOLD",
                0.05,
            ),
            error_rate_recovery_threshold: env_utils::env_or_default(
                "SUPERVISOR_ERROR_RATE_RECOVERY_THRESHOLD",
                0.02,
            ),
            error_rate_rel_threshold: env_utils::env_or_default(
                "SUPERVISOR_ERROR_RATE_REL_THRESHOLD",
                0.5,
            ),
            error_rate_abs_threshold: env_utils::env_or_default(
                "SUPERVISOR_ERROR_RATE_ABS_THRESHOLD",
                0.02,
            ),
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.control_interval_secs == 0 {
            return Err("control_interval_secs must be greater than 0".to_string());
        }

        if self.aggregation_interval_secs == 0 {
            return Err("aggregation_interval_secs must be greater than 0".to_string());
        }

        if self.capacity_protection_ratio <= 0.0 || self.capacity_protection_ratio > 1.0 {
            return Err("capacity_protection_ratio must be between 0 and 1".to_string());
        }

        if self.max_adjustment_ratio <= 0.0 || self.max_adjustment_ratio > 1.0 {
            return Err("max_adjustment_ratio must be between 0 and 1".to_string());
        }

        if self.unhealthy_threshold <= 0.0 || self.unhealthy_threshold >= 1.0 {
            return Err("unhealthy_threshold must be between 0 and 1 (exclusive)".to_string());
        }

        if self.batch_size == 0 {
            return Err("batch_size must be greater than 0".to_string());
        }

        // Validate error rate switching parameters
        if self.error_rate_threshold <= 0.0 || self.error_rate_threshold >= 1.0 {
            return Err("error_rate_threshold must be between 0 and 1 (exclusive)".to_string());
        }

        if self.error_rate_recovery_threshold <= 0.0 || self.error_rate_recovery_threshold >= 1.0 {
            return Err(
                "error_rate_recovery_threshold must be between 0 and 1 (exclusive)".to_string(),
            );
        }

        if self.error_rate_threshold <= self.error_rate_recovery_threshold {
            return Err("error_rate_threshold must be greater than error_rate_recovery_threshold (hysteresis)".to_string());
        }

        if self.error_rate_rel_threshold <= 0.0 {
            return Err("error_rate_rel_threshold must be greater than 0".to_string());
        }

        if self.error_rate_abs_threshold <= 0.0 {
            return Err("error_rate_abs_threshold must be greater than 0".to_string());
        }

        Ok(())
    }
}
