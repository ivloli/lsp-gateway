//! Middleware for GrowthBook probabilistic feature refresh

use crate::app::AppState;
use axum::{extract::State, http::Request, middleware::Next, response::Response};

/// Middleware that probabilistically triggers GrowthBook feature refresh
/// Based on the configured probability (1/N chance per request)
pub async fn growthbook_refresh_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Trigger probabilistic refresh if enabled
    if state.config.growthbook.enabled {
        crate::client::growthbook::maybe_refresh_features(
            state.config.growthbook.refresh_probability,
        );
    }

    // Continue processing the request
    next.run(request).await
}
