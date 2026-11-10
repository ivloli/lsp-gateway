use chrono::{DateTime, Utc};
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, error};

// Static cache instances for ModelProvider
static PROVIDER_CACHE: OnceLock<Cache<String, ModelProvider>> = OnceLock::new();
static GROUP_CACHE: OnceLock<Cache<String, Vec<ModelProvider>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ModelProvider {
    pub id: String,
    #[sqlx(rename = "group")]
    pub group_name: String,
    pub provider: String,
    pub weight: i32,
    #[sqlx(rename = "replicaCount")]
    pub replica_count: String,
    #[sqlx(rename = "baseUrl")]
    pub base_url: String,
    #[sqlx(rename = "maxRps")]
    pub max_rps: i32,
    #[sqlx(rename = "minRps")]
    pub min_rps: i32,
    pub rps: Option<i32>,
    pub tier: i32,
    pub active: bool,
    #[sqlx(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sqlx(rename = "updatedAt")]
    pub updated_at: Option<DateTime<Utc>>,
    #[sqlx(rename = "overrideModelName")]
    pub override_model_name: Option<String>,
    #[sqlx(rename = "apiKeyEnvName")]
    pub api_key_env_name: Option<String>,
    pub frpmodel: Option<String>,
}

impl ModelProvider {
    /// Initialize caches - called once at startup
    fn init_caches() {
        PROVIDER_CACHE.get_or_init(|| {
            Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(30)) // 5 minutes
                .build()
        });
        GROUP_CACHE.get_or_init(|| {
            Cache::builder()
                .max_capacity(500)
                .time_to_live(Duration::from_secs(30)) // 5 minutes
                .build()
        });
    }

    /// Check if frpmodel field exists and is not empty
    pub fn has_frpmodel(&self) -> bool {
        self.frpmodel.as_ref().map_or(false, |s| !s.is_empty())
    }

    #[tracing::instrument(skip(pool), fields(db.operation = "query", db.table = "ModelProvider"))]
    pub async fn find_active_by_group(
        pool: &sqlx::PgPool,
        group: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        Self::init_caches();
        let group_cache = GROUP_CACHE
            .get()
            .ok_or_else(|| sqlx::Error::Configuration("Group cache not initialized".into()))?;

        // Check cache first
        if let Some(cached_providers) = group_cache.get(group).await {
            debug!("Cache hit for provider group: {}", group);
            return Ok(cached_providers);
        }

        // Query database
        let results = sqlx::query_as::<_, ModelProvider>(
            r#"
            SELECT id, "group", provider, weight, "replicaCount", "baseUrl", 
                   "maxRps", "minRps", rps, tier, active, "createdAt", "updatedAt","overrideModelName", "apiKeyEnvName", frpmodel
            FROM "ModelProvider" 
            WHERE "group" = $1 AND active = true
            "#,
        )
        .bind(group)
        .fetch_all(pool)
        .await
        .map_err(|db_error| {
            error!("❌ DB_ERROR: Failed to query active providers for group '{}': {}", group, db_error);
            db_error
        })?;

        // Cache the result
        group_cache.insert(group.to_string(), results.clone()).await;
        debug!("Loaded and cached provider group: {}", group);

        Ok(results)
    }

    #[tracing::instrument(skip(pool), fields(db.operation = "query", db.table = "ModelProvider"))]
    pub async fn find_by_id(pool: &sqlx::PgPool, id: &str) -> Result<Option<Self>, sqlx::Error> {
        Self::init_caches();
        let provider_cache = PROVIDER_CACHE
            .get()
            .ok_or_else(|| sqlx::Error::Configuration("Provider cache not initialized".into()))?;

        // Check cache first
        if let Some(cached_provider) = provider_cache.get(id).await {
            debug!("Cache hit for model provider: {}", id);
            return Ok(Some(cached_provider));
        }

        // Query database
        let result = sqlx::query_as::<_, ModelProvider>(
            r#"
            SELECT id, "group", provider, weight, "replicaCount", "baseUrl", 
                   "maxRps", "minRps", rps, tier, active, "createdAt", "updatedAt","overrideModelName", "apiKeyEnvName", frpmodel
            FROM "ModelProvider" 
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|db_error| {
            error!("❌ Failed to query provider by ID '{}': {}", id, db_error);
            db_error
        })?;

        // Cache the result if found
        if let Some(ref provider) = result {
            provider_cache
                .insert(id.to_string(), provider.clone())
                .await;
            debug!("Loaded and cached model provider: {}", id);
        }

        Ok(result)
    }

    /// Batch query providers by IDs to optimize database access
    pub async fn find_by_ids(
        pool: &sqlx::PgPool,
        ids: &[String],
    ) -> Result<Vec<Self>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        Self::init_caches();
        let provider_cache = PROVIDER_CACHE
            .get()
            .ok_or_else(|| sqlx::Error::Configuration("Provider cache not initialized".into()))?;

        let mut results = Vec::new();
        let mut missing_ids = Vec::new();

        // Check cache for each ID
        for id in ids {
            if let Some(cached_provider) = provider_cache.get(id).await {
                results.push(cached_provider);
            } else {
                missing_ids.push(id.as_str());
            }
        }

        // Query database for missing IDs
        if !missing_ids.is_empty() {
            let placeholders: Vec<String> =
                (1..=missing_ids.len()).map(|i| format!("${}", i)).collect();
            let query = format!(
                r#"
                SELECT id, "group", provider, weight, "replicaCount", "baseUrl", 
                       "maxRps", "minRps", rps, tier, active, "createdAt", "updatedAt","overrideModelName", "apiKeyEnvName", frpmodel
                FROM "ModelProvider" 
                WHERE id IN ({})
                "#,
                placeholders.join(", ")
            );

            let mut query_builder = sqlx::query_as::<_, ModelProvider>(&query);
            for id in &missing_ids {
                query_builder = query_builder.bind(id);
            }

            let db_results = query_builder.fetch_all(pool).await?;

            // Cache the results
            for provider in &db_results {
                provider_cache
                    .insert(provider.id.clone(), provider.clone())
                    .await;
            }

            results.extend(db_results);
            debug!(
                "Batch loaded and cached {} model providers",
                missing_ids.len()
            );
        }

        Ok(results)
    }
}
