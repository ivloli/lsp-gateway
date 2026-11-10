pub mod chat;
pub mod profiling;
pub mod weights;

use crate::app::AppState;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tracing::error;

/// Health check endpoint
pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "lsp-gateway",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Prometheus metrics endpoint
pub async fn metrics(_state: State<AppState>) -> Result<Response<String>, StatusCode> {
    let metrics_data = crate::monitoring::render_metrics_global();
    if metrics_data.is_empty() {
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::CONTENT_TYPE, "text/plain")
            .body("Prometheus metrics not available".to_string())
            .map_err(|e| {
                error!("Failed to build metrics response: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )
            .body(metrics_data)
            .map_err(|e| {
                error!("Failed to build metrics response: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })
    }
}

pub async fn frp_nodes(
    axum::extract::State(state): axum::extract::State<crate::app::AppState>,
) -> Response {
    // Check if FRP manager is available
    if let Some(ref frp_manager) = state.provider_service.frp_manager {
        match frp_manager.get_all_nodes_as_json().await {
            Ok(json_str) => {
                // Return JSON response with proper content type
                ([("content-type", "application/json")], json_str).into_response()
            }
            Err(e) => {
                error!("Failed to get FRP nodes as JSON: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "Failed to retrieve FRP nodes",
                        "message": e.to_string()
                    })),
                )
                    .into_response()
            }
        }
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "FRP manager not available"
            })),
        )
            .into_response()
    }
}
