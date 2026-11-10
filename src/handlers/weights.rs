use crate::app::AppState;
use crate::error::{AppError, Result};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Weight status for a single provider
#[derive(Debug, Serialize)]
pub struct WeightStatus {
    pub provider_id: String,
    pub group: String,
    pub original_weight: i32,           // 数据库原始权重
    pub current_weight: i32,            // 当前实际权重
    pub supervisor_weight: Option<i32>, // Supervisor计算权重
    pub adjustment_reason: Option<String>,
    pub last_updated: Option<u64>,
}

/// Response for weight status queries
#[derive(Debug, Serialize)]
pub struct WeightStatusResponse {
    pub providers: Vec<WeightStatus>,
    pub total_count: usize,
    pub group_filter: Option<String>,
}

/// Query parameters for weight status
#[derive(Debug, Deserialize)]
pub struct WeightStatusQuery {
    pub group: Option<String>,
}

/// Auto adjustment configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct AutoAdjustmentConfig {
    pub enabled: bool,
}

/// Request to update auto adjustment settings
#[derive(Debug, Deserialize)]
pub struct UpdateAutoAdjustmentRequest {
    pub enabled: Option<bool>,
}

/// Request to reset weights
#[derive(Debug, Deserialize)]
pub struct ResetWeightsRequest {
    pub group: Option<String>,
}

/// Response for weight reset operation
#[derive(Debug, Serialize)]
pub struct ResetWeightsResponse {
    pub success: bool,
    pub message: String,
    pub reset_count: usize,
    pub group_filter: Option<String>,
}

/// Get weight status for all providers or filtered by group
pub async fn get_weight_status(
    State(state): State<AppState>,
    Query(query): Query<WeightStatusQuery>,
) -> Result<Json<WeightStatusResponse>> {
    info!("Getting weight status, group filter: {:?}", query.group);

    let weight_statuses = collect_weight_status(&state, query.group.as_deref()).await?;

    let response = WeightStatusResponse {
        total_count: weight_statuses.len(),
        group_filter: query.group,
        providers: weight_statuses,
    };

    Ok(Json(response))
}

/// Get weight status for a specific provider by ID
pub async fn get_provider_weight_status(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<WeightStatus>> {
    info!("Getting weight status for provider: {}", provider_id);

    // First get the provider from database to find its group
    let provider = crate::models::ModelProvider::find_by_id(
        &state.provider_service.get_db_pool(),
        &provider_id,
    )
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::InvalidRequest(format!("Provider {} not found", provider_id)))?;

    // Then get weight status for that specific group and provider
    let weight_statuses = collect_weight_status(&state, Some(&provider.group_name)).await?;

    let provider_status = weight_statuses
        .into_iter()
        .find(|status| status.provider_id == provider_id)
        .ok_or_else(|| {
            AppError::InvalidRequest(format!(
                "Provider {} not found in group {}",
                provider_id, provider.group_name
            ))
        })?;

    Ok(Json(provider_status))
}

/// Collect weight status from database and Redis
async fn collect_weight_status(
    state: &AppState,
    group_filter: Option<&str>,
) -> Result<Vec<WeightStatus>> {
    // Get providers from database (original weights)
    let providers = if let Some(group) = group_filter {
        crate::models::ModelProvider::find_active_by_group(
            &state.provider_service.get_db_pool(),
            group,
        )
        .await
        .map_err(AppError::Database)?
    } else {
        return Err(AppError::InvalidRequest(
            "Group parameter is required. Please specify a group.".to_string(),
        ));
    };

    let mut weight_statuses = Vec::new();

    for provider in providers {
        // Get health info from Redis (includes supervisor_weight)
        let health_info = if let Some(redis_service) = state.provider_service.get_redis_service() {
            redis_service
                .get_provider_health(&provider.id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        let current_weight = health_info
            .as_ref()
            .and_then(|h| h.supervisor_weight)
            .unwrap_or(provider.weight);

        let weight_status = WeightStatus {
            provider_id: provider.id,
            group: provider.group_name,
            original_weight: provider.weight,
            current_weight,
            supervisor_weight: health_info.as_ref().and_then(|h| h.supervisor_weight),
            adjustment_reason: health_info
                .as_ref()
                .and_then(|h| h.weight_update_reason.clone()),
            last_updated: health_info.as_ref().and_then(|h| h.weight_updated_at),
        };

        weight_statuses.push(weight_status);
    }

    debug!("Collected {} weight statuses", weight_statuses.len());
    Ok(weight_statuses)
}

/// Reset provider weights to database original values
pub async fn reset_weights(
    State(state): State<AppState>,
    Json(request): Json<ResetWeightsRequest>,
) -> Result<Json<ResetWeightsResponse>> {
    info!("Resetting weights, group filter: {:?}", request.group);

    // Get providers from database
    let providers = if let Some(group) = &request.group {
        crate::models::ModelProvider::find_active_by_group(
            &state.provider_service.get_db_pool(),
            group,
        )
        .await
        .map_err(AppError::Database)?
    } else {
        return Err(AppError::InvalidRequest(
            "Group parameter is required for weight reset. Please specify a group.".to_string(),
        ));
    };

    let mut reset_count = 0;

    // Reset weights in Redis by removing supervisor_weight entries
    if let Some(redis_service) = state.provider_service.get_redis_service() {
        for provider in &providers {
            let key = format!("lsp:routing:provider:{}", provider.id);

            // Remove supervisor weight fields to reset to original
            let fields_to_remove = vec![
                "supervisor_weight",
                "weight_update_reason",
                "weight_updated_at",
                "weight_version",
            ];

            for field in fields_to_remove {
                let _ = redis_service.hdel(&key, field).await;
            }

            reset_count += 1;
            debug!(
                "Reset weight for provider {} to original value {}",
                provider.id, provider.weight
            );

            // Update dynamic weights in Prometheus (reset supervisor_weight to None)
            crate::monitoring::update_dynamic_weights_global(
                &provider.id,
                &provider.group_name,
                provider.weight,
                None,
            );
        }
    }

    let response = ResetWeightsResponse {
        success: true,
        message: format!(
            "Successfully reset {} provider weights to database original values",
            reset_count
        ),
        reset_count,
        group_filter: request.group,
    };

    info!("Reset weights completed: {} providers", reset_count);
    Ok(Json(response))
}
