use super::{EndpointType, ProviderAdapter};
use crate::error::{AppError, Result};
use crate::genai::utils::AdapterUtils;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, CompletionRequest,
    CompletionResponse, Usage,
};
use axum::http::HeaderMap;
use serde_json::{json, Value as JsonValue};

pub struct AnthropicAdapter;

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Convert OpenAI format messages to Anthropic format
    fn convert_messages_to_anthropic(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(Option<String>, Vec<JsonValue>)> {
        let mut system_message = None;
        let mut anthropic_messages = Vec::new();

        for message in messages {
            match message.role.as_str() {
                "system" => {
                    system_message = Some(message.content.to_string());
                }
                "user" | "assistant" => {
                    anthropic_messages.push(json!({
                        "role": message.role,
                        "content": message.content.to_string()
                    }));
                }
                _ => {
                    // Skip unknown roles or convert to user
                    anthropic_messages.push(json!({
                        "role": "user",
                        "content": message.content.to_string()
                    }));
                }
            }
        }

        Ok((system_message, anthropic_messages))
    }

    /// Convert Anthropic response to OpenAI format
    fn convert_anthropic_to_openai_response(
        &self,
        anthropic_response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse> {
        let anthropic_json: JsonValue =
            AdapterUtils::parse_response(anthropic_response, "Anthropic")?;
        AdapterUtils::check_api_error(&anthropic_json, "Anthropic")?;

        // Extract content from Anthropic response
        let content_array = anthropic_json
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                AppError::Internal(
                    "Missing or invalid 'content' field in Anthropic response".to_string(),
                )
            })?;

        let content = content_array
            .first()
            .and_then(|c| c.get("text"))
            .and_then(|text| text.as_str())
            .unwrap_or("");

        // Extract usage information
        let usage = anthropic_json.get("usage").map(|usage_json| Usage {
            prompt_tokens: usage_json
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            completion_tokens: usage_json
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            total_tokens: usage_json
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32
                + usage_json
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
        });

        Ok(AdapterUtils::create_chat_response(
            content,
            original_model,
            usage,
            Some("stop"),
        ))
    }

    /// Convert Anthropic streaming chunk to OpenAI streaming format
    fn convert_anthropic_stream_chunk_to_openai(
        &self,
        anthropic_chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        let chunk_data = if anthropic_chunk.starts_with("data: ") {
            &anthropic_chunk[6..]
        } else {
            anthropic_chunk
        };

        if chunk_data.trim() == "[DONE]" || chunk_data.trim().is_empty() {
            return AdapterUtils::process_stream_chunk(
                anthropic_chunk,
                original_model,
                is_completion,
            );
        }

        let anthropic_json: JsonValue = AdapterUtils::parse_response(chunk_data, "Anthropic")?;
        AdapterUtils::check_api_error(&anthropic_json, "Anthropic")?;

        let event_type = anthropic_json
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        match event_type {
            "content_block_delta" => {
                let content = anthropic_json
                    .get("delta")
                    .and_then(|delta| delta.get("text"))
                    .and_then(|text| text.as_str())
                    .unwrap_or("");

                AdapterUtils::create_stream_chunk(content, original_model, None, is_completion)
            }
            "message_delta" => {
                // Extract usage information from message_delta event
                let usage = anthropic_json.get("usage").map(|usage_json| Usage {
                    prompt_tokens: usage_json
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    completion_tokens: usage_json
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    total_tokens: usage_json
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32
                        + usage_json
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32,
                });

                // Extract finish reason from delta if present
                let finish_reason = anthropic_json
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(|reason| reason.as_str())
                    .or(Some("stop"));

                AdapterUtils::create_stream_chunk_with_usage(
                    "",
                    original_model,
                    finish_reason,
                    usage,
                    is_completion,
                )
            }
            "message_stop" => {
                AdapterUtils::create_stream_chunk("", original_model, Some("stop"), is_completion)
            }
            "content_block_start" | "message_start" => Ok(String::new()),
            _ => Ok(String::new()),
        }
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn transform_chat_request(
        &self,
        request: &ChatCompletionRequest,
        target_model: &str,
        _model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        let (system_message, messages) = self.convert_messages_to_anthropic(&request.messages)?;

        let mut anthropic_request = json!({
            "model": target_model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        // Add system message if present
        if let Some(system) = system_message {
            anthropic_request["system"] = JsonValue::String(system);
        }

        // Add optional parameters
        if let Some(temp) = request.temperature {
            anthropic_request["temperature"] =
                JsonValue::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if let Some(top_p) = request.top_p {
            anthropic_request["top_p"] =
                JsonValue::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        if let Some(stop) = &request.stop {
            anthropic_request["stop_sequences"] =
                JsonValue::Array(stop.iter().map(|s| JsonValue::String(s.clone())).collect());
        }

        // Add streaming if requested
        if request.stream {
            anthropic_request["stream"] = JsonValue::Bool(true);
        }

        Ok(anthropic_request)
    }

    fn transform_completion_request(
        &self,
        request: &CompletionRequest,
        target_model: &str,
        _model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        // Convert completion request to chat format for Anthropic
        let messages = vec![json!({
            "role": "user",
            "content": request.prompt
        })];

        let mut anthropic_request = json!({
            "model": target_model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        // Add optional parameters
        if let Some(temp) = request.temperature {
            anthropic_request["temperature"] =
                JsonValue::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if let Some(top_p) = request.top_p {
            anthropic_request["top_p"] =
                JsonValue::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        if let Some(stop) = &request.stop {
            anthropic_request["stop_sequences"] =
                JsonValue::Array(stop.iter().map(|s| JsonValue::String(s.clone())).collect());
        }

        // Add streaming if requested
        if request.stream {
            anthropic_request["stream"] = JsonValue::Bool(true);
        }

        Ok(anthropic_request)
    }

    fn transform_chat_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse> {
        self.convert_anthropic_to_openai_response(response, original_model)
    }

    fn transform_completion_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<CompletionResponse> {
        let chat_response = self.convert_anthropic_to_openai_response(response, original_model)?;
        Ok(AdapterUtils::chat_to_completion_response(chat_response))
    }

    fn get_headers(&self, api_key: &str, _base_url: &str, _request_extra: &JsonValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key.parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        headers
    }

    fn provider_name(&self) -> &'static str {
        "anthropic"
    }

    fn get_endpoint_url(
        &self,
        base_url: &str,
        _model_name: &str,
        _endpoint_type: EndpointType,
        _api_key: Option<&str>,
        _request_extra: &JsonValue,
    ) -> String {
        format!("{}/v1/messages", base_url.trim_end_matches('/'))
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        self.convert_anthropic_stream_chunk_to_openai(chunk, original_model, is_completion)
    }
}
