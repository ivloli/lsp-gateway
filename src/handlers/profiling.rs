//! Performance profiling endpoints
//!
//! This module provides endpoints for CPU profiling and flamegraph generation.
//! These endpoints should only be enabled in development or staging environments,
//! or behind proper authentication in production.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[cfg(feature = "profiling")]
use tracing::{error, info, warn};

#[cfg(feature = "profiling")]
use axum::{extract::Query, http::header};

#[cfg(feature = "profiling")]
use pprof::protos::Message;

#[cfg(feature = "profiling")]
use serde::Deserialize;

/// Query parameters for profiling endpoints
#[cfg(feature = "profiling")]
#[derive(Deserialize)]
pub struct ProfileQuery {
    /// Duration in seconds (default: 30, min: 1, max: 300)
    #[serde(default = "default_seconds")]
    pub seconds: u64,
}

#[cfg(feature = "profiling")]
fn default_seconds() -> u64 {
    30
}

#[cfg(feature = "profiling")]
impl ProfileQuery {
    /// Get validated duration in seconds (clamped between 1 and 300)
    fn duration_secs(&self) -> u64 {
        self.seconds.clamp(1, 300)
    }
}

/// CPU profiling endpoint
///
/// Generates a pprof-compatible CPU profile by sampling.
/// The profile can be analyzed using tools like `go tool pprof` or converted to flamegraph.
///
/// # Query Parameters
/// - `seconds`: Duration in seconds (default: 30, min: 1, max: 300)
///
/// # Example
/// ```bash
/// # Download the profile (default 30 seconds)
/// curl http://localhost:8080/debug/pprof/profile > cpu.pb.gz
///
/// # Custom duration (60 seconds)
/// curl "http://localhost:8080/debug/pprof/profile?seconds=60" > cpu.pb.gz
///
/// # Analyze with pprof (requires Go tools)
/// go tool pprof -http=:8080 cpu.pb.gz
/// ```
#[cfg(feature = "profiling")]
pub async fn cpu_profile(Query(query): Query<ProfileQuery>) -> Response {
    let duration_secs = query.duration_secs();
    info!("Starting CPU profiling for {} seconds...", duration_secs);

    let guard = match pprof::ProfilerGuard::new(100) {
        Ok(guard) => guard,
        Err(e) => {
            error!("Failed to create profiler: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create profiler: {}", e),
            )
                .into_response();
        }
    };

    // Profile for specified duration
    tokio::time::sleep(tokio::time::Duration::from_secs(duration_secs)).await;

    info!("CPU profiling complete, generating report...");

    match guard.report().build() {
        Ok(report) => match report.pprof() {
            Ok(profile) => {
                let mut body = Vec::new();
                if let Err(e) = profile.write_to_vec(&mut body) {
                    error!("Failed to serialize profile: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to serialize profile: {}", e),
                    )
                        .into_response();
                }

                info!("CPU profile generated successfully ({} bytes)", body.len());

                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=\"cpu.pb.gz\"",
                    )
                    .body(axum::body::Body::from(body))
                    .unwrap()
                    .into_response()
            }
            Err(e) => {
                error!("Failed to build pprof profile: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to build profile: {}", e),
                )
                    .into_response()
            }
        },
        Err(e) => {
            error!("Failed to create report: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create report: {}", e),
            )
                .into_response()
        }
    }
}

/// Flamegraph generation endpoint
///
/// Generates an interactive SVG flamegraph by sampling CPU.
/// The flamegraph can be viewed directly in a browser.
///
/// # Query Parameters
/// - `seconds`: Duration in seconds (default: 30, min: 1, max: 300)
///
/// # Example
/// ```bash
/// # Download and view the flamegraph (default 30 seconds)
/// curl http://localhost:8080/debug/pprof/flamegraph > flamegraph.svg
///
/// # Custom duration (60 seconds)
/// curl "http://localhost:8080/debug/pprof/flamegraph?seconds=60" > flamegraph.svg
///
/// open flamegraph.svg  # macOS
/// # or
/// xdg-open flamegraph.svg  # Linux
/// ```
#[cfg(feature = "profiling")]
pub async fn flamegraph(Query(query): Query<ProfileQuery>) -> Response {
    let duration_secs = query.duration_secs();
    info!(
        "Starting CPU profiling for flamegraph ({} seconds)...",
        duration_secs
    );

    let guard = match pprof::ProfilerGuard::new(100) {
        Ok(guard) => guard,
        Err(e) => {
            error!("Failed to create profiler: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create profiler: {}", e),
            )
                .into_response();
        }
    };

    // Profile for specified duration
    tokio::time::sleep(tokio::time::Duration::from_secs(duration_secs)).await;

    info!("CPU profiling complete, generating flamegraph...");

    match guard.report().build() {
        Ok(report) => {
            let mut body = Vec::new();
            if let Err(e) = report.flamegraph(&mut body) {
                error!("Failed to generate flamegraph: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to generate flamegraph: {}", e),
                )
                    .into_response();
            }

            info!("Flamegraph generated successfully ({} bytes)", body.len());

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/svg+xml")
                .header(
                    header::CONTENT_DISPOSITION,
                    "inline; filename=\"flamegraph.svg\"",
                )
                .body(axum::body::Body::from(body))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            error!("Failed to create report: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create report: {}", e),
            )
                .into_response()
        }
    }
}

