use crate::error::Result;
use crate::monitoring::supervisor::config::SupervisorConfig;
use crate::monitoring::supervisor::Supervisor;
use crate::storage::RedisService;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Supervisor Manager - Handles distributed supervisor functionality within Gateway
pub struct SupervisorManager {
    instance_id: String,
    redis_service: Arc<RedisService>,
    supervisor_handle: Option<tokio::task::JoinHandle<()>>,
    supervisor_instance: Arc<RwLock<Option<Arc<Supervisor>>>>,
    config: SupervisorConfig,
    redis_url: String,
    db_url: String,
    running: Arc<RwLock<bool>>,
    is_supervisor_active: Arc<RwLock<bool>>,
}

impl SupervisorManager {
    /// Create a new SupervisorManager
    pub async fn new(
        instance_id: String,
        redis_service: Arc<RedisService>,
        config: SupervisorConfig,
        redis_url: &str,
        db_url: &str,
    ) -> Result<Self> {
        info!(
            "Initializing SupervisorManager for instance: {}",
            instance_id
        );

        Ok(Self {
            instance_id,
            redis_service,
            supervisor_handle: None,
            supervisor_instance: Arc::new(RwLock::new(None)),
            config,
            redis_url: redis_url.to_string(),
            db_url: db_url.to_string(),
            running: Arc::new(RwLock::new(false)),
            is_supervisor_active: Arc::new(RwLock::new(false)),
        })
    }

    /// Start the supervisor manager (attempts to acquire lock and run supervisor)
    pub async fn start(&mut self) -> Result<()> {
        // Check if supervisor is enabled
        if !self.config.enabled {
            info!("Supervisor is disabled via configuration (SUPERVISOR_ENABLED=false), skipping startup for instance: {}", self.instance_id);
            return Ok(());
        }

        {
            let mut running = self.running.write().await;
            *running = true;
        }

        info!(
            "Starting SupervisorManager for instance: {}",
            self.instance_id
        );

        // Start the lock acquisition loop
        let manager = self.clone_for_task().await?;
        let handle = tokio::spawn(async move {
            manager.supervisor_lock_loop().await;
        });

        self.supervisor_handle = Some(handle);
        Ok(())
    }

    /// Stop the supervisor manager
    pub async fn stop(&mut self) {
        info!(
            "Stopping SupervisorManager for instance: {}",
            self.instance_id
        );

        {
            let mut running = self.running.write().await;
            *running = false;
        }

        // Release lock if we hold it
        if *self.is_supervisor_active.read().await {
            if let Err(e) = self
                .redis_service
                .release_supervisor_lock(&self.instance_id)
                .await
            {
                error!("Failed to release supervisor lock during shutdown: {}", e);
            }
        }

        // Stop supervisor handle
        if let Some(handle) = self.supervisor_handle.take() {
            handle.abort();
        }
    }

    /// Get supervisor status
    pub async fn get_status(&self) -> SupervisorManagerStatus {
        let is_supervisor_active = *self.is_supervisor_active.read().await;
        let lock_holder = self
            .redis_service
            .get_supervisor_lock_holder()
            .await
            .unwrap_or(None);

        SupervisorManagerStatus {
            instance_id: self.instance_id.clone(),
            is_supervisor_active,
            lock_holder,
            config: self.config.clone(),
        }
    }

    /// Helper to clone necessary data for tasks
    async fn clone_for_task(&self) -> Result<SupervisorManagerTask> {
        Ok(SupervisorManagerTask {
            instance_id: self.instance_id.clone(),
            redis_service: Arc::clone(&self.redis_service),
            supervisor_instance: Arc::clone(&self.supervisor_instance),
            config: self.config.clone(),
            redis_url: self.redis_url.clone(),
            db_url: self.db_url.clone(),
            running: Arc::clone(&self.running),
            is_supervisor_active: Arc::clone(&self.is_supervisor_active),
        })
    }
}

/// Lightweight version for async tasks
#[derive(Clone)]
struct SupervisorManagerTask {
    instance_id: String,
    redis_service: Arc<RedisService>,
    supervisor_instance: Arc<RwLock<Option<Arc<Supervisor>>>>,
    config: SupervisorConfig,
    redis_url: String,
    db_url: String,
    running: Arc<RwLock<bool>>,
    is_supervisor_active: Arc<RwLock<bool>>,
}

