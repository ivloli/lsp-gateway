//! Utility functions for chat handlers
//!
//! This module provides shared utilities including:
//! - Request ID extraction from headers
//! - Streaming metrics recording
//! - Error classification for metrics

use crate::app::AppState;
use crate::error::{AppError, Result};
use crate::routing::{ProviderService, SelectedProvider};
use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tracing::{debug, info, info_span, Span};
use uuid::Uuid;

/// Classify error type for metrics recording
///
/// This provides fine-grained error categorization for streaming requests,
/// enabling better observability and root cause analysis.
pub fn classify_streaming_error(error: &AppError) -> &'static str {
    match error {
        // Timeout errors - most common in streaming
        AppError::StreamTokenTimeout { .. } => "stream_token_timeout",
        AppError::ResponseFinishTimeout { .. } => "response_finish_timeout",

        // Network/HTTP errors
        AppError::HttpClient(e) => {
            if e.is_timeout() {
                "http_timeout"
            } else if e.is_connect() {
                "connection_error"
            } else if e.is_request() {
                "request_error"
            } else {
                "http_client_error"
            }
        }

        // Provider errors
        AppError::Provider(_) => "provider_error",
        AppError::EmptyResponse(_) => "empty_response",
        AppError::ContentFilter(_) => "content_filter",

        // Routing errors
        AppError::NoActiveProviders(_) => "no_active_providers",
        AppError::AllProvidersRateLimited(_) => "rate_limited",
        AppError::ModelNotFound(_) => "model_not_found",

        // Internal errors
        AppError::Serialization(_) => "serialization_error",
        AppError::Internal(_) => "internal_error",
        AppError::InvalidRequest(_) => "invalid_request",

        // Infrastructure errors
        AppError::Database(_) => "database_error",
        AppError::Redis(_) => "redis_error",
        AppError::Config(_) => "config_error",
    }
}

/// Extract request ID from traceparent header or generate new UUID
///
/// Traceparent format: version-trace_id-parent_id-trace_flags
/// Example: 00-3e78a79ecf20a3135c33df5af592aecd-464576e952f4d100-01
pub fn extract_request_id_from_headers(headers: &HeaderMap) -> String {
    if let Some(traceparent) = headers.get("traceparent") {
        if let Ok(traceparent_str) = traceparent.to_str() {
            let parts: Vec<&str> = traceparent_str.split('-').collect();
            if parts.len() >= 2 {
                // Extract the trace_id (second part) which is 32 hex characters
                return parts[1].to_string();
            }
        }
    }

    // Fallback to UUID if no valid traceparent found
    Uuid::new_v4().to_string()
}

/// Extract conversation ID from X-Conversation-Id header
/// Returns None if header is not present or invalid
pub fn extract_conversation_id_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(conversation_id) = headers.get("X-Conversation-Id") {
        if let Ok(conversation_id_str) = conversation_id.to_str() {
            debug!(
                "Extracted conversation ID from headers: {}",
                conversation_id_str
            );
            return Some(conversation_id_str.to_string());
        }
    } else {
        debug!("X-Conversation-Id header not found");
    }
    None
}

/// Metrics context for tracking streaming requests
struct StreamMetricsContext {
    start_time: Instant,
    provider_id: String,
    model_id: String,
    base_url: String,
    group_for_prometheus: String,
    model_provider_group: Option<String>,
    provider_service: Arc<ProviderService>,
    /// Whether metrics have been recorded (to prevent duplicate recording)
    recorded: bool,
    /// Error encountered during streaming (if any)
    last_error: Option<String>,
    stream_done: bool,
    /// Whether TTFT has been recorded
    ttft_recorded: bool,
    /// Whether TTFB to client has been recorded
    ttfb_to_client_recorded: bool,
}

/// A stream wrapper that records metrics when the stream completes
///
/// This ensures we capture the total duration of the streaming request,
/// not just the time to first token.
pub struct MetricsStream<S> {
    inner: S,
    ctx: StreamMetricsContext,
    /// Span for tracing stream consumption lifecycle
    span: Span,
}

