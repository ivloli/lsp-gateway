// ! OpenTelemetry trace context propagation middleware
//!
//! This middleware extracts trace context from incoming HTTP headers (traceparent/tracestate)
//! and sets it as the parent context for all spans created within this request.

use axum::{extract::Request, middleware::Next, response::Response};
use opentelemetry::global;
use opentelemetry_http::HeaderExtractor;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Middleware to extract and propagate OpenTelemetry trace context from HTTP headers
///
/// This extracts the W3C Trace Context from `traceparent` and `tracestate` headers
/// and creates a root span with the extracted context as parent.
///
/// This middleware MUST be applied as the outermost layer (added last in the layer stack)
/// to ensure the trace context is available for all inner layers and handlers.
pub async fn trace_context_middleware(request: Request, next: Next) -> Response {
    // Extract trace context from headers
    let headers = request.headers();

    let extractor = HeaderExtractor(headers);
    let parent_context =
        global::get_text_map_propagator(|propagator| propagator.extract(&extractor));

    // Extract request_id from traceparent header or generate new UUID
    let request_id = extract_request_id_from_headers(headers);

    // Create a root span for this request and set the extracted context as its parent
    // This span will be the parent of all spans created by TraceLayer and handlers
    let span = tracing::info_span!(
        "http_request_root",
        method = %request.method(),
        uri = %request.uri(),
        request_id = %request_id,
    );

    // Set the extracted OpenTelemetry context as the parent
    span.set_parent(parent_context.clone());

    // Use Instrument trait to properly attach the span to the async execution
    // This ensures the span context is available throughout the request lifecycle
    next.run(request).instrument(span).await
}

/// Extract request ID from traceparent header or generate new UUID
///
/// Traceparent format: version-trace_id-parent_id-trace_flags
/// Example: 00-3e78a79ecf20a3135c33df5af592aecd-464576e952f4d100-01
fn extract_request_id_from_headers(headers: &axum::http::HeaderMap) -> String {
    if let Some(traceparent) = headers.get("traceparent") {
        if let Ok(traceparent_str) = traceparent.to_str() {
            let parts: Vec<&str> = traceparent_str.split('-').collect();
            if parts.len() >= 2 {
                // Extract the trace_id (second part) which is 32 hex characters
                return parts[1].to_string();
            }
        }
    }

    // Generate new UUID if no traceparent header
    uuid::Uuid::new_v4().to_string().replace("-", "")
}
