use crate::config::RedisConfig;
use crate::storage::ProviderHealth;
use crate::utils::TimeUtils;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Provider health tracking data
#[derive(Debug, Clone)]
pub struct ProviderHealthTracker {
    pub provider_id: String,
    pub consecutive_fails: u32,
    pub last_success: u64,
    pub response_times: Vec<u64>, // Recent response times for averaging
    pub total_requests: u64,
    pub failed_requests: u64,
}

impl ProviderHealthTracker {
    pub fn new(provider_id: String) -> Self {
        let now = TimeUtils::current_timestamp_secs();

        Self {
            provider_id,
            consecutive_fails: 0,
            last_success: now,
            response_times: Vec::new(),
            total_requests: 0,
            failed_requests: 0,
        }
    }

    pub fn record_success(&mut self, response_time_ms: u64) {
        self.consecutive_fails = 0;
        self.last_success = TimeUtils::current_timestamp_secs();

        self.response_times.push(response_time_ms);
        if self.response_times.len() > 100 {
            self.response_times.remove(0); // Keep only recent 100 samples
        }

        self.total_requests += 1;
    }

    pub fn record_failure(&mut self) {
        self.consecutive_fails += 1;
        self.total_requests += 1;
        self.failed_requests += 1;
    }

    pub fn get_average_response_time(&self) -> u64 {
        if self.response_times.is_empty() {
            0
        } else {
            self.response_times.iter().sum::<u64>() / self.response_times.len() as u64
        }
    }

    pub fn get_error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.failed_requests as f64 / self.total_requests as f64
        }
    }

    pub fn get_health_status(&self) -> String {
        let error_rate = self.get_error_rate();
        let time_since_last_success =
            TimeUtils::current_timestamp_secs().saturating_sub(self.last_success);

        if self.consecutive_fails >= 5 || error_rate > 0.5 || time_since_last_success > 300 {
            "unhealthy".to_string()
        } else if self.consecutive_fails >= 3 || error_rate > 0.2 || time_since_last_success > 120 {
            "degraded".to_string()
        } else {
            "healthy".to_string()
        }
    }

    pub fn to_provider_health(&self) -> ProviderHealth {
        let now = TimeUtils::current_timestamp_secs();

        ProviderHealth {
            provider_id: self.provider_id.clone(),
            status: self.get_health_status(),
            last_success: self.last_success,
            consecutive_fails: self.consecutive_fails,
            response_time: self.get_average_response_time(),
            error_rate: self.get_error_rate(),
            updated_at: now,
        }
    }
}