impl<S> MetricsStream<S> {
    fn new(
        inner: S,
        provider_service: Arc<ProviderService>,
        provider: &SelectedProvider,
        model_id: &str,
        request_start_time: Instant, // 请求到达 LSP 的时间
    ) -> Self {
        let group_for_prometheus = provider
            .model_config
            .model_provider_group
            .clone()
            .unwrap_or_else(|| format!("direct-{}", provider.model_config.id));

        // Create span for tracing stream consumption lifecycle
        let span = info_span!(
            "stream_consumption",
            provider_id = %provider.id,
            model_id = %model_id,
            ttft_ms = tracing::field::Empty,
            total_duration_ms = tracing::field::Empty,
            success = tracing::field::Empty,
        );
        // Note: stream_mode is recorded in the parent llm_request span

        Self {
            inner,
            span,
            ctx: StreamMetricsContext {
                start_time: request_start_time, // 使用请求到达时间，而不是流建立时间
                provider_id: provider.id.clone(),
                model_id: model_id.to_string(),
                base_url: provider.base_url.clone(),
                group_for_prometheus,
                model_provider_group: provider.model_config.model_provider_group.clone(),
                provider_service,
                recorded: false,
                last_error: None,
                stream_done: false,
                ttft_recorded: false,
                ttfb_to_client_recorded: false,
            },
        }
    }
}

impl<S> Stream for MetricsStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = std::result::Result<Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Clone the span to avoid borrow conflicts with self
        let span = self.span.clone();
        let _guard = span.enter();

        let inner = Pin::new(&mut self.inner);
        match inner.poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                // Record TTFT on first chunk
                if !self.ctx.ttft_recorded {
                    self.ctx.ttft_recorded = true;
                    let ttft_ms = self.ctx.start_time.elapsed().as_millis() as u64;

                    // Record TTFT in span
                    span.record("ttft_ms", ttft_ms);

                    // Record TTFT in Prometheus
                    if let Some(metrics) =
                        crate::monitoring::prometheus::get_global_prometheus_metrics()
                    {
                        metrics.record_ttft(
                            &self.ctx.provider_id,
                            &self.ctx.model_id,
                            &self.ctx.group_for_prometheus,
                            ttft_ms as f64 / 1000.0,
                        );
                    }

                    info!(
                        "⚡ TTFT: provider={}, model={}, ttft={}ms",
                        self.ctx.provider_id, self.ctx.model_id, ttft_ms
                    );
                }

                if !self.ctx.stream_done {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        if text.contains("data: [DONE]") {
                            self.ctx.stream_done = true;
                            info!(
                                "Logical stream end ([DONE] received): provider={}, model={}",
                                self.ctx.provider_id, self.ctx.model_id
                            );
                        }
                    }
                }

                // Record TTFB to client on first chunk (just before returning to client)
                if !self.ctx.ttfb_to_client_recorded {
                    self.ctx.ttfb_to_client_recorded = true;
                    let ttfb_to_client_ms = self.ctx.start_time.elapsed().as_millis() as u64;

                    // Record TTFB to client in Prometheus
                    if let Some(metrics) =
                        crate::monitoring::prometheus::get_global_prometheus_metrics()
                    {
                        metrics.record_ttfb_to_client(
                            &self.ctx.provider_id,
                            &self.ctx.model_id,
                            &self.ctx.group_for_prometheus,
                            ttfb_to_client_ms as f64 / 1000.0,
                        );
                    }

                    debug!(
                        "📤 TTFB to client: provider={}, model={}, ttfb={}ms",
                        self.ctx.provider_id, self.ctx.model_id, ttfb_to_client_ms
                    );
                }

                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(e))) => {
                // Record error for metrics
                self.ctx.last_error = Some(format!("{}", e));
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                // Stream completed - record metrics
                info!(
                    "Physical stream end (Poll::Ready(None)): provider={}, model={}",
                    self.ctx.provider_id, self.ctx.model_id
                );
                self.record_completion_metrics();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> MetricsStream<S> {
    fn record_completion_metrics(&mut self) {
        if self.ctx.recorded {
            return; // Already recorded
        }
        self.ctx.recorded = true;

        let latency_ms = self.ctx.start_time.elapsed().as_millis() as u64;
        let success = self.ctx.last_error.is_none();

        // Record span fields for tracing
        self.span.record("total_duration_ms", latency_ms);
        self.span.record("success", success);

        if let Some(ref error_msg) = self.ctx.last_error {
            // Record failure
            let error_type = classify_streaming_error_str(error_msg);

            // Record in provider service (Redis metrics) - spawn only if runtime is available
            // This prevents panic when Drop is called outside of tokio runtime context
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let provider_service = self.ctx.provider_service.clone();
                let provider_id = self.ctx.provider_id.clone();
                let model_provider_group = self.ctx.model_provider_group.clone();
                handle.spawn(async move {
                    provider_service
                        .record_request_completion(
                            &provider_id,
                            false,
                            latency_ms,
                            model_provider_group,
                        )
                        .await;
                });
            }

            // Record in Prometheus (latency is recorded in request_duration_seconds histogram)
            crate::monitoring::record_failure_global(
                &self.ctx.provider_id,
                &self.ctx.model_id,
                &self.ctx.base_url,
                &self.ctx.group_for_prometheus,
                error_type,
                latency_ms,
            );

            info!(
                "📊 Stream completed with error: provider={}, model={}, error_type={}, total_duration={}ms",
                self.ctx.provider_id, self.ctx.model_id, error_type, latency_ms
            );
        } else {
            // Record success
            // Record in provider service (Redis metrics) - spawn only if runtime is available
            // This prevents panic when Drop is called outside of tokio runtime context
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let provider_service = self.ctx.provider_service.clone();
                let provider_id = self.ctx.provider_id.clone();
                let model_provider_group = self.ctx.model_provider_group.clone();
                handle.spawn(async move {
                    provider_service
                        .record_request_completion(
                            &provider_id,
                            true,
                            latency_ms,
                            model_provider_group,
                        )
                        .await;
                });
            }

            // Record in Prometheus (latency is recorded in request_duration_seconds histogram)
            crate::monitoring::record_success_global(
                &self.ctx.provider_id,
                &self.ctx.model_id,
                &self.ctx.base_url,
                &self.ctx.group_for_prometheus,
                latency_ms,
                None, // Token count not tracked in streaming
            );

            info!(
                "📊 Stream completed successfully: provider={}, model={}, total_duration={}ms",
                self.ctx.provider_id, self.ctx.model_id, latency_ms
            );
        }
    }
}

