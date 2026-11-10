use lsp_gateway::app::create_app;
use lsp_gateway::config::Config;
use lsp_gateway::error::Result;
use lsp_gateway::logging_format::LspGatewayFormat;
use std::net::SocketAddr;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file first, before initializing tracing
    dotenvy::dotenv().ok();

    // Load configuration first (needed for tracing setup)
    let config = Config::from_env()?;

    // Create base tracing layers
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "lsp_gateway=info,tower_http=info,h2=warn".into());

    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(LspGatewayFormat)
        .with_ansi(true)
        .fmt_fields(tracing_subscriber::fmt::format::DefaultFields::new());

    // Initialize OpenTelemetry layer if tempo is enabled
    let telemetry_layer = lsp_gateway::tracing_setup::init_tracer(&config.tracing);

    // Build subscriber with conditional OpenTelemetry layer
    // Use Option to make the layer type uniform
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(telemetry_layer)
        .init();

    if config.tracing.tracing_enabled {
        info!("Tracing initialized with OpenTelemetry support");
    } else {
        info!("Tracing initialized without OpenTelemetry");
    }

    // Initialize Growthbook client
    lsp_gateway::client::growthbook::init_growthbook_client(config.growthbook.clone());
    if config.growthbook.enabled {
        info!("Growthbook client initialized");
        // Try to load features in background (don't block startup)
        if let Some(gb_client) = lsp_gateway::client::growthbook::get_growthbook_client() {
            let gb_clone = gb_client.clone();
            tokio::spawn(async move {
                match gb_clone.load_features().await {
                    Ok(_) => info!("Growthbook features loaded successfully"),
                    Err(e) => warn!(
                        "Failed to load Growthbook features: {}, will use defaults",
                        e
                    ),
                }
            });

            // Start periodic refresh task
            let gb_periodic = gb_client.clone();
            let refresh_interval = config.growthbook.refresh_interval_secs;
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_secs(refresh_interval));
                loop {
                    interval.tick().await;
                    match gb_periodic.load_features().await {
                        Ok(_) => info!("Growthbook features refreshed successfully"),
                        Err(e) => warn!("Failed to refresh Growthbook features: {}", e),
                    }
                }
            });
            info!(
                "Growthbook periodic refresh task started (interval: {}s)",
                refresh_interval
            );
        }
    } else {
        info!("Growthbook is disabled");
    }

    let bind_address = config.server.bind_address.clone();
    let port = config.server.port;
    info!("Starting LSP Gateway on {}:{}", bind_address, port);

    // Create the application
    let app = create_app(config).await?;

    // Start the server
    let addr: SocketAddr = format!("{}:{}", bind_address, port)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bind address {}:{}: {}", bind_address, port, e))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", addr, e))?;

    info!("LSP Gateway listening on {}", addr);

    // Create a graceful shutdown signal handler
    let shutdown_signal = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {
                info!("Received Ctrl+C signal, initiating graceful shutdown...");
            },
            _ = terminate => {
                info!("Received SIGTERM signal, initiating graceful shutdown...");
            },
        }
    };

    // Run server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

    // Shutdown tracer before exit to ensure all pending spans are exported
    info!("Shutting down OpenTelemetry tracer...");
    lsp_gateway::tracing_setup::shutdown_tracer();

    // Give the tracer a moment to flush remaining spans
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    info!("LSP Gateway shutdown complete");

    Ok(())
}