/// Health monitor service for tracking provider health
#[derive(Clone)]
pub struct HealthMonitor {
    health_trackers: Arc<RwLock<HashMap<String, ProviderHealthTracker>>>,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(_config: &RedisConfig) -> Self {
        Self {
            health_trackers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a successful request for a provider
    pub async fn record_success(&self, provider_id: &str, response_time_ms: u64) {
        let mut trackers = self.health_trackers.write().await;
        let tracker = trackers
            .entry(provider_id.to_string())
            .or_insert_with(|| ProviderHealthTracker::new(provider_id.to_string()));

        tracker.record_success(response_time_ms);
        debug!(
            "Recorded health success for provider {}: {}ms",
            provider_id, response_time_ms
        );
    }

    /// Record a failed request for a provider
    pub async fn record_failure(&self, provider_id: &str) {
        let mut trackers = self.health_trackers.write().await;
        let tracker = trackers
            .entry(provider_id.to_string())
            .or_insert_with(|| ProviderHealthTracker::new(provider_id.to_string()));

        tracker.record_failure();
        debug!("Recorded health failure for provider {}", provider_id);
    }

    /// Record request completion (success or failure with response time)
    pub async fn record_request_completion(
        &self,
        provider_id: &str,
        success: bool,
        response_time_ms: u64,
    ) {
        if success {
            self.record_success(provider_id, response_time_ms).await;
        } else {
            self.record_failure(provider_id).await;
        }
    }

    /// Get current health status for a provider
    pub async fn get_provider_health(&self, provider_id: &str) -> Option<String> {
        let trackers = self.health_trackers.read().await;
        trackers
            .get(provider_id)
            .map(|tracker| tracker.get_health_status())
    }

    /// Get all provider health statuses
    pub async fn get_all_health_statuses(&self) -> HashMap<String, String> {
        let trackers = self.health_trackers.read().await;
        trackers
            .iter()
            .map(|(id, tracker)| (id.clone(), tracker.get_health_status()))
            .collect()
    }

    /// Get health tracker for debugging
    pub async fn get_health_tracker(&self, provider_id: &str) -> Option<ProviderHealthTracker> {
        let trackers = self.health_trackers.read().await;
        trackers.get(provider_id).cloned()
    }

    /// Clear health data for a provider
    pub async fn clear_provider_health(&self, provider_id: &str) {
        let mut trackers = self.health_trackers.write().await;
        if trackers.remove(provider_id).is_some() {
            info!("Cleared health data for provider: {}", provider_id);
        }
    }

    /// Clear all health data
    pub async fn clear_all_health(&self) {
        let mut trackers = self.health_trackers.write().await;
        trackers.clear();
        info!("Cleared all health data");
    }

    /// Get the number of providers being tracked
    pub async fn get_tracked_provider_count(&self) -> usize {
        let trackers = self.health_trackers.read().await;
        trackers.len()
    }

    /// Check if provider is degraded (new method for traffic control)
    pub async fn is_provider_degraded(&self, provider_id: &str) -> bool {
        if let Some(tracker) = self.get_health_tracker(provider_id).await {
            tracker.consecutive_fails >= 3
                || tracker.get_error_rate() > 0.3
                || tracker.get_average_response_time() > 2000 // 2 second threshold
        } else {
            false
        }
    }

    /// Get comprehensive health score (new method for traffic control)
    pub async fn get_health_score(&self, provider_id: &str) -> f64 {
        if let Some(tracker) = self.get_health_tracker(provider_id).await {
            let mut score = 0.0;

            // Error rate impact (50% weight)
            score += tracker.get_error_rate() * 0.5;

            // Latency impact (30% weight)
            let latency_factor = (tracker.get_average_response_time() as f64 / 1000.0).min(1.0);
            score += latency_factor * 0.3;

            // Consecutive failure impact (20% weight)
            let fail_factor = (tracker.consecutive_fails as f64 / 5.0).min(1.0);
            score += fail_factor * 0.2;

            score.min(1.0)
        } else {
            0.0
        }
    }

    /// Get health tracker for a provider (exposed for traffic controller)
    pub async fn get_provider_tracker(&self, provider_id: &str) -> Option<ProviderHealthTracker> {
        self.get_health_tracker(provider_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_health_tracker() {
        let mut tracker = ProviderHealthTracker::new("test_provider".to_string());

        // Initially healthy
        assert_eq!(tracker.get_health_status(), "healthy");

        // Record some successes
        tracker.record_success(100);
        tracker.record_success(150);
        assert_eq!(tracker.get_health_status(), "healthy");
        assert_eq!(tracker.get_average_response_time(), 125);

        // Record failures - with 2 successes and 3 failures, error rate is 3/5 = 60% > 50%, so it should be unhealthy
        tracker.record_failure();
        tracker.record_failure();
        tracker.record_failure();
        assert_eq!(tracker.get_health_status(), "unhealthy");

        // Test degraded state with lower error rate
        let mut tracker2 = ProviderHealthTracker::new("test_provider2".to_string());
        // Record more successes to keep error rate lower
        for _ in 0..7 {
            tracker2.record_success(100);
        }
        // Record 2 failures - error rate will be 2/9 = 22% > 20% but < 50%, so degraded
        tracker2.record_failure();
        tracker2.record_failure();
        assert_eq!(tracker2.get_health_status(), "degraded");
    }

    #[test]
    fn test_error_rate_calculation() {
        let mut tracker = ProviderHealthTracker::new("test_provider".to_string());

        tracker.record_success(100);
        tracker.record_success(100);
        tracker.record_failure();
        tracker.record_failure();

        assert_eq!(tracker.get_error_rate(), 0.5);
    }
}
