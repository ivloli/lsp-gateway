use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] anyhow::Error),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("No active providers found for group: {0}")]
    NoActiveProviders(String),

    #[error("All providers rate limited for group: {0}")]
    AllProvidersRateLimited(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Redis error: {0}")]
    Redis(String),

    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("Content blocked by safety filters: {0}")]
    ContentFilter(String),

    #[error("Empty response from provider")]
    EmptyResponse(String),

    #[error("Stream token receive timeout after {timeout_ms}ms")]
    StreamTokenTimeout { timeout_ms: u64 },

    #[error("Response finish timeout after {timeout_ms}ms")]
    ResponseFinishTimeout { timeout_ms: u64 },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::ModelNotFound(ref model_id) => {
                error!("Model not found or not live: {}", model_id);
                (StatusCode::NOT_FOUND, self.to_string())
            }
            AppError::NoActiveProviders(_) => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            AppError::AllProvidersRateLimited(_) => {
                (StatusCode::TOO_MANY_REQUESTS, self.to_string())
            }
            AppError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::Provider(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::ContentFilter(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::EmptyResponse(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::StreamTokenTimeout { .. } | AppError::ResponseFinishTimeout { .. } => {
                (StatusCode::REQUEST_TIMEOUT, self.to_string())
            }
            AppError::Database(db_error) => {
                error!("❌ Database error: {}", db_error);
                // Note: Removed {:?} debug output to prevent multiline logs
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            AppError::Internal(internal_error) => {
                error!("❌ Internal error: {}", internal_error);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            AppError::Redis(redis_error) => {
                error!("❌ Redis error: {}", redis_error);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            _ => {
                error!("❌ Unknown error: {}", self);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };

        let body = Json(json!({
            "error": {
                "message": error_message,
                "type": "api_error"
            }
        }));

        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
