use super::types::{ChatCompletionResponse, CompletionResponse};
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use futures_util::Stream;
use std::pin::Pin;
use tracing::error;

pub type StreamResponse = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;

/// Metadata to be returned as HTTP headers
#[derive(Debug, Clone, Default)]
pub struct ResponseMetadata {
    pub base_url: Option<String>,
    pub model_name: Option<String>,
}

impl ResponseMetadata {
    pub fn new(base_url: String, model_name: String) -> Self {
        Self {
            base_url: Some(base_url),
            model_name: Some(model_name),
        }
    }
}

pub enum ChatCompletionResult {
    Json(ChatCompletionResponse, ResponseMetadata),
    Stream(StreamResponse, ResponseMetadata),
}

pub enum CompletionResult {
    Json(CompletionResponse),
    Stream(StreamResponse),
}

impl IntoResponse for ChatCompletionResult {
    fn into_response(self) -> Response {
        match self {
            ChatCompletionResult::Json(response, metadata) => {
                let mut res = Json(response).into_response();

                // Add metadata headers
                if let Some(base_url) = metadata.base_url {
                    if let Ok(value) = base_url.parse() {
                        res.headers_mut().insert("X-Provider-Base-Url", value);
                    }
                }
                if let Some(model_name) = metadata.model_name {
                    if let Ok(value) = model_name.parse() {
                        res.headers_mut().insert("X-Model-Name", value);
                    }
                }

                res
            }
            ChatCompletionResult::Stream(stream, metadata) => {
                use axum::body::Body;
                use futures_util::StreamExt;
                use std::sync::atomic::{AtomicBool, Ordering};
                use std::sync::Arc;

                let first_chunk_sent = Arc::new(AtomicBool::new(false));
                let body_stream = stream.map(move |chunk_result| {
                    if let Ok(ref chunk) = chunk_result {
                        if !chunk.is_empty() && !first_chunk_sent.swap(true, Ordering::Relaxed) {
                            let _span = tracing::info_span!("first_chunk_sent", chunk_size = chunk.len()).entered();
                        }
                    }
                    if let Err(e) = &chunk_result {
                        error!("❌ Stream error: {} (kind: {:?}, is_timeout: {}, is_connect: {}, is_request: {})",
                            e,
                            e.status(),
                            e.is_timeout(),
                            e.is_connect(),
                            e.is_request()
                        );
                    }
                    chunk_result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                });

                let mut builder = Response::builder()
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-cache")
                    .header("connection", "keep-alive")
                    .header("access-control-allow-origin", "*");

                // Add metadata headers
                if let Some(base_url) = metadata.base_url {
                    builder = builder.header("X-Provider-Base-Url", base_url);
                }
                if let Some(model_name) = metadata.model_name {
                    builder = builder.header("X-Model-Name", model_name);
                }

                builder
                    .body(Body::from_stream(body_stream))
                    .unwrap_or_else(|e| {
                        error!("Failed to build streaming response: {}", e);
                        Response::builder()
                            .status(500)
                            .body(Body::from("Internal server error"))
                            .unwrap_or_default()
                    })
            }
        }
    }
}

impl IntoResponse for CompletionResult {
    fn into_response(self) -> Response {
        match self {
            CompletionResult::Json(response) => Json(response).into_response(),
            CompletionResult::Stream(stream) => {
                use axum::body::Body;
                use futures_util::StreamExt;

                let body_stream = stream.map(|chunk_result| {
                    chunk_result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                });

                Response::builder()
                    .header("content-type", "text/plain; charset=utf-8")
                    .header("cache-control", "no-cache")
                    .header("connection", "keep-alive")
                    .body(Body::from_stream(body_stream))
                    .unwrap_or_else(|e| {
                        error!("Failed to build streaming response: {}", e);
                        Response::builder()
                            .status(500)
                            .body(Body::from("Internal server error"))
                            .unwrap_or_default()
                    })
            }
        }
    }
}
