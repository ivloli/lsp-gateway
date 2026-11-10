//! HTTP endpoint handlers for chat and text completions
//!
//! This module contains the main HTTP endpoint handlers that process incoming
//! completion requests, route them to the appropriate backend, and return responses.

use crate::app::AppState;
use crate::error::{AppError, Result};
use crate::handlers::chat::{
    processor::{ClientType, RequestContext, RequestProcessor},
    response::{ChatCompletionResult, CompletionResult, ResponseMetadata},
    template_handler::handle_templated_chat_request,
    types::{
        ChatCompletionRequest, ChatCompletionResponse, CompletionRequest, CompletionResponse,
        WithModelParameters,
    },
    utils::{
        extract_conversation_id_from_headers, extract_request_id_from_headers,
        record_streaming_metrics,
    },
};
use axum::{extract::State, http::HeaderMap, Json};
use tracing::{debug, info, info_span, Instrument};

/// Chat completion endpoint handler
///
/// Handles POST requests to /v1/chat/completions
/// Supports both streaming and non-streaming responses.
///
/// For models that require template formatting, this will automatically
/// route to the completions endpoint and convert the response.
#[tracing::instrument(
    skip(state, headers, request),
    fields(
        model = %request.model,
        stream = request.stream,
        request_id = tracing::field::Empty
    )
)]
pub async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<ChatCompletionResult> {
    // 📊 记录请求到达时间（用于流式请求延迟统计）
    let request_arrival_time = std::time::Instant::now();

    // Extract request ID from traceparent header or generate new UUID
    let request_id = extract_request_id_from_headers(&headers);

    // Extract conversation ID from X-Conversation-Id header
    let conversation_id = extract_conversation_id_from_headers(&headers);

    tracing::Span::current().record("request_id", &request_id);
    let request_id_for_span = request_id.clone();

    // Log incoming request details (combined into single INFO log)
    let payload_size = serde_json::to_string(&request)
        .map(|s| s.len())
        .unwrap_or(0);
    info!(
        request_id = %request_id,
        payload_size_bytes = payload_size,
        message_count = request.messages.len(),
        "📥 Incoming chat completion request: {} bytes, {} messages",
        payload_size,
        request.messages.len()
    );

    // DEBUG: Log full incoming request payload
    if let Ok(request_json) = serde_json::to_string(&request) {
        debug!(
            request_id = %request_id,
            "📋 Full incoming request: {}",
            request_json
        );
    }

    // Check if this request needs template formatting and should be routed to completions endpoint
    let selected_provider = state
        .provider_service
        .select_provider(&request.model, conversation_id.as_deref())
        .await?;
    let model_class_name = &selected_provider.model_config.model_class_name;

    if crate::config::ModelRegistry::needs_template(model_class_name) {
        debug!(
            "Routing templated request to completions endpoint for model class: {}",
            model_class_name
        );
        // Convert ChatCompletionRequest to CompletionRequest and route to completions endpoint
        return handle_templated_chat_request(
            state,
            headers,
            request,
            selected_provider,
            request_id_for_span,
            conversation_id.clone(),
            request_arrival_time,
        )
        .await;
    }

    async move {
        let is_streaming = request.stream;

        if is_streaming {
            let model_id = request.model.clone();
            let req_with_params = request
                .with_model_parameters(&selected_provider.model_config.parameters);

            let base_url_for_metadata = selected_provider.base_url.clone();
            let model_name_for_metadata = selected_provider.override_model_name.clone();

            let stream = record_streaming_metrics(
                &state,
                &selected_provider,
                &model_id,
                request_arrival_time,
                || async {
                    // Get model-specific timeout config if available, otherwise use global config
                    let stream_timeout_ms = selected_provider.model_config
                        .get_stream_no_token_timeout_ms();
                    let response_finish_timeout_ms = selected_provider.model_config
                        .get_response_finish_timeout_ms();

                    // Debug: log the raw parameters to help troubleshoot timeout config
                    debug!("🔍 [chat] Model {} parameters: {}",
                        selected_provider.model_config.id,
                        selected_provider.model_config.parameters);

                    if let Some(timeout) = stream_timeout_ms {
                        debug!("🕐 Using model-specific stream_no_token_timeout_ms: {}ms for model: {}",
                            timeout, selected_provider.model_config.id);
                    } else {
                        debug!("⚠️ No stream_no_token_timeout_ms found in parameters, using global default for model: {}",
                            selected_provider.model_config.id);
                    }

                    if let Some(timeout) = response_finish_timeout_ms {
                        debug!("🕐 Using model-specific responseFinishTimeout: {}ms for model: {}",
                            timeout, selected_provider.model_config.id);
                    }

                    let timeout_config = state.config.timeouts
                        .to_stream_timeout_config_with_model_override(stream_timeout_ms, response_finish_timeout_ms);

                    let genai_client = crate::genai::GenAIClient::with_timeout_config(
                        selected_provider.clone(),
                        timeout_config
                    )?;
                    handle_chat_streaming_genai(genai_client, req_with_params, headers).await
                }
            ).await?;

            let metadata = ResponseMetadata::new(
                base_url_for_metadata,
                model_name_for_metadata,
            );

            Ok(ChatCompletionResult::Stream(stream, metadata))
        } else {
            // Non-streaming: use retry mechanism
            let model_id = request.model.clone();
            let is_streaming = request.stream;

            let ctx = RequestContext {
                state: &state,
                headers: headers.clone(),
                model_id: model_id.clone(),
                is_streaming,
                conv_id: conversation_id.clone(),
            };

            let (response, _metrics) = RequestProcessor::process_request_with_retry(
                ctx,
                request,
                |client, provider, req, headers| async move {
                    // Apply model configuration parameters to the request
                    let req_with_params = req.with_model_parameters(&provider.model_config.parameters);
                    handle_chat_regular_unified(client, req_with_params, headers).await
                },
            ).await?;

            let parsed_response: ChatCompletionResponse = serde_json::from_str(&response)
                .map_err(|e| AppError::Internal(
                    format!("Failed to parse response: {}", e)
                ))?;

            // Get metadata from the final selected provider
            let final_provider = state.provider_service.select_provider(&model_id, conversation_id.as_deref()).await?;
            let metadata = ResponseMetadata::new(
                final_provider.base_url.clone(),
                final_provider.override_model_name.clone(),
            );

            Ok(ChatCompletionResult::Json(parsed_response, metadata))
        }
    }
    .instrument(info_span!("chat_completion", request_id = %request_id_for_span))
    .await
}

