pub mod model_registry;

use crate::error::Result;
use crate::gateway::StreamTimeoutConfig;

// Re-export the model registry for easy access
pub use model_registry::{
    EndpointType, ModelRegistry, ModelRegistryEntry, RegistryStats, TemplateDefinition,
    MODEL_REGISTRY, TEMPLATES,
};

#[derive(Debug, Clone)]
pub struct Config {
    pub database: DatabaseConfig,
    pub server: ServerConfig,
    pub cache: CacheConfig,
    pub redis: RedisConfig,
    pub timeouts: TimeoutConfig,
    pub routing: RoutingConfig,
    pub tracing: TracingConfig,
    pub frp: FrpConfig,
    pub growthbook: GrowthbookConfig,
    pub retry: RetryConfig,
}

#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    pub response_finish_timeout_ms: u64,
    pub stream_no_token_timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_address: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub max_capacity: u64,
    pub time_to_live: u64,
}

#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub max_connections: u32,
    pub metrics_report_interval: u64,
}

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub strategy: String,
}

#[derive(Debug, Clone)]
pub struct FrpConfig {
    pub frp_enabled: bool,
    pub url: String,
    pub passwd: String,
    pub conns_limit: u32,
}

#[derive(Debug, Clone)]
pub struct TracingConfig {
    pub tracing_enabled: bool,
    pub tempo_endpoint: String,
    pub service_name: String,
    pub service_version: String,
    pub sampling_ratio: f64,
    pub debug_mode: bool,
}

#[derive(Debug, Clone)]
pub struct GrowthbookConfig {
    pub enabled: bool,
    pub host: String,
    pub proxy_host: String,
    pub client_key: String,
    /// Interval in seconds for periodic feature refresh (default: 30)
    pub refresh_interval_secs: u64,
    /// Probability for random feature refresh (1/N chance per request, default: 1000)
    pub refresh_probability: u32,
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub jitter_enabled: bool,
    pub retry_on_rate_limit: bool,
}

/// Environment variable utilities for consistent parsing across the application
pub mod env_utils {
    use std::env;

