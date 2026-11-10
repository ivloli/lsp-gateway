use crate::error::{AppError, Result};
use redis::{aio::ConnectionManager, AsyncCommands, RedisResult};

use tracing::{debug, warn};

/// Gateway instance registration data
#[derive(Debug, Clone)]
pub struct GatewayInstance {
    pub instance_id: String,
    pub hostname: String,
    pub ip_address: String,
    pub port: u16,
    pub version: String,
    pub start_time: u64,
    pub last_heartbeat: u64,
    pub status: String,
}

/// Service for handling gateway instance management in Redis
#[derive(Clone)]
pub struct GatewayService {
    connection_manager: ConnectionManager,
}

impl GatewayService {
    pub fn new(connection_manager: ConnectionManager) -> Self {
        Self { connection_manager }
    }

    /// Register gateway instance
    pub async fn register_gateway(&self, instance: &GatewayInstance) -> Result<()> {
        let key = format!("lsp:gateway:registry:{}", instance.instance_id);

        debug!("Registering gateway instance: {}", instance.instance_id);

        let mut conn = self.connection_manager.clone();

        let result: RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("HSET")
            .arg(&key)
            .arg("instance_id")
            .arg(&instance.instance_id)
            .cmd("HSET")
            .arg(&key)
            .arg("hostname")
            .arg(&instance.hostname)
            .cmd("HSET")
            .arg(&key)
            .arg("ip_address")
            .arg(&instance.ip_address)
            .cmd("HSET")
            .arg(&key)
            .arg("port")
            .arg(instance.port)
            .cmd("HSET")
            .arg(&key)
            .arg("version")
            .arg(&instance.version)
            .cmd("HSET")
            .arg(&key)
            .arg("start_time")
            .arg(instance.start_time)
            .cmd("HSET")
            .arg(&key)
            .arg("last_heartbeat")
            .arg(instance.last_heartbeat)
            .cmd("HSET")
            .arg(&key)
            .arg("status")
            .arg(&instance.status)
            .cmd("EXPIRE")
            .arg(&key)
            .arg(60u64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(_) => {
                debug!(
                    "Successfully registered gateway instance: {}",
                    instance.instance_id
                );
                Ok(())
            }
            Err(e) => {
                warn!(
                    "Failed to register gateway instance {}: {}",
                    instance.instance_id, e
                );
                Err(AppError::Redis(format!(
                    "Failed to register gateway: {}",
                    e
                )))
            }
        }
    }

    /// Update gateway heartbeat
    pub async fn update_gateway_heartbeat(&self, gateway_id: &str) -> Result<()> {
        let key = format!("lsp:gateway:registry:{}", gateway_id);
        let timestamp = crate::utils::TimeUtils::current_timestamp_secs();

        let mut conn = self.connection_manager.clone();

        let result: RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("HSET")
            .arg(&key)
            .arg("last_heartbeat")
            .arg(timestamp)
            .cmd("HSET")
            .arg(&key)
            .arg("status")
            .arg("active")
            .cmd("EXPIRE")
            .arg(&key)
            .arg(60u64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                warn!(
                    "Failed to update heartbeat for gateway {}: {}",
                    gateway_id, e
                );
                Ok(()) // Don't treat heartbeat failures as hard errors
            }
        }
    }

    /// Get providers for a specific group
    pub async fn get_group_providers(&self, group_name: &str) -> Result<Vec<String>> {
        let key = format!("lsp:group:{}:providers", group_name);
        let mut conn = self.connection_manager.clone();

        let result: RedisResult<Vec<String>> = conn.smembers(&key).await;

        match result {
            Ok(providers) => {
                debug!(
                    "Found {} providers for group {}",
                    providers.len(),
                    group_name
                );
                Ok(providers)
            }
            Err(e) => {
                warn!("Failed to get providers for group {}: {}", group_name, e);
                Ok(Vec::new())
            }
        }
    }

    /// Register provider to a group
    pub async fn register_provider_to_group(
        &self,
        group_name: &str,
        provider_id: &str,
    ) -> Result<()> {
        let key = format!("lsp:group:{}:providers", group_name);
        let mut conn = self.connection_manager.clone();

        let result: RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("SADD")
            .arg(&key)
            .arg(provider_id)
            .cmd("EXPIRE")
            .arg(&key)
            .arg(120u64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(_) => {
                debug!(
                    "Registered provider {} to group {}",
                    provider_id, group_name
                );
                Ok(())
            }
            Err(e) => {
                warn!(
                    "Failed to register provider {} to group {}: {}",
                    provider_id, group_name, e
                );
                Ok(()) // Don't treat registration failures as hard errors
            }
        }
    }
}
