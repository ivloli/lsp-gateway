use crate::config::GrowthbookConfig;
use crate::error::{AppError, Result};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GrowthbookFeatures {
    pub features: HashMap<String, GrowthbookFeature>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GrowthbookFeature {
    #[serde(rename = "defaultValue")]
    pub default_value: serde_json::Value,
}

/// Growthbook client that fetches and caches feature flags
pub struct GrowthbookClient {
    config: GrowthbookConfig,
    http_client: Client,
    features_cache: Arc<RwLock<Option<GrowthbookFeatures>>>,
}

impl GrowthbookClient {
    pub fn new(config: GrowthbookConfig) -> Self {
        Self {
            config,
            http_client: Client::new(),
            features_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Fetch features from Growthbook API
    /// Tries proxy host first, falls back to main host if that fails
    async fn fetch_features(&self) -> Result<GrowthbookFeatures> {
        if self.config.client_key.is_empty() {
            return Err(AppError::Internal(
                "Growthbook client key is not configured".to_string(),
            ));
        }

        // Try proxy host first
        let proxy_url = format!(
            "{}/api/features/{}",
            self.config.proxy_host, self.config.client_key
        );
        debug!("Fetching Growthbook features from proxy: {}", proxy_url);

        match self.http_client.get(&proxy_url).send().await {
            Ok(response) if response.status().is_success() => {
                match response.json::<GrowthbookFeatures>().await {
                    Ok(features) => {
                        debug!("Successfully fetched features from Growthbook proxy");
                        return Ok(features);
                    }
                    Err(e) => {
                        warn!("Failed to parse features from proxy: {}", e);
                    }
                }
            }
            Ok(response) => {
                warn!(
                    "Growthbook proxy returned error status: {}",
                    response.status()
                );
            }
            Err(e) => {
                warn!("Failed to fetch from Growthbook proxy: {}", e);
            }
        }

        // Fallback to main host
        let main_url = format!(
            "{}/api/features/{}",
            self.config.host, self.config.client_key
        );
        debug!("Falling back to main Growthbook host: {}", main_url);

        let response =
            self.http_client.get(&main_url).send().await.map_err(|e| {
                AppError::Internal(format!("Failed to fetch from Growthbook: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(AppError::Internal(format!(
                "Growthbook returned error status: {}",
                response.status()
            )));
        }

        response
            .json::<GrowthbookFeatures>()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to parse Growthbook response: {}", e)))
    }

    /// Load features into cache
    pub async fn load_features(&self) -> Result<()> {
        match self.fetch_features().await {
            Ok(features) => {
                let mut cache = self.features_cache.write().await;
                *cache = Some(features);
                Ok(())
            }
            Err(e) => {
                error!("Failed to load Growthbook features: {}", e);
                Err(e)
            }
        }
    }

    /// Get feature value with fallback
    /// Returns the default_value if feature is not found
    pub async fn get_feature_value<T>(&self, feature_key: &str, default_value: T) -> T
    where
        T: serde::de::DeserializeOwned + Clone,
    {
        let cache = self.features_cache.read().await;

        if let Some(features) = cache.as_ref() {
            if let Some(feature) = features.features.get(feature_key) {
                match serde_json::from_value::<T>(feature.default_value.clone()) {
                    Ok(value) => {
                        debug!(
                            "Got Growthbook feature '{}': {:?}",
                            feature_key, feature.default_value
                        );
                        return value;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to deserialize feature '{}': {}, using default",
                            feature_key, e
                        );
                    }
                }
            } else {
                debug!(
                    "Feature '{}' not found in Growthbook, using default",
                    feature_key
                );
            }
        } else {
            debug!(
                "Growthbook features not loaded, using default for '{}'",
                feature_key
            );
        }

        default_value
    }
}

/// Global Growthbook client instance
static GROWTHBOOK_CLIENT: once_cell::sync::OnceCell<Option<Arc<GrowthbookClient>>> =
    once_cell::sync::OnceCell::new();

/// Initialize global Growthbook client
pub fn init_growthbook_client(config: GrowthbookConfig) {
    if !config.enabled {
        debug!("Growthbook is disabled");
        GROWTHBOOK_CLIENT.set(None).ok();
        return;
    }

    // Don't initialize if client_key is empty
    if config.client_key.is_empty() {
        warn!("Growthbook is enabled but GROWTHBOOK_CLIENT_KEY is empty, disabling Growthbook");
        GROWTHBOOK_CLIENT.set(None).ok();
        return;
    }

    let client = Arc::new(GrowthbookClient::new(config));
    GROWTHBOOK_CLIENT.set(Some(client)).ok();
}

/// Get global Growthbook client instance
pub fn get_growthbook_client() -> Option<Arc<GrowthbookClient>> {
    GROWTHBOOK_CLIENT.get().and_then(|c| c.clone())
}

/// Get max surge percentage from Growthbook with fallback
pub async fn get_max_surge_percentage() -> f64 {
    const DEFAULT_SURGE_PERCENTAGE: f64 = 0.3;

    match get_growthbook_client() {
        Some(client) => {
            client
                .get_feature_value("max-surge-percentage", DEFAULT_SURGE_PERCENTAGE)
                .await
        }
        None => {
            debug!(
                "Growthbook client not initialized, using default surge percentage: {}",
                DEFAULT_SURGE_PERCENTAGE
            );
            DEFAULT_SURGE_PERCENTAGE
        }
    }
}

/// Maybe refresh features based on configured probability
/// This should be called on each request to enable probabilistic hot-reload
pub fn maybe_refresh_features(probability: u32) {
    if probability == 0 {
        return;
    }

    // Generate random number and check if we should refresh
    let should_refresh = rand::thread_rng().gen_range(0..probability) == 0;

    if should_refresh {
        if let Some(client) = get_growthbook_client() {
            // Spawn background task to refresh features without blocking
            tokio::spawn(async move {
                match client.load_features().await {
                    Ok(_) => debug!("Probabilistic feature refresh completed successfully"),
                    Err(e) => warn!("Probabilistic feature refresh failed: {}", e),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_feature_value_fallback() {
        let config = GrowthbookConfig {
            enabled: true,
            host: "https://cdn.growthbook.io".to_string(),
            proxy_host: "https://cdn.growthbook.io".to_string(),
            client_key: "test_key".to_string(),
            refresh_interval_secs: 30,
            refresh_probability: 1000,
        };

        let client = GrowthbookClient::new(config);

        // Should return default value when features not loaded
        let value = client.get_feature_value("test-feature", 0.5).await;
        assert_eq!(value, 0.5);
    }

    #[tokio::test]
    async fn test_get_max_surge_percentage_default() {
        // Should return default when client not initialized
        let value = get_max_surge_percentage().await;
        assert_eq!(value, 0.3);
    }
}
