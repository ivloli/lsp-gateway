use crate::error::{AppError, Result};
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{debug, info, warn};

/// Service for handling distributed locks in Redis
#[derive(Clone)]
pub struct LockService {
    connection_manager: ConnectionManager,
}

impl LockService {
    pub fn new(connection_manager: ConnectionManager) -> Self {
        Self { connection_manager }
    }

    /// Acquire distributed lock for supervisor functionality
    pub async fn acquire_supervisor_lock(&self, instance_id: &str, ttl_secs: u64) -> Result<bool> {
        let mut conn = self.connection_manager.clone();
        let lock_key = "lsp:supervisor:lock";

        // Use SET with NX (only set if not exists) and EX (expire)
        let result: Option<String> = redis::cmd("SET")
            .arg(lock_key)
            .arg(instance_id)
            .arg("NX") // Only set if key doesn't exist
            .arg("EX") // Set expire time in seconds
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to acquire supervisor lock: {}", e)))?;

        match result {
            Some(_) => {
                info!("Supervisor lock acquired by instance: {}", instance_id);
                Ok(true)
            }
            None => {
                debug!("Supervisor lock already held by another instance");
                Ok(false)
            }
        }
    }

    /// Renew supervisor lock (extend TTL)
    pub async fn renew_supervisor_lock(&self, instance_id: &str, ttl_secs: u64) -> Result<bool> {
        let mut conn = self.connection_manager.clone();
        let lock_key = "lsp:supervisor:lock";

        // Use Lua script to atomically check owner and renew
        let script = r#"
            if redis.call("GET", KEYS[1]) == ARGV[1] then
                return redis.call("EXPIRE", KEYS[1], ARGV[2])
            else
                return 0
            end
        "#;

        let result: i32 = redis::Script::new(script)
            .key(lock_key)
            .arg(instance_id)
            .arg(ttl_secs)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to renew supervisor lock: {}", e)))?;

        if result == 1 {
            Ok(true)
        } else {
            warn!(
                "Failed to renew supervisor lock - not owned by instance: {}",
                instance_id
            );
            Ok(false)
        }
    }

    /// Release supervisor lock
    pub async fn release_supervisor_lock(&self, instance_id: &str) -> Result<bool> {
        let mut conn = self.connection_manager.clone();
        let lock_key = "lsp:supervisor:lock";

        // Use Lua script to atomically check owner and delete
        let script = r#"
            if redis.call("GET", KEYS[1]) == ARGV[1] then
                return redis.call("DEL", KEYS[1])
            else
                return 0
            end
        "#;

        let result: i32 = redis::Script::new(script)
            .key(lock_key)
            .arg(instance_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to release supervisor lock: {}", e)))?;

        if result == 1 {
            info!("Supervisor lock released by instance: {}", instance_id);
            Ok(true)
        } else {
            warn!(
                "Failed to release supervisor lock - not owned by instance: {}",
                instance_id
            );
            Ok(false)
        }
    }

    /// Check current supervisor lock holder
    pub async fn get_supervisor_lock_holder(&self) -> Result<Option<String>> {
        let mut conn = self.connection_manager.clone();
        let lock_key = "lsp:supervisor:lock";

        let holder: Option<String> = conn
            .get(lock_key)
            .await
            .map_err(|e| AppError::Redis(format!("Failed to get supervisor lock holder: {}", e)))?;

        Ok(holder)
    }
}