/// Quick profiling endpoint (10 seconds)
///
/// A faster version of the profiling endpoint for quick performance checks.
/// Samples for only 10 seconds instead of 30.
#[cfg(feature = "profiling")]
pub async fn quick_profile() -> Response {
    info!("Starting quick CPU profiling (10 seconds)...");

    let guard = match pprof::ProfilerGuard::new(100) {
        Ok(guard) => guard,
        Err(e) => {
            error!("Failed to create profiler: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create profiler: {}", e),
            )
                .into_response();
        }
    };

    // Profile for 10 seconds
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    info!("Quick profiling complete, generating flamegraph...");

    // Use catch_unwind to handle potential panics in pprof
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| guard.report().build()));

    match result {
        Ok(Ok(report)) => {
            let mut body = Vec::new();

            // Also catch potential panics in flamegraph generation
            let flamegraph_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                report.flamegraph(&mut body)
            }));

            match flamegraph_result {
                Ok(Ok(())) => {
                    if body.is_empty() {
                        warn!("Flamegraph is empty - this may happen when there's no CPU activity during sampling");
                        return (
                            StatusCode::OK,
                            "<!-- No CPU activity detected during profiling. Try running some load on the server. -->\n\
                             <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"200\">\n\
                             <text x=\"50\" y=\"100\" font-size=\"16\" fill=\"#666\">No CPU activity detected. Try profiling under load.</text>\n\
                             </svg>"
                        ).into_response();
                    }

                    info!(
                        "Quick flamegraph generated successfully ({} bytes)",
                        body.len()
                    );

                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "image/svg+xml")
                        .header(
                            header::CONTENT_DISPOSITION,
                            "inline; filename=\"flamegraph-quick.svg\"",
                        )
                        .body(axum::body::Body::from(body))
                        .unwrap()
                        .into_response()
                }
                Ok(Err(e)) => {
                    error!("Failed to generate flamegraph: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to generate flamegraph: {}", e),
                    )
                        .into_response()
                }
                Err(_) => {
                    error!("Flamegraph generation panicked - this is a known issue with pprof on some platforms");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Flamegraph generation failed due to a library panic. This can happen when there's insufficient CPU activity during sampling. Try profiling under load."
                    ).into_response()
                }
            }
        }
        Ok(Err(e)) => {
            error!("Failed to create report: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create report: {}", e),
            )
                .into_response()
        }
        Err(_) => {
            error!("Report building panicked - this is a known issue with pprof on some platforms");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Profile report generation failed due to a library panic. This can happen when there's insufficient CPU activity during sampling. Try profiling under load."
            ).into_response()
        }
    }
}

/// Fallback handler when profiling feature is not enabled
#[cfg(not(feature = "profiling"))]
pub async fn cpu_profile() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Profiling feature not enabled. Compile with --features profiling to enable.",
    )
        .into_response()
}

/// Fallback handler when profiling feature is not enabled
#[cfg(not(feature = "profiling"))]
pub async fn flamegraph() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Profiling feature not enabled. Compile with --features profiling to enable.",
    )
        .into_response()
}

/// Fallback handler when profiling feature is not enabled
#[cfg(not(feature = "profiling"))]
pub async fn quick_profile() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Profiling feature not enabled. Compile with --features profiling to enable.",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_profiling_endpoints_exist() {
        // Just test that the endpoints can be called
        // Actual profiling would take too long for unit tests

        #[cfg(not(feature = "profiling"))]
        {
            let response = cpu_profile().await;
            // Should return NOT_IMPLEMENTED when feature is disabled
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        }
    }
}