    /// Parse environment variable with default value and type conversion
    /// Supports all types that implement FromStr: bool, i32, u64, f64, String, etc.
    ///
    /// Examples:
    /// - `env_or_default("SERVER_PORT", 8080)` -> u16
    /// - `env_or_default("SUPERVISOR_ENABLED", true)` -> bool  
    /// - `env_or_default("ROUTING_STRATEGY", "default".to_string())` -> String
    pub fn env_or_default<T: std::str::FromStr>(key: &str, default: T) -> T {
        env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// Get database URL from the environment
    pub fn get_database_url() -> String {
        env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost/lsp_gateway".to_string())
    }

    /// Get Redis URL with fallback logic
    /// First tries REDIS_URL, then RATELIMIT_REDIS_URL, finally uses default
    pub fn get_redis_url() -> String {
        env::var("REDIS_URL")
            .or_else(|_| env::var("RATELIMIT_REDIS_URL"))
            .unwrap_or_else(|_| "redis://localhost:6379".to_string())
    }
}

impl Config {
    /// Load configuration from environment variables with simplified validation
    pub fn from_env() -> Result<Self> {
        Ok(Config {
            database: DatabaseConfig {
                url: env_utils::get_database_url(),
                max_connections: env_utils::env_or_default("DATABASE_MAX_CONNECTIONS", 10),
            },
            server: ServerConfig {
                bind_address: env_utils::env_or_default(
                    "SERVER_BIND_ADDRESS",
                    "0.0.0.0".to_string(),
                ),
                port: env_utils::env_or_default("SERVER_PORT", 8080),
            },
            cache: CacheConfig {
                max_capacity: env_utils::env_or_default("CACHE_MAX_CAPACITY", 10000),
                time_to_live: env_utils::env_or_default("CACHE_TTL_SECONDS", 30),
            },
            redis: RedisConfig {
                url: env_utils::get_redis_url(),
                max_connections: env_utils::env_or_default("REDIS_MAX_CONNECTIONS", 10),
                metrics_report_interval: env_utils::env_or_default(
                    "METRICS_REPORT_INTERVAL_SECONDS",
                    3,
                ),
            },
            timeouts: TimeoutConfig {
                response_finish_timeout_ms: env_utils::env_or_default(
                    "SERVER_RESPONSE_TIMEOUT_MS",
                    55000,
                ),
                stream_no_token_timeout_ms: env_utils::env_or_default(
                    "SERVER_STREAM_NO_TOKEN_TIMEOUT_MS",
                    10000, // 10s for default TTFT timeout
                ),
            },
            routing: RoutingConfig {
                strategy: env_utils::env_or_default(
                    "ROUTING_STRATEGY",
                    "smooth_weighted_round_robin".to_string(),
                ),
            },
            frp: FrpConfig {
                frp_enabled: env_utils::env_or_default("FRP_ENABLED", false),
                url: env_utils::env_or_default("FRP_URL", "http://localhost:10086".to_string()),
                passwd: env_utils::env_or_default("FRP_PASSWD", "".to_string()),
                conns_limit: env_utils::env_or_default("FRP_CONNS_LIMIT", 50),
            },
            tracing: TracingConfig {
                tracing_enabled: env_utils::env_or_default("TRACING_ENABLED", false),
                tempo_endpoint: env_utils::env_or_default(
                    "OTLP_TRACES_URL",
                    "http://localhost:4318".to_string(),
                ),
                service_name: env_utils::env_or_default("SERVICE_NAME", "lsp-gateway".to_string()),
                service_version: env_utils::env_or_default("SERVICE_VERSION", "0.1.0".to_string()),
                sampling_ratio: env_utils::env_or_default("TRACING_SAMPLING_RATIO", 1.0),
                debug_mode: env_utils::env_or_default("TRACING_DEBUG", false),
            },
            growthbook: GrowthbookConfig {
                enabled: env_utils::env_or_default("GROWTHBOOK_ENABLED", false),
                host: env_utils::env_or_default(
                    "GROWTHBOOK_HOST",
                    "https://cdn.growthbook.io".to_string(),
                ),
                proxy_host: env_utils::env_or_default(
                    "GROWTHBOOK_PROXY_HOST",
                    "https://cdn.growthbook.io".to_string(),
                ),
                client_key: env_utils::env_or_default("GROWTHBOOK_CLIENT_KEY", "".to_string()),
                refresh_interval_secs: env_utils::env_or_default(
                    "GROWTHBOOK_REFRESH_INTERVAL_SECS",
                    30,
                ),
                refresh_probability: env_utils::env_or_default(
                    "GROWTHBOOK_REFRESH_PROBABILITY",
                    1000,
                ),
            },
            retry: RetryConfig {
                max_retries: env_utils::env_or_default("RETRY_MAX_RETRIES", 3),
                base_backoff_ms: env_utils::env_or_default("RETRY_BASE_BACKOFF_MS", 1000),
                max_backoff_ms: env_utils::env_or_default("RETRY_MAX_BACKOFF_MS", 10000),
                jitter_enabled: env_utils::env_or_default("RETRY_JITTER_ENABLED", true),
                retry_on_rate_limit: env_utils::env_or_default("RETRY_ON_RATE_LIMIT", true),
            },
        })
    }
}

impl TimeoutConfig {
    pub fn to_stream_timeout_config(&self) -> StreamTimeoutConfig {
        StreamTimeoutConfig {
            response_finish_timeout_ms: self.response_finish_timeout_ms,
            stream_no_token_timeout_ms: self.stream_no_token_timeout_ms,
        }
    }

