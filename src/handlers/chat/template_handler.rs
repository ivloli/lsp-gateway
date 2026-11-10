//! Template-based chat completion handler
//!
//! This module handles chat completion requests for models that require template formatting.
//! These models use the /v1/completions endpoint but need to accept chat messages as input.
//!
//! The conversion process:
//! 1. Check if the model needs template formatting
//! 2. Convert chat messages to a single prompt string using the model's template
//! 3. Send the request to the completions endpoint
//! 4. Convert the response back to chat completion format

use crate::app::AppState;
use crate::error::{AppError, Result};
use crate::handlers::chat::{
    response::{ChatCompletionResult, ResponseMetadata},
    stream_converter::convert_completion_stream_to_chat_stream,
    types::{
        ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, CompletionRequest,
        CompletionResponse, MessageContent, WithModelParameters,
    },
    utils::record_streaming_metrics,
};
use axum::http::HeaderMap;
use tracing::{debug, error, info, info_span, Instrument};

/// Handle templated chat requests by converting them to completion requests
///
/// This function is called when a model requires template formatting (e.g., Llama, Mistral).
/// It converts the chat messages into a single prompt string using the model's template,
/// processes it through the completions endpoint, and converts the response back to
/// chat completion format.
pub async fn handle_templated_chat_request(
    state: AppState,
    headers: HeaderMap,
    chat_request: ChatCompletionRequest,
    selected_provider: crate::routing::provider::SelectedProvider,
    request_id: String,
    conv_id: Option<String>,
    request_arrival_time: std::time::Instant,
) -> Result<ChatCompletionResult> {
    async move {
        // Convert ChatCompletionRequest to CompletionRequest using template
        let completion_request = convert_chat_to_completion_request(
            &chat_request,
            &selected_provider.model_config.model_class_name,
        )?;

        info!(
            "🔄 Converted to completion request with prompt length: {}",
            completion_request.prompt.len()
        );

        let is_streaming = completion_request.stream;

        if is_streaming {
            handle_templated_streaming(
                state,
                headers,
                completion_request,
                selected_provider,
                request_arrival_time,
            )
            .await
        } else {
            handle_templated_non_streaming(
                state,
                headers,
                completion_request,
                selected_provider,
                conv_id,
            )
            .await
        }
    }
    .instrument(info_span!("templated_chat_completion", request_id = %request_id))
    .await
}

