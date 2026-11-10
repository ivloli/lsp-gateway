use crate::config::Config;
use crate::error::Result;
use crate::handlers;
use crate::middleware::{growthbook_refresh_middleware, trace_context_middleware};
use crate::{
    gateway::{generate_gateway_id, BackgroundTaskManager, SupervisorManager},
    monitoring::{supervisor::config::SupervisorConfig, HealthMonitor, MetricsReporter},
    routing::{frp_service::FrpManager, ProviderService},
    storage::{CacheService, RedisService},
};
use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use futures::executor::block_on;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub provider_service: Arc<ProviderService>,
    pub background_task_manager: Option<Arc<tokio::sync::Mutex<BackgroundTaskManager>>>,
    pub prometheus_metrics: Option<Arc<crate::monitoring::PrometheusMetrics>>,
    pub config: Config,
}

/// Redis-related services container
struct RedisServices {
    redis_service: Arc<RedisService>,
    metrics_reporter: Arc<MetricsReporter>,
    health_monitor: Arc<HealthMonitor>,
    supervisor_manager: Arc<tokio::sync::Mutex<SupervisorManager>>,
}

/// Application builder for managing complex initialization
pub struct AppBuilder {
    config: Config,
    gateway_id: String,
    database_pool: Option<sqlx::PgPool>,
    cache_service: Option<Arc<CacheService>>,
    prometheus_metrics: Option<Arc<crate::monitoring::PrometheusMetrics>>,
    redis_services: Option<RedisServices>,
}

impl AppBuilder {
    /// Create a new application builder
    pub fn new(config: Config) -> Self {
        let gateway_id = generate_gateway_id();
        info!("Gateway instance ID: {}", gateway_id);

        Self {
            config,
            gateway_id,
            database_pool: None,
            cache_service: None,
            prometheus_metrics: None,
            redis_services: None,
        }
    }

    /// Initialize database connection pool
    pub async fn with_database(mut self) -> Result<Self> {
        info!(
            "Initializing database connection pool with {} max connections",
            self.config.database.max_connections
        );

        let pool = PgPoolOptions::new()
            .max_connections(self.config.database.max_connections)
            .connect(&self.config.database.url)
            .await?;

        self.database_pool = Some(pool);
        Ok(self)
    }

    /// Initialize cache service
    pub fn with_cache(mut self) -> Self {
        info!(
            "Initializing cache service with capacity: {}, TTL: {}s",
            self.config.cache.max_capacity, self.config.cache.time_to_live
        );

        let cache_service = Arc::new(CacheService::new(
            self.config.cache.max_capacity,
            self.config.cache.time_to_live,
        ));

        self.cache_service = Some(cache_service);
        self
    }

    /// Initialize Prometheus metrics
    pub fn with_prometheus(mut self) -> Self {
        match crate::monitoring::init_global_prometheus_metrics() {
            Ok(()) => {
                info!("Global Prometheus metrics initialized successfully");
                // Initialize rate limiter metrics
                crate::monitoring::init_rate_limiter_metrics();
                info!("Rate limiter metrics initialized successfully");
                // Set a flag to indicate Prometheus is available
                self.prometheus_metrics =
                    Some(Arc::new(crate::monitoring::PrometheusMetrics::default()));
            }
            Err(e) => {
                warn!("Failed to initialize global Prometheus metrics: {}. Continuing without Prometheus.", e);
            }
        }
        self
    }

    /// Initialize Redis services
    pub async fn with_redis(mut self) -> Self {
        match self.initialize_redis_services().await {
            Ok(services) => {
                info!("Successfully initialized Redis services");
                self.redis_services = Some(services);
            }
            Err(e) => {
                warn!(
                    "Failed to initialize Redis services: {}. Running without Redis integration.",
                    e
                );
            }
        }
        self
    }

    /// Build the final AppState
    pub async fn build(mut self) -> Result<AppState> {
        let database_pool = self
            .database_pool
            .take()
            .ok_or_else(|| anyhow::anyhow!("Database pool not initialized"))?;
        let cache_service = self
            .cache_service
            .take()
            .ok_or_else(|| anyhow::anyhow!("Cache service not initialized"))?;

        // Create provider service
        let provider_service = self.create_provider_service(database_pool, cache_service);

        // Start background services if Redis is available
        let background_task_manager = if let Some(ref redis_services) = self.redis_services {
            Some(
                self.start_background_services(
                    redis_services,
                    provider_service.frp_manager.clone(),
                )
                .await,
            )
        } else {
            None
        };

        Ok(AppState {
            provider_service,
            background_task_manager,
            prometheus_metrics: self.prometheus_metrics.take(),
            config: self.config,
        })
    }

    fn initialize_frp_manager(
        &self,
        provider_service: &ProviderService,
    ) -> Result<Option<FrpManager>> {
        // Check if FRP is enabled in configuration
        if !self.config.frp.frp_enabled {
            return Ok(None);
        }

        Ok(Some(block_on(FrpManager::new(
            self.config.frp.url.clone(),
            self.config.frp.passwd.clone(),
            self.config.frp.conns_limit,
            Arc::clone(provider_service.get_cache()),
        ))))
    }

