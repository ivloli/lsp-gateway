use crate::monitoring::MetricsReporter;
use crate::routing::FrpManager;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Background task manager for handling periodic operations
pub struct BackgroundTaskManager {
    tasks: Vec<JoinHandle<()>>,
}

impl BackgroundTaskManager {
    /// Create a new background task manager
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Start metrics reporting task
    pub async fn start_metrics_reporting(&mut self, metrics_reporter: Arc<MetricsReporter>) {
        info!("Starting metrics reporting background task");
        let task = metrics_reporter.start_reporting_task().await;

        self.tasks.push(task);
    }

    pub async fn start_sync_frp_task(&mut self, frp_manager: Option<Arc<FrpManager>>) {
        if let Some(manager) = frp_manager {
            let handle = manager.start_update_task();
            self.tasks.push(handle);
        }
    }

    /// Start all background tasks
    pub async fn start_all_tasks(
        &mut self,
        metrics_reporter: Option<Arc<MetricsReporter>>,
        frp_manager: Option<Arc<FrpManager>>,
    ) {
        if let Some(reporter) = metrics_reporter {
            self.start_metrics_reporting(reporter).await;
        }

        if frp_manager.is_some() {
            self.start_sync_frp_task(frp_manager).await;
            info!("Started FRP sync task");
        }

        info!("Started {} background tasks", self.tasks.len());
    }

    /// Gracefully shutdown all background tasks
    pub async fn shutdown(self) {
        info!("Shutting down {} background tasks", self.tasks.len());

        for task in self.tasks {
            task.abort();
            if let Err(e) = task.await {
                if !e.is_cancelled() {
                    error!("Background task failed during shutdown: {}", e);
                }
            }
        }

        info!("All background tasks have been shut down");
    }

    /// Get the number of running tasks
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Check if any tasks have finished (which would indicate an error)
    pub fn check_task_health(&mut self) -> Vec<String> {
        let mut failed_tasks = Vec::new();
        let mut active_tasks = Vec::new();

        for (index, task) in self.tasks.iter().enumerate() {
            if task.is_finished() {
                failed_tasks.push(format!("Task {} has finished unexpectedly", index));
            } else {
                active_tasks.push(task);
            }
        }

        // Keep only active tasks
        self.tasks.retain(|task| !task.is_finished());

        if !failed_tasks.is_empty() {
            error!("Found {} failed background tasks", failed_tasks.len());
        }

        failed_tasks
    }
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Health check for background tasks
pub struct BackgroundTaskHealthCheck {
    task_manager: Arc<tokio::sync::Mutex<BackgroundTaskManager>>,
}

impl BackgroundTaskHealthCheck {
    pub fn new(task_manager: BackgroundTaskManager) -> Self {
        Self {
            task_manager: Arc::new(tokio::sync::Mutex::new(task_manager)),
        }
    }

    /// Check the health of all background tasks
    pub async fn check_health(&self) -> Result<(), Vec<String>> {
        let mut manager = self.task_manager.lock().await;
        let failed_tasks = manager.check_task_health();

        if failed_tasks.is_empty() {
            Ok(())
        } else {
            Err(failed_tasks)
        }
    }

    /// Get the number of active tasks
    pub async fn active_task_count(&self) -> usize {
        let manager = self.task_manager.lock().await;
        manager.task_count()
    }

    /// Shutdown all tasks
    pub async fn shutdown(self) {
        match Arc::try_unwrap(self.task_manager) {
            Ok(mutex) => {
                let manager = mutex.into_inner();
                manager.shutdown().await;
            }
            Err(_) => {
                warn!("Failed to unwrap Arc for task manager shutdown - multiple references exist");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_background_task_manager_creation() {
        let manager = BackgroundTaskManager::new();
        assert_eq!(manager.task_count(), 0);
    }

    #[tokio::test]
    async fn test_background_task_manager_default() {
        let manager = BackgroundTaskManager::default();
        assert_eq!(manager.task_count(), 0);
    }

    #[tokio::test]
    async fn test_health_check_creation() {
        let manager = BackgroundTaskManager::new();
        let health_check = BackgroundTaskHealthCheck::new(manager);
        assert_eq!(health_check.active_task_count().await, 0);
    }
}