/// Convert ChatCompletionRequest to CompletionRequest using template
fn convert_chat_to_completion_request(
    chat_request: &ChatCompletionRequest,
    model_class_name: &str,
) -> Result<CompletionRequest> {
    // Get template for the model
    let template = crate::templates::get_template_for_model(model_class_name)
        .ok_or_else(|| AppError::Internal("Template not found".to_string()))?;

    // Squash consecutive messages first
    let squashed_request = chat_request.clone().squash_consecutive_messages();

    // Convert messages to JSON format for template rendering
    let messages_json: Vec<serde_json::Value> = squashed_request
        .messages
        .iter()
        .map(|msg| {
            serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            })
        })
        .collect();

    // Render template with messages
    let prompt = match crate::templates::render_template(template, &messages_json) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to render template: {}", e);
            // Fallback to simple concatenation
            squashed_request
                .messages
                .iter()
                .map(|m| format!("{}: {}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    };

    // Create CompletionRequest from ChatCompletionRequest
    Ok(CompletionRequest {
        model: chat_request.model.clone(),
        prompt,
        stream: chat_request.stream,
        temperature: chat_request.temperature,
        max_tokens: chat_request.max_tokens,
        top_p: chat_request.top_p,
        frequency_penalty: chat_request.frequency_penalty,
        presence_penalty: chat_request.presence_penalty,
        stop: chat_request.stop.clone(),
        user: chat_request.user.clone(),
        extra: chat_request.extra.clone(),
    })
}

/// Handle streaming templated request
async fn handle_templated_streaming(
    state: AppState,
    headers: HeaderMap,
    completion_request: CompletionRequest,
    selected_provider: crate::routing::provider::SelectedProvider,
    request_arrival_time: std::time::Instant,
) -> Result<ChatCompletionResult> {
    let model_id = completion_request.model.clone();
    let base_url_for_metadata = selected_provider.base_url.clone();
    let model_name_for_metadata = selected_provider.override_model_name.clone();

    let req_with_params =
        completion_request.with_model_parameters(&selected_provider.model_config.parameters);

    let completion_stream = record_streaming_metrics(
        &state,
        &selected_provider,
        &model_id,
        request_arrival_time,
        || async {
            // Get model-specific timeout config if available, otherwise use global config
            let stream_timeout_ms = selected_provider
                .model_config
                .get_stream_no_token_timeout_ms();
            let response_finish_timeout_ms = selected_provider
                .model_config
                .get_response_finish_timeout_ms();

            if let Some(timeout) = stream_timeout_ms {
                debug!(
                    "🕐 Using model-specific stream_no_token_timeout_ms: {}ms for model: {}",
                    timeout, selected_provider.model_config.id
                );
            } else {
                debug!(
                    "Using global stream timeout config for model: {}",
                    selected_provider.model_config.id
                );
            }

            if let Some(timeout) = response_finish_timeout_ms {
                debug!(
                    "🕐 Using model-specific responseFinishTimeout: {}ms for model: {}",
                    timeout, selected_provider.model_config.id
                );
            }

            let timeout_config = state
                .config
                .timeouts
                .to_stream_timeout_config_with_model_override(
                    stream_timeout_ms,
                    response_finish_timeout_ms,
                );

            let genai_client = crate::genai::GenAIClient::with_timeout_config(
                selected_provider.clone(),
                timeout_config,
            )?;
            handle_completion_streaming_genai(genai_client, req_with_params, headers).await
        },
    )
    .await?;

    // Convert completion stream to chat completion stream
    let chat_stream = convert_completion_stream_to_chat_stream(completion_stream);

    let metadata = ResponseMetadata::new(base_url_for_metadata, model_name_for_metadata);

    Ok(ChatCompletionResult::Stream(chat_stream, metadata))
}

/// Handle non-streaming templated request
async fn handle_templated_non_streaming(
    state: AppState,
    headers: HeaderMap,
    completion_request: CompletionRequest,
    selected_provider: crate::routing::provider::SelectedProvider,
    _conv_id: Option<String>,
) -> Result<ChatCompletionResult> {
    let model_id = completion_request.model.clone();
    let timer = std::time::Instant::now();

    // Apply model configuration parameters to the request
    let req_with_params =
        completion_request.with_model_parameters(&selected_provider.model_config.parameters);

    // Create GenAI client with model-specific timeout config
    let stream_timeout_ms = selected_provider
        .model_config
        .get_stream_no_token_timeout_ms();
    let response_finish_timeout_ms = selected_provider
        .model_config
        .get_response_finish_timeout_ms();
    let timeout_config = state
        .config
        .timeouts
        .to_stream_timeout_config_with_model_override(
            stream_timeout_ms,
            response_finish_timeout_ms,
        );
    let genai_client =
        crate::genai::GenAIClient::with_timeout_config(selected_provider.clone(), timeout_config)?;

    // Execute the request
    let result = genai_client.text_completion(req_with_params, headers).await;
    let latency_ms = timer.elapsed().as_millis() as u64;

    // Determine model provider group for metrics
    // Use model_provider_group if available, otherwise use "direct-{model_config.id}"
    let model_provider_group = selected_provider.model_config.model_provider_group.clone();
    let group_for_prometheus = model_provider_group
        .clone()
        .unwrap_or_else(|| format!("direct-{}", selected_provider.model_config.id));

    match result {
        Ok((response, token_count)) => {
            // Record success metrics
            if let Some(tokens) = token_count {
                state
                    .provider_service
                    .record_request_completion_with_tokens(
                        &selected_provider.id,
                        true,
                        latency_ms,
                        tokens,
                        model_provider_group,
                    )
                    .await;
            } else {
                state
                    .provider_service
                    .record_request_completion(
                        &selected_provider.id,
                        true,
                        latency_ms,
                        model_provider_group,
                    )
                    .await;
            }

            crate::monitoring::record_success_global(
                &selected_provider.id,
                &model_id,
                &selected_provider.base_url,
                &group_for_prometheus,
                latency_ms,
                token_count,
            );

            info!(
                "✅ Templated completion successful: provider={}, model={}, latency={}ms",
                selected_provider.id, model_id, latency_ms
            );

            // Parse as CompletionResponse first, then convert to ChatCompletionResponse
            let completion_response: CompletionResponse =
                serde_json::from_str(&response).map_err(|e| {
                    AppError::Internal(format!("Failed to parse completion response: {}", e))
                })?;

            // Convert CompletionResponse to ChatCompletionResponse
            let chat_response = convert_completion_to_chat_response(completion_response);

            let metadata = ResponseMetadata::new(
                selected_provider.base_url.clone(),
                selected_provider.override_model_name.clone(),
            );

            Ok(ChatCompletionResult::Json(chat_response, metadata))
        }
        Err(e) => {
            // Record failure metrics
            state
                .provider_service
                .record_request_completion(
                    &selected_provider.id,
                    false,
                    latency_ms,
                    model_provider_group,
                )
                .await;

            crate::monitoring::record_failure_global(
                &selected_provider.id,
                &model_id,
                &selected_provider.base_url,
                &group_for_prometheus,
                "provider_error",
                latency_ms,
            );

            error!(
                "❌ Templated completion failed: provider={}, model={}, error={}",
                selected_provider.id, model_id, e
            );

            Err(AppError::Provider(format!(
                "Request to provider failed: {}",
                e
            )))
        }
    }
}

/// Convert CompletionResponse to ChatCompletionResponse
fn convert_completion_to_chat_response(
    completion_response: CompletionResponse,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: completion_response.id,
        object: "chat.completion".to_string(),
        created: completion_response.created,
        model: completion_response.model,
        choices: completion_response
            .choices
            .into_iter()
            .map(|choice| ChatChoice {
                index: choice.index,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: MessageContent::Text(choice.text),
                    name: None,
                },
                finish_reason: choice.finish_reason,
            })
            .collect(),
        usage: completion_response.usage,
    }
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