impl<S> Drop for MetricsStream<S> {
    fn drop(&mut self) {
        // Ensure metrics are recorded even if stream is dropped early
        if !self.ctx.recorded {
            if self.ctx.stream_done {
                debug!(
                    "Stream dropped after [DONE]: provider={}, model={} - recording as success",
                    self.ctx.provider_id, self.ctx.model_id
                );
            } else {
                self.ctx.last_error = Some("stream_dropped_early".to_string());
            }
            self.record_completion_metrics();
        }
    }
}

/// Classify error type from error message string
fn classify_streaming_error_str(error_msg: &str) -> &'static str {
    if error_msg.contains("timeout") {
        "http_timeout"
    } else if error_msg.contains("connection") {
        "connection_error"
    } else if error_msg.contains("dropped_early") {
        "client_disconnect"
    } else {
        "stream_error"
    }
}

/// Record metrics for streaming requests
///
/// This function wraps the stream with a MetricsStream that records
/// the complete duration when the stream finishes (not just first token time).
///
/// The duration is measured from when the request arrives at LSP (request_arrival_time)
/// to when the stream completes.
pub async fn record_streaming_metrics<F, Fut>(
    state: &AppState,
    provider: &SelectedProvider,
    model_id: &str,
    request_arrival_time: Instant,
    handler: F,
) -> Result<crate::handlers::chat::response::StreamResponse>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<
        Output = Result<(crate::handlers::chat::response::StreamResponse, Option<u64>)>,
    >,
{
    // Execute handler to get the stream
    match handler().await {
        Ok((stream, _token_count)) => {
            // Wrap stream with MetricsStream to record total duration at completion
            // 传入请求到达时间，确保记录的是从请求到达到流结束的完整时间
            let metrics_stream = MetricsStream::new(
                stream,
                state.provider_service.clone(),
                provider,
                model_id,
                request_arrival_time,
            );

            Ok(Box::pin(metrics_stream))
        }
        Err(e) => {
            // Record immediate failure (stream creation failed)
            let latency_ms = 0u64; // Immediate failure
            let error_type = classify_streaming_error(&e);

            let group_for_prometheus = provider
                .model_config
                .model_provider_group
                .clone()
                .unwrap_or_else(|| format!("direct-{}", provider.model_config.id));

            // Record in provider service (Redis metrics)
            state
                .provider_service
                .record_request_completion(
                    &provider.id,
                    false,
                    latency_ms,
                    provider.model_config.model_provider_group.clone(),
                )
                .await;

            // Record in Prometheus with specific error type
            crate::monitoring::record_failure_global(
                &provider.id,
                model_id,
                &provider.base_url,
                &group_for_prometheus,
                error_type,
                latency_ms,
            );

            tracing::warn!(
                "⚠️ Stream creation failed: provider={}, model={}, base_url={}, group={}, error_type={}, latency={}ms, error={}",
                provider.id, model_id, provider.base_url, group_for_prometheus, error_type, latency_ms, e
            );

            Err(e)
        }
    }
}