/// Text completion endpoint handler
///
/// Handles POST requests to /v1/completions
/// Supports both streaming and non-streaming responses.
#[tracing::instrument(
    skip(state, headers, request),
    fields(
        model = %request.model,
        stream = request.stream,
        request_id = tracing::field::Empty
    )
)]
pub async fn text_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CompletionRequest>,
) -> Result<CompletionResult> {
    // 📊 记录请求到达时间（用于流式请求延迟统计）
    let request_arrival_time = std::time::Instant::now();

    // Extract request ID from traceparent header or generate new UUID
    let request_id = extract_request_id_from_headers(&headers);

    // Extract conversation ID from X-Conversation-Id header
    let conversation_id = extract_conversation_id_from_headers(&headers);

    tracing::Span::current().record("request_id", &request_id);
    let request_id_for_span = request_id.clone();

    async move {
        // Log incoming request details (combined into single INFO log)
        let payload_size = serde_json::to_string(&request)
            .map(|s| s.len())
            .unwrap_or(0);
        let prompt_len = request.prompt.len();
        info!(
            request_id = %request_id,
            payload_size_bytes = payload_size,
            prompt_length = prompt_len,
            "📥 Incoming text completion request: {} bytes, prompt: {} chars",
            payload_size,
            prompt_len
        );

        // DEBUG: Log full incoming request payload
        if let Ok(request_json) = serde_json::to_string(&request) {
            debug!(
                request_id = %request_id,
                "📋 Full incoming text completion request: {}",
                request_json
            );
        }

        let is_streaming = request.stream;
        let ctx = RequestContext {
            state: &state,
            headers: headers.clone(),
            model_id: request.model.clone(),
            is_streaming,
            conv_id: conversation_id.clone(),
        };

        if is_streaming {
            let selected_provider = state.provider_service.select_provider(&request.model, conversation_id.as_deref()).await?;
            let model_id = request.model.clone();
            let req_with_params = request
                .with_model_parameters(&selected_provider.model_config.parameters);

            let stream = record_streaming_metrics(
                &state,
                &selected_provider,
                &model_id,
                request_arrival_time,
                || async {
                    // Get model-specific timeout config if available, otherwise use global config
                    let stream_timeout_ms = selected_provider.model_config
                        .get_stream_no_token_timeout_ms();
                    let response_finish_timeout_ms = selected_provider.model_config
                        .get_response_finish_timeout_ms();

                    if let Some(timeout) = stream_timeout_ms {
                        debug!("🕐 Using model-specific stream_no_token_timeout_ms: {}ms for model: {}",
                            timeout, selected_provider.model_config.id);
                    } else {
                        debug!("Using global stream timeout config for model: {}",
                            selected_provider.model_config.id);
                    }

                    if let Some(timeout) = response_finish_timeout_ms {
                        debug!("🕐 Using model-specific responseFinishTimeout: {}ms for model: {}",
                            timeout, selected_provider.model_config.id);
                    }

                    let timeout_config = state.config.timeouts
                        .to_stream_timeout_config_with_model_override(stream_timeout_ms, response_finish_timeout_ms);

                    let genai_client = crate::genai::GenAIClient::with_timeout_config(
                        selected_provider.clone(),
                        timeout_config
                    )?;
                    handle_completion_streaming_genai(genai_client, req_with_params, headers).await
                }
            ).await?;

            Ok(CompletionResult::Stream(stream))
        } else {
            let (response, _metrics) = RequestProcessor::process_request_with_retry(
                ctx,
                request,
                |client, provider, req, headers| async move {
                    // Apply model configuration parameters to the request
                    let req_with_params = req.with_model_parameters(&provider.model_config.parameters);
                    handle_completion_regular_unified(client, req_with_params, headers).await
                },
            ).await?;

            let parsed_response: CompletionResponse = serde_json::from_str(&response)
                .map_err(|e| crate::error::AppError::Internal(
                    format!("Failed to parse response: {}", e)
                ))?;
            Ok(CompletionResult::Json(parsed_response))
        }
    }
    .instrument(info_span!("text_completion", request_id = %request_id_for_span))
    .await
}