impl SupervisorManagerTask {
    async fn supervisor_lock_loop(self) {
        let mut lock_attempt_interval =
            interval(Duration::from_secs(self.config.lock_attempt_interval_secs));
        let mut lock_renewal_interval =
            interval(Duration::from_secs(self.config.lock_renewal_interval_secs));

        loop {
            tokio::select! {
                // Check if we should stop
                _ = async {
                    loop {
                        {
                            let running = self.running.read().await;
                            if !*running {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                } => {
                    info!("SupervisorManager stopping lock loop for instance: {}", self.instance_id);
                    break;
                }

                // Try to acquire lock periodically
                _ = lock_attempt_interval.tick() => {
                    if !*self.is_supervisor_active.read().await {
                        self.try_acquire_supervisor_role().await;
                    }
                }

                // Renew lock if we have it
                _ = lock_renewal_interval.tick() => {
                    if *self.is_supervisor_active.read().await {
                        self.try_renew_supervisor_lock().await;
                    }
                }
            }
        }

        // Cleanup on exit
        if *self.is_supervisor_active.read().await {
            if let Err(e) = self
                .redis_service
                .release_supervisor_lock(&self.instance_id)
                .await
            {
                error!("Failed to release supervisor lock during loop exit: {}", e);
            }
        }
    }

    async fn try_acquire_supervisor_role(&self) {
        match self
            .redis_service
            .acquire_supervisor_lock(&self.instance_id, self.config.lock_ttl_secs)
            .await
        {
            Ok(true) => {
                info!(
                    "✅ Acquired supervisor role for instance: {}",
                    self.instance_id
                );
                {
                    let mut is_active = self.is_supervisor_active.write().await;
                    *is_active = true;
                }

                // Start supervisor functionality
                if let Err(e) = self.start_supervisor_functionality().await {
                    error!("Failed to start supervisor functionality: {}", e);
                    // Release lock on failure
                    if let Err(e2) = self
                        .redis_service
                        .release_supervisor_lock(&self.instance_id)
                        .await
                    {
                        error!("Failed to release lock after startup failure: {}", e2);
                    }
                    {
                        let mut is_active = self.is_supervisor_active.write().await;
                        *is_active = false;
                    }
                }
            }
            Ok(false) => {
                debug!("Supervisor lock held by another instance");
            }
            Err(e) => {
                warn!("Failed to acquire supervisor lock: {}", e);
            }
        }
    }

    async fn try_renew_supervisor_lock(&self) {
        match self
            .redis_service
            .renew_supervisor_lock(&self.instance_id, self.config.lock_ttl_secs)
            .await
        {
            Ok(true) => {
                // debug!("🔄 Renewed supervisor lock for instance: {}", self.instance_id);
            }
            Ok(false) => {
                warn!(
                    "❌ Lost supervisor role - lock not owned by instance: {}",
                    self.instance_id
                );
                {
                    let mut is_active = self.is_supervisor_active.write().await;
                    *is_active = false;
                }
                self.stop_supervisor_functionality().await;
            }
            Err(e) => {
                error!("Failed to renew supervisor lock: {}", e);
                // Assume we've lost the lock
                {
                    let mut is_active = self.is_supervisor_active.write().await;
                    *is_active = false;
                }
                self.stop_supervisor_functionality().await;
            }
        }
    }

    async fn start_supervisor_functionality(&self) -> Result<()> {
        info!(
            "🚀 Starting supervisor functionality for instance: {}",
            self.instance_id
        );

        // Create a new Supervisor instance
        let supervisor =
            Arc::new(Supervisor::new(self.config.clone(), &self.redis_url, &self.db_url).await?);

        // Start the supervisor control loop
        let _supervisor_handle = supervisor.clone().start().await;

        // Store the supervisor instance
        {
            let mut instance = self.supervisor_instance.write().await;
            *instance = Some(supervisor);
        }

        // Note: We don't store the supervisor_handle here because SupervisorManagerTask
        // is only for the lock management loop. The actual supervisor runs independently.
        info!(
            "✅ Supervisor instance started successfully for: {}",
            self.instance_id
        );

        Ok(())
    }

    async fn stop_supervisor_functionality(&self) {
        info!(
            "🛑 Stopping supervisor functionality for instance: {}",
            self.instance_id
        );

        // Stop the supervisor instance if it exists
        let supervisor = {
            let mut instance = self.supervisor_instance.write().await;
            instance.take()
        };

        if let Some(supervisor) = supervisor {
            supervisor.stop();
            info!("✅ Supervisor instance stopped for: {}", self.instance_id);
        } else {
            debug!("No supervisor instance to stop for: {}", self.instance_id);
        }
    }
}

/// Status information for the supervisor manager
#[derive(Debug, Clone)]
pub struct SupervisorManagerStatus {
    pub instance_id: String,
    pub is_supervisor_active: bool,
    pub lock_holder: Option<String>,
    pub config: SupervisorConfig,
}
