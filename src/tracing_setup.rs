//! OpenTelemetry tracing setup for Grafana Tempo integration
//!
//! This module provides initialization functions for distributed tracing
//! using OpenTelemetry and OTLP exporter to send traces to Grafana Tempo.

use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{Sampler, TracerProvider},
    Resource,
};

use crate::config::TracingConfig;

/// Initialize OpenTelemetry tracer and return the tracing layer
///
/// This function sets up the OTLP exporter to send traces to Grafana Tempo.
/// If initialization fails or tempo is disabled, returns None,
/// allowing the application to continue without distributed tracing.
pub fn init_tracer<S>(
    config: &TracingConfig,
) -> Option<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    // Set the text map propagator for trace context propagation
    global::set_text_map_propagator(TraceContextPropagator::new());

    if !config.tracing_enabled {
        return None;
    }

    // Prefer explicit OTEL_EXPORTER_OTLP_ENDPOINT if provided (for compatibility with
    // standard OpenTelemetry envs), otherwise fall back to our config value which is
    // driven by OTLP_TRACES_URL.
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .unwrap_or_else(|| config.tempo_endpoint.clone());

    // Create resource with service information.
    // We construct it directly here so our configured service.name / service.version
    // are guaranteed to be used, instead of relying on default resource merging rules.
    let resource = Resource::new(vec![
        KeyValue::new("service.name", config.service_name.clone()),
        KeyValue::new("service.version", config.service_version.clone()),
    ]);

    let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
        .unwrap_or_else(|_| "http/protobuf".to_string());

    let export_timeout = std::time::Duration::from_secs(30);

    let base_exporter = if protocol == "grpc" {
        let grpc_endpoint = endpoint.replace(":4318", ":4317");

        match opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&grpc_endpoint)
            .with_timeout(export_timeout)
            .build()
        {
            Ok(exp) => exp,
            Err(_e) => {
                return None;
            }
        }
    } else {
        let http_endpoint = if endpoint.ends_with("/v1/traces") {
            endpoint.clone()
        } else if endpoint.ends_with("/") {
            format!("{}v1/traces", endpoint)
        } else {
            format!("{}/v1/traces", endpoint)
        };

        match opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&http_endpoint)
            .with_timeout(export_timeout)
            .build()
        {
            Ok(exp) => exp,
            Err(_e) => {
                return None;
            }
        }
    };

    // Configure sampler with ParentBased to respect upstream sampling decisions
    let root_sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };
    let sampler = Sampler::ParentBased(Box::new(root_sampler));

    // Configure batch span processor
    use opentelemetry_sdk::trace::BatchConfigBuilder;
    let batch_config = BatchConfigBuilder::default()
        .with_max_queue_size(4096)
        .with_max_export_batch_size(512)
        .with_max_export_timeout(std::time::Duration::from_secs(30))
        .with_scheduled_delay(std::time::Duration::from_secs(5))
        .with_max_concurrent_exports(1)
        .build();

    use opentelemetry_sdk::trace::BatchSpanProcessor;
    let batch_processor =
        BatchSpanProcessor::builder(base_exporter, opentelemetry_sdk::runtime::Tokio)
            .with_batch_config(batch_config)
            .build();

    let tracer_provider = TracerProvider::builder()
        .with_resource(resource)
        .with_sampler(sampler)
        .with_span_processor(batch_processor)
        .build();

    global::set_tracer_provider(tracer_provider.clone());

    Some(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("lsp-gateway")))
}

/// Shutdown the global tracer provider gracefully
pub fn shutdown_tracer() {
    global::shutdown_tracer_provider();
}
