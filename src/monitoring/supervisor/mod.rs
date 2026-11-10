// === Core Infrastructure ===
pub mod config;

// === Domain Modules ===
pub mod health;
pub mod metrics;
pub mod traffic;
pub mod weight_calculation;

// External dependencies
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};
use tokio::time::interval;
use tracing::{error, info};
use uuid::Uuid;

// Internal dependencies
use crate::{error::Result, storage::RedisService};

// Module dependencies
use config::SupervisorConfig;
use health::HealthService;
use metrics::MetricsService;
use traffic::TrafficService;
use weight_calculation::WeightCalculationService;

/// Main Supervisor service for global traffic management
pub struct Supervisor {
    config: SupervisorConfig,
    redis_service: Arc<RedisService>,
    db_pool: Arc<sqlx::PgPool>,
    metrics_service: MetricsService,
    health_service: HealthService,
    running: std::sync::atomic::AtomicBool,
}

impl Supervisor {
    /// Create a new Supervisor instance
    pub async fn new(config: SupervisorConfig, redis_url: &str, db_url: &str) -> Result<Self> {
        info!("Initializing Supervisor");

        let db_pool = Self::create_db_pool(&config, db_url).await?;
        let redis_service = Self::create_redis_service(&config, redis_url).await?;
        let metrics_service = Self::create_metrics_service(&redis_service, &db_pool, &config);
        let health_service = Self::create_health_service();

        Ok(Self {
            config,
            redis_service,
            db_pool,
            metrics_service,
            health_service,
            running: std::sync::atomic::AtomicBool::new(false),
        })
    }

    async fn create_db_pool(config: &SupervisorConfig, db_url: &str) -> Result<Arc<sqlx::PgPool>> {
        let pool = PgPoolOptions::new()
            .max_connections(config.db_max_connections)
            .connect(db_url)
            .await?;
        Ok(Arc::new(pool))
    }

    async fn create_redis_service(
        config: &SupervisorConfig,
        redis_url: &str,
    ) -> Result<Arc<RedisService>> {
        let redis_config = crate::config::RedisConfig {
            url: redis_url.to_string(),
            max_connections: config.redis_max_connections,
            metrics_report_interval: 30,
        };
        let redis_service = Arc::new(RedisService::new(&redis_config).await?);
        redis_service.health_check().await?;
        info!("Redis connection established");
        Ok(redis_service)
    }

    fn create_metrics_service(
        redis_service: &Arc<RedisService>,
        db_pool: &Arc<sqlx::PgPool>,
        config: &SupervisorConfig,
    ) -> MetricsService {
        let metrics_aggregator = metrics::MetricsAggregator::new(
            Arc::clone(redis_service),
            config.aggregation_interval_secs,
            config.gateway_timeout_secs,
        );
        let metrics_converter = metrics::MetricsConverter::new(Arc::clone(db_pool));
        MetricsService::new(metrics_aggregator, metrics_converter)
    }

    fn create_health_service() -> HealthService {
        let composite_scorer = health::CompositeHealthScorer::new();
        HealthService::new(composite_scorer)
    }

    /// Start the supervisor main loop
    pub async fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        self.running
            .store(true, std::sync::atomic::Ordering::Relaxed);

        info!(
            "Starting Supervisor control loop with interval: {}s",
            self.config.control_interval_secs
        );

        let supervisor = Arc::clone(&self);
        tokio::spawn(async move {
            supervisor.control_loop().await;
        })
    }

    async fn control_loop(&self) {
        let mut interval = interval(Duration::from_secs(self.config.control_interval_secs));

        while self.running.load(std::sync::atomic::Ordering::Relaxed) {
            interval.tick().await;

            if let Err(e) = self.run_control_cycle().await {
                error!("Control cycle failed: {}", e);
                tokio::time::sleep(Duration::from_secs(self.config.error_backoff_secs)).await;
            }
        }

        info!("Supervisor control loop stopped");
    }

    /// Stop the supervisor
    pub fn stop(&self) {
        info!("Stopping Supervisor...");
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Main control cycle - process metrics and update health scores
    async fn run_control_cycle(&self) -> Result<()> {
        // Generate unique cycle ID
        let cycle_id = Uuid::new_v4();
        let start_time = std::time::Instant::now();

        let (health_metrics, base_weights) = self.metrics_service.process_metrics().await?;

        if health_metrics.is_empty() {
            return Ok(());
        }

        info!(
            "🔄 [Cycle {}] Processing {} providers",
            cycle_id,
            health_metrics.len()
        );

        // Process health evaluation pipeline
        let health_infos = self
            .health_service
            .evaluate_health(&health_metrics, &base_weights);
        let mut traffic_service =
            TrafficService::new(&self.config, Arc::clone(&self.redis_service));
        let mut final_health_infos = traffic_service.manage_traffic(health_infos).await;

        // 🆕 按组计算权重更新并合并到 health_infos
        self.calculate_and_merge_group_weights(&mut final_health_infos)
            .await?;

        // Store results (now includes weight updates)
        self.redis_service
            .batch_store_provider_health(&final_health_infos)
            .await?;

        let elapsed = start_time.elapsed();
        info!(
            "✅ [Cycle {}] Completed in {:.2}s - Updated {} providers",
            cycle_id,
            elapsed.as_secs_f64(),
            final_health_infos.len()
        );

        Ok(())
    }

    /// 按组计算权重更新并合并到 health_infos
    async fn calculate_and_merge_group_weights(
        &self,
        health_infos: &mut [crate::storage::ProviderHealthInfo],
    ) -> Result<()> {
        use crate::models::ModelProvider;
        use std::collections::HashMap;

        let weight_calculator = WeightCalculationService::new(
            Arc::clone(&self.redis_service),
            Arc::clone(&self.db_pool),
        );

        // 批量查询所有provider信息，避免在循环中单独查询
        let provider_ids: Vec<String> =
            health_infos.iter().map(|h| h.provider_id.clone()).collect();

        let providers = ModelProvider::find_by_ids(&self.db_pool, &provider_ids)
            .await
            .map_err(|e| crate::error::AppError::Database(e))?;

        // 创建provider_id到group_name的映射
        let provider_group_map: HashMap<String, String> = providers
            .into_iter()
            .map(|p| (p.id, p.group_name))
            .collect();

        // 按组分组
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, health_info) in health_infos.iter().enumerate() {
            if let Some(group_name) = provider_group_map.get(&health_info.provider_id) {
                groups.entry(group_name.clone()).or_default().push(i);
            }
        }

        // 为每个组计算权重
        for (group, indices) in groups {
            // 🚨 边界情况处理：如果组内只有一个provider，跳过权重调整
            if indices.len() == 1 {
                info!(
                    "Single provider in group {}: {} - skipping weight adjustment",
                    group, health_infos[indices[0]].provider_id
                );
                continue;
            }

            let mut group_health_infos: Vec<crate::storage::ProviderHealthInfo> =
                indices.iter().map(|&i| health_infos[i].clone()).collect();

            if let Ok(()) = weight_calculator
                .calculate_and_store_group_weights(&group, &mut group_health_infos)
                .await
            {
                // 将更新后的权重信息合并回原数组
                for (group_idx, &original_idx) in indices.iter().enumerate() {
                    health_infos[original_idx] = group_health_infos[group_idx].clone();
                }
            }
        }

        Ok(())
    }
}