    async fn initialize_redis_services(&self) -> Result<RedisServices> {
        // Create Redis service
        let redis_service = Arc::new(RedisService::new(&self.config.redis).await?);
        redis_service.health_check().await?;
        info!("Redis connection established successfully");

        // Create metrics reporter
        let mut metrics_reporter = MetricsReporter::new(
            Arc::clone(&redis_service),
            &self.config.redis,
            self.gateway_id.clone(),
        );

        if let Some(prometheus) = &self.prometheus_metrics {
            metrics_reporter = metrics_reporter.with_prometheus_metrics(Arc::clone(prometheus));
        }
        let metrics_reporter = Arc::new(metrics_reporter);

        // Create health monitor
        let health_monitor = Arc::new(HealthMonitor::new(&self.config.redis));

        // Create supervisor manager
        let supervisor_config = SupervisorConfig::from_env();
        let supervisor_manager = SupervisorManager::new(
            self.gateway_id.clone(),
            Arc::clone(&redis_service),
            supervisor_config,
            &self.config.redis.url,
            &self.config.database.url,
        )
        .await?;
        let supervisor_manager = Arc::new(tokio::sync::Mutex::new(supervisor_manager));

        Ok(RedisServices {
            redis_service,
            metrics_reporter,
            health_monitor,
            supervisor_manager,
        })
    }

    fn create_provider_service(
        &self,
        pool: sqlx::PgPool,
        cache_service: Arc<CacheService>,
    ) -> Arc<ProviderService> {
        let mut provider_service = ProviderService::new(pool, cache_service);

        if let Some(ref redis_services) = self.redis_services {
            provider_service = provider_service
                .with_metrics_reporter(Arc::clone(&redis_services.metrics_reporter))
                .with_health_monitor(Arc::clone(&redis_services.health_monitor))
                .with_redis_service(Arc::clone(&redis_services.redis_service));
        }

        // Add FRP manager if enabled; handle possible initialization error and continue without FRP on failure
        match self.initialize_frp_manager(&provider_service) {
            Ok(Some(frp_manager)) => {
                provider_service = provider_service.with_frp_manager(Arc::new(frp_manager));
            }
            Ok(None) => {}
            Err(e) => {
                warn!(
                    "Failed to initialize FRP manager: {}. Continuing without FRP.",
                    e
                );
            }
        }

        // Add routing configuration
        provider_service = provider_service.with_routing_config(self.config.routing.clone());

        Arc::new(provider_service)
    }

    async fn start_background_services(
        &self,
        redis_services: &RedisServices,
        frp_manager: Option<Arc<FrpManager>>,
    ) -> Arc<tokio::sync::Mutex<BackgroundTaskManager>> {
        let mut task_manager = BackgroundTaskManager::new();

        task_manager
            .start_all_tasks(
                Some(Arc::clone(&redis_services.metrics_reporter)),
                frp_manager,
            )
            .await;

        // Start supervisor manager
        if let Err(e) = redis_services.supervisor_manager.lock().await.start().await {
            warn!("Failed to start supervisor manager: {}", e);
        } else {
            info!("Started supervisor manager for distributed supervision");
        }

        Arc::new(tokio::sync::Mutex::new(task_manager))
    }
}

/// Main application factory function - creates the entire app with simplified initialization
pub async fn create_app(config: Config) -> Result<Router> {
    let app_state = AppBuilder::new(config)
        .with_database()
        .await?
        .with_cache()
        .with_prometheus()
        .with_redis()
        .await
        .build()
        .await?;

    build_router(app_state)
}

/// Fallback handler for unmatched routes - logs request details for debugging
async fn fallback_handler(
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    // Log unmatched request for debugging 404 issues
    warn!(
        "Unmatched request: method={}, uri={}, content_type={:?}, user_agent={:?}",
        method,
        uri,
        headers
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("?")),
        headers.get("user-agent").map(|v| v.to_str().unwrap_or("?"))
    );

    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": {
                "message": format!("No route matched for {} {}", method, uri.path()),
                "type": "not_found"
            }
        })),
    )
}

/// Build and configure the Axum router with all routes and middleware
fn build_router(state: AppState) -> Result<Router> {
    // Clone state for middleware usage before consuming it
    let state_for_middleware = state.clone();

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/metrics", get(handlers::metrics))
        .route("/v1/chat/completions", post(handlers::chat::completions))
        .route("/v1/completions", post(handlers::chat::text_completions))
        // Weight management APIs
        .route(
            "/api/v1/weights/status",
            get(handlers::weights::get_weight_status),
        )
        .route(
            "/api/v1/weights/provider/{provider_id}",
            get(handlers::weights::get_provider_weight_status),
        )
        // Weight reset API
        .route(
            "/api/v1/weights/reset",
            post(handlers::weights::reset_weights),
        )
        // FRP nodes API
        .route("/api/v1/frp/nodes", get(handlers::frp_nodes));

    // Add profiling endpoints if feature is enabled
    #[cfg(feature = "profiling")]
    let app = {
        info!("⚡ Profiling endpoints enabled");
        app.route(
            "/debug/pprof/profile",
            get(handlers::profiling::cpu_profile),
        )
        .route(
            "/debug/pprof/flamegraph",
            get(handlers::profiling::flamegraph),
        )
        .route(
            "/debug/pprof/quick",
            get(handlers::profiling::quick_profile),
        )
    };

    #[cfg(not(feature = "profiling"))]
    {
        info!("ℹ️  Profiling endpoints disabled (compile with --features profiling to enable)");
    }

    // Add fallback handler to capture and log unmatched requests
    let app = app.fallback(fallback_handler);

    let app = app
        .with_state(state)
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        // Add trace context extraction middleware AFTER other layers
        // (which means it executes BEFORE them due to Axum's reverse layer order)
        .layer(middleware::from_fn(trace_context_middleware))
        // Add GrowthBook probabilistic refresh middleware
        .layer(middleware::from_fn_with_state(
            state_for_middleware,
            growthbook_refresh_middleware,
        ));

    Ok(app)
}