    pub fn to_stream_timeout_config_with_model_override(
        &self,
        model_stream_timeout_ms: Option<u64>,
        model_response_finish_timeout_ms: Option<u64>,
    ) -> StreamTimeoutConfig {
        StreamTimeoutConfig {
            response_finish_timeout_ms: model_response_finish_timeout_ms
                .unwrap_or(self.response_finish_timeout_ms),
            stream_no_token_timeout_ms: model_stream_timeout_ms
                .unwrap_or(self.stream_no_token_timeout_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::env_utils;
    use std::env;

    #[test]
    fn test_env_or_default_bool() {
        // Test with no env var set
        let result = env_utils::env_or_default("TEST_BOOL_NOT_SET", true);
        assert_eq!(result, true);

        // Test with env var set to false
        env::set_var("TEST_BOOL_FALSE", "false");
        let result = env_utils::env_or_default("TEST_BOOL_FALSE", true);
        assert_eq!(result, false);

        // Test with env var set to true
        env::set_var("TEST_BOOL_TRUE", "true");
        let result = env_utils::env_or_default("TEST_BOOL_TRUE", false);
        assert_eq!(result, true);

        // Cleanup
        env::remove_var("TEST_BOOL_FALSE");
        env::remove_var("TEST_BOOL_TRUE");
    }

    #[test]
    fn test_env_or_default_string() {
        // Test with no env var set
        let result = env_utils::env_or_default("TEST_STRING_NOT_SET", "default".to_string());
        assert_eq!(result, "default");

        // Test with env var set
        env::set_var("TEST_STRING_SET", "custom_value");
        let result = env_utils::env_or_default("TEST_STRING_SET", "default".to_string());
        assert_eq!(result, "custom_value");

        // Cleanup
        env::remove_var("TEST_STRING_SET");
    }

    #[test]
    fn test_env_or_default_numeric() {
        // Test with no env var set
        let result = env_utils::env_or_default("TEST_NUMERIC_NOT_SET", 42);
        assert_eq!(result, 42);

        // Test with env var set
        env::set_var("TEST_NUMERIC_SET", "100");
        let result = env_utils::env_or_default("TEST_NUMERIC_SET", 42);
        assert_eq!(result, 100);

        // Test with invalid value (should use default)
        env::set_var("TEST_NUMERIC_INVALID", "not_a_number");
        let result = env_utils::env_or_default("TEST_NUMERIC_INVALID", 42);
        assert_eq!(result, 42);

        // Cleanup
        env::remove_var("TEST_NUMERIC_SET");
        env::remove_var("TEST_NUMERIC_INVALID");
    }

    #[test]
    fn test_routing_config_from_env() {
        use super::Config;

        // Test default routing strategy
        let config = Config::from_env().expect("Config should load successfully");
        assert_eq!(config.routing.strategy, "smooth_weighted_round_robin");

        // Test custom routing strategy
        env::set_var("ROUTING_STRATEGY", "weighted_random");
        let config = Config::from_env().expect("Config should load successfully");
        assert_eq!(config.routing.strategy, "weighted_random");

        // Cleanup
        env::remove_var("ROUTING_STRATEGY");
    }

    #[test]
    fn test_database_url_fallback() {
        use super::env_utils;

        // Ensure clean state
        env::remove_var("DATABASE_URL");

        // Test default when unset
        let result = env_utils::get_database_url();
        assert_eq!(result, "postgresql://localhost/lsp_gateway");

        // Test DATABASE_URL override
        env::set_var("DATABASE_URL", "postgresql://primary:5432/main_db");
        let result = env_utils::get_database_url();
        assert_eq!(result, "postgresql://primary:5432/main_db");

        // Cleanup
        env::remove_var("DATABASE_URL");
    }

    #[test]
    fn test_redis_url_fallback() {
        use super::env_utils;

        // Ensure clean state
        env::remove_var("REDIS_URL");
        env::remove_var("RATELIMIT_REDIS_URL");

        // Test default when neither is set
        let result = env_utils::get_redis_url();
        assert_eq!(result, "redis://localhost:6379");

        // Test RATELIMIT_REDIS_URL when REDIS_URL is not set
        env::set_var("RATELIMIT_REDIS_URL", "redis://ratelimit:6380/1");
        let result = env_utils::get_redis_url();
        assert_eq!(result, "redis://ratelimit:6380/1");

        // Test REDIS_URL takes priority
        env::set_var("REDIS_URL", "redis://primary:6379/0");
        let result = env_utils::get_redis_url();
        assert_eq!(result, "redis://primary:6379/0");

        // Cleanup
        env::remove_var("REDIS_URL");
        env::remove_var("RATELIMIT_REDIS_URL");
    }
}