// ==================== Handler Functions ====================

/// Unified handler for regular chat completion (GenAI only)
async fn handle_chat_regular_unified(
    client: ClientType,
    request: ChatCompletionRequest,
    headers: HeaderMap,
) -> Result<(String, Option<u64>)> {
    client.chat_completion(request, headers).await
}

/// Unified handler for regular text completion (GenAI only)
async fn handle_completion_regular_unified(
    client: ClientType,
    request: CompletionRequest,
    headers: HeaderMap,
) -> Result<(String, Option<u64>)> {
    client.text_completion(request, headers).await
}

/// Handle streaming chat completion with GenAI client
async fn handle_chat_streaming_genai(
    genai_client: crate::genai::GenAIClient,
    request: ChatCompletionRequest,
    headers: HeaderMap,
) -> Result<(crate::handlers::chat::response::StreamResponse, Option<u64>)> {
    let stream = genai_client
        .chat_completion_stream(request, headers)
        .await?;

    Ok((stream, None))
}

/// Handle streaming text completion with GenAI client
async fn handle_completion_streaming_genai(
    genai_client: crate::genai::GenAIClient,
    request: CompletionRequest,
    headers: HeaderMap,
) -> Result<(crate::handlers::chat::response::StreamResponse, Option<u64>)> {
    let stream = genai_client
        .text_completion_stream(request, headers)
        .await?;

    Ok((stream, None))
}
