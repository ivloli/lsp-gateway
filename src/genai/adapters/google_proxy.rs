// Google Proxy Adapter - 通过 genai-proxy 调用 Google GenAI SDK
use super::google_common::{
    build_generation_config, convert_gemini_to_openai_response,
    convert_messages_to_gemini_contents, extract_system_instruction, is_vertex_from_extra,
    should_use_system_instruction,
};
use super::{EndpointType, ProviderAdapter};
use crate::error::Result;
use crate::genai::utils::AdapterUtils;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionRequest, CompletionResponse,
    ToProviderRequest,
};
use axum::http::HeaderMap;
use serde_json::{json, Value as JsonValue};
use tracing::{debug, warn};

pub struct GoogleProxyAdapter {
    proxy_url: String,
}

impl GoogleProxyAdapter {
    pub fn new() -> Self {
        let proxy_url = std::env::var("GOOGLE_PROXY_URL")
            .unwrap_or_else(|_| "http://localhost:8081".to_string());
        Self { proxy_url }
    }

    /// Check if using Vertex AI from request extra fields (for ChatCompletionRequest)
    fn is_vertex_from_request(request: &ChatCompletionRequest) -> bool {
        is_vertex_from_extra(&serde_json::to_value(&request.extra).unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for GoogleProxyAdapter {
    fn transform_chat_request(
        &self,
        request: &ChatCompletionRequest,
        target_model: &str,
        model_class_name: &str,
        base_url: &str,
    ) -> Result<JsonValue> {
        debug!(
            "🔄 Processing {} messages for Gemini proxy (model_class: {})",
            request.messages.len(),
            model_class_name
        );

        let use_system_instruction =
            should_use_system_instruction("google_proxy", model_class_name);

        let (system_instruction, processed_chat_request) = if use_system_instruction {
            // Path 1: System instruction enabled
            // 1. Extract ALL system messages (for systemInstruction)
            // 2. Remove system messages from the list
            // 3. Apply squash and role validation to remaining messages

            // Check for explicit gemini_system_instruction first
            let explicit_system_instruction = request
                .extra
                .get("extra_body")
                .and_then(|eb| eb.get("gemini_system_instruction"))
                .cloned();

            let system_instruction = if explicit_system_instruction.is_some() {
                explicit_system_instruction
            } else {
                extract_system_instruction(&request.messages, &request.extra)?
            };

            let messages_without_system: Vec<_> = request
                .messages
                .iter()
                .filter(|m| m.role != "system")
                .cloned()
                .collect();

            let mut temp_request = request.clone();
            temp_request.messages = messages_without_system;

            let processed_request =
                temp_request.to_provider_request(target_model, model_class_name);
            let processed_chat_request: ChatCompletionRequest =
                serde_json::from_value(processed_request).map_err(|e| {
                    crate::error::AppError::Internal(format!(
                        "Failed to deserialize processed request: {}",
                        e
                    ))
                })?;

            (system_instruction, processed_chat_request)
        } else {
            // Path 2: System instruction disabled
            // Apply squash and role validation to ALL messages (including system)
            // System messages will be converted to user messages as needed

            let processed_request = request.to_provider_request(target_model, model_class_name);
            let processed_chat_request: ChatCompletionRequest =
                serde_json::from_value(processed_request).map_err(|e| {
                    crate::error::AppError::Internal(format!(
                        "Failed to deserialize processed request: {}",
                        e
                    ))
                })?;

            (None, processed_chat_request)
        };

        debug!(
            "Message roles after processing: {:?}",
            processed_chat_request
                .messages
                .iter()
                .map(|m| &m.role)
                .collect::<Vec<_>>()
        );

        // Convert processed messages to Gemini contents format (skip system messages)
        let final_contents =
            convert_messages_to_gemini_contents(&processed_chat_request.messages, true)?;
        let is_vertex = Self::is_vertex_from_request(request);
        let backend = if is_vertex { "vertex" } else { "gemini" };
        let config = build_generation_config(request, target_model);

        // Build genai-proxy request
        let mut proxy_request = json!({
            "backend": backend,
            "model": target_model,
            "contents": final_contents,
            "stream": request.stream,
            "baseURL": base_url,
        });

        // Add system instruction if present
        if let Some(sys_inst) = &system_instruction {
            proxy_request["systemInstruction"] = sys_inst.clone();
            debug!("✅ Added systemInstruction to genai-proxy request");
        }

        // For Vertex AI, add project and location from request.extra
        if is_vertex {
            if let Some(project) = request.extra.get("project") {
                proxy_request["project"] = project.clone();
            }
            if let Some(location) = request.extra.get("location") {
                proxy_request["location"] = location.clone();
            }
        }

        // Add config if not empty
        if !config.as_object().unwrap().is_empty() {
            proxy_request["config"] = config;
        }

        // Add safety settings (default: turn off all filters to avoid over-filtering)
        // This matches geminiChatV2.ts behavior
        let default_safety_settings = json!([
            {"category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_HARASSMENT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_CIVIC_INTEGRITY", "threshold": "OFF"},
        ]);

        // Allow override via extra.safety_settings
        if let Some(custom_settings) = request.extra.get("safety_settings") {
            proxy_request["safetySettings"] = custom_settings.clone();
            debug!("✅ Using custom safety settings from request");
        } else {
            proxy_request["safetySettings"] = default_safety_settings;
            debug!("✅ Using default safety settings (all filters OFF)");
        }

        // Handle extra_body fields (like google.thinking_config)
        if let Some(extra_body) = request.extra.get("extra_body") {
            if let Some(google_fields) = extra_body.get("google") {
                // Merge google-specific fields directly into the request
                if let Some(google_obj) = google_fields.as_object() {
                    for (key, value) in google_obj {
                        // Special handling for thinking_config:
                        // 1. Convert to camelCase (thinking_config -> thinkingConfig)
                        // 2. Place it inside config object (not at top level)
                        // 3. Convert internal fields to camelCase (thinking_budget -> thinkingBudget)
                        if key == "thinking_config" {
                            // Convert internal fields to camelCase
                            let thinking_config = if let Some(tc_obj) = value.as_object() {
                                let mut converted = serde_json::Map::new();
                                for (tc_key, tc_value) in tc_obj {
                                    let camel_tc_key = match tc_key.as_str() {
                                        "thinking_budget" => "thinkingBudget".to_string(),
                                        "include_thoughts" => "includeThoughts".to_string(),
                                        "thinking_level" => "thinkingLevel".to_string(),
                                        _ => tc_key.clone(),
                                    };
                                    converted.insert(camel_tc_key, tc_value.clone());
                                }
                                JsonValue::Object(converted)
                            } else {
                                value.clone()
                            };

                            // Place thinkingConfig inside config object
                            if let Some(config_obj) = proxy_request
                                .get_mut("config")
                                .and_then(|c| c.as_object_mut())
                            {
                                config_obj
                                    .insert("thinkingConfig".to_string(), thinking_config.clone());
                            } else {
                                // If config doesn't exist, create it
                                proxy_request["config"] =
                                    json!({"thinkingConfig": thinking_config});
                            }
                            debug!(
                                "Added extra_body.google.thinking_config to config.thinkingConfig"
                            );
                        } else {
                            // For other google fields, add to top level with camelCase conversion
                            debug!("Added extra_body.google.{} to proxy request", key);
                            proxy_request[key.clone()] = value.clone();
                        }
                    }
                }
            }
        }

        // Debug log: show what we're sending to Gemini
        debug!(
            "Gemini request to be sent: {}",
            serde_json::to_string(&proxy_request).unwrap_or_default()
        );

        Ok(proxy_request)
    }

    fn transform_completion_request(
        &self,
        request: &CompletionRequest,
        target_model: &str,
        _model_class_name: &str,
        base_url: &str,
    ) -> Result<JsonValue> {
        // Convert completion request to chat format for genai-proxy
        let contents = vec![json!({
            "role": "user",
            "parts": [{"text": request.prompt}]
        })];

        let request_extra = serde_json::to_value(&request.extra).unwrap_or_default();
        let is_vertex = is_vertex_from_extra(&request_extra);
        let backend = if is_vertex { "vertex" } else { "gemini" };
        let config = build_generation_config(request, target_model);

        // Build genai-proxy request
        let mut proxy_request = json!({
            "backend": backend,
            "model": target_model,
            "contents": contents,
            "stream": request.stream,
            "baseURL": base_url,
        });

        // For Vertex AI, add project and location
        if is_vertex {
            if let Some(project) = request.extra.get("project") {
                proxy_request["project"] = project.clone();
            }
            if let Some(location) = request.extra.get("location") {
                proxy_request["location"] = location.clone();
            }
        }

        // Add config if not empty
        if !config.as_object().unwrap().is_empty() {
            proxy_request["config"] = config;
        }

        // Add safety settings (same as chat request)
        let default_safety_settings = json!([
            {"category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_HARASSMENT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "OFF"},
            {"category": "HARM_CATEGORY_CIVIC_INTEGRITY", "threshold": "OFF"},
        ]);

        if let Some(custom_settings) = request.extra.get("safety_settings") {
            proxy_request["safetySettings"] = custom_settings.clone();
        } else {
            proxy_request["safetySettings"] = default_safety_settings;
        }

        Ok(proxy_request)
    }

    fn transform_chat_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse> {
        // genai-proxy returns {"result": {...}} format
        // Extract result field and convert
        let response_json: JsonValue = serde_json::from_str(response).map_err(|e| {
            crate::error::AppError::Internal(format!("Failed to parse genai-proxy response: {}", e))
        })?;

        if let Some(result) = response_json.get("result") {
            convert_gemini_to_openai_response(&serde_json::to_string(result)?, original_model)
        } else {
            // Fallback: try direct conversion
            convert_gemini_to_openai_response(response, original_model)
        }
    }

    fn transform_completion_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<CompletionResponse> {
        let chat_response = self.transform_chat_response(response, original_model)?;
        Ok(AdapterUtils::chat_to_completion_response(chat_response))
    }

    fn process_headers(
        &self,
        _headers: &mut HeaderMap,
        _base_url: &str,
        _request_extra: &JsonValue,
    ) {
        // genai-proxy handles headers internally, no processing needed
    }

    fn get_headers(&self, api_key: &str, _base_url: &str, _request_extra: &JsonValue) -> HeaderMap {
        // genai-proxy gets API key from header or payload, we pass it via header
        let mut headers = HeaderMap::new();
        // headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert(
            "authorization",
            format!("Bearer {}", api_key).parse().unwrap(),
        );
        headers
    }

    fn provider_name(&self) -> &'static str {
        "google-proxy"
    }

    fn get_endpoint_url(
        &self,
        _base_url: &str,
        _model_name: &str,
        _endpoint_type: EndpointType,
        _api_key: Option<&str>,
        _request_extra: &JsonValue,
    ) -> String {
        format!("{}/generate", self.proxy_url.trim_end_matches('/'))
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        // genai-proxy returns SSE format: data: {...}\n\n
        // We need to extract the data field and convert to OpenAI format

        // Parse SSE format
        let lines: Vec<&str> = chunk.lines().collect();
        let mut data_content = String::new();

        for line in lines {
            if line.starts_with("data: ") {
                data_content = line[6..].to_string();
                break;
            }
        }

        // If no SSE data found, check if chunk itself is JSON (for non-SSE responses)
        if data_content.is_empty() {
            if chunk.trim().starts_with('{') {
                data_content = chunk.trim().to_string();
            } else {
                return AdapterUtils::process_stream_chunk(chunk, original_model, is_completion);
            }
        }

        // Check for [DONE] marker
        if data_content.trim() == "[DONE]" || chunk.contains("event: done") {
            return Ok("data: [DONE]\n\n".to_string());
        }

        // Parse the JSON data
        let gemini_json: JsonValue = serde_json::from_str(&data_content).map_err(|e| {
            crate::error::AppError::Internal(format!(
                "Failed to parse genai-proxy data: {} (data: {})",
                e, data_content
            ))
        })?;

        // Check for error event
        if let Some(error) = gemini_json.get("error") {
            return Err(crate::error::AppError::Internal(format!(
                "genai-proxy error: {}",
                error
            )));
        }

        // Extract candidates array
        let candidates = gemini_json.get("candidates").and_then(|c| c.as_array());

        if candidates.is_none() || candidates.unwrap().is_empty() {
            // No candidates yet, return empty chunk
            return Ok(String::new());
        }

        let first_candidate = candidates.unwrap().first().unwrap();

        // Extract and concatenate all text parts from content (not just the first one)
        // Gemini may return multiple parts in a single response
        // Filter out parts with "thought": true to exclude thinking content
        let content = first_candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(|parts| parts.as_array())
            .map(|parts_arr| {
                parts_arr
                    .iter()
                    // Filter out thinking parts: only keep parts without "thought": true
                    // Normal content parts either have no "thought" field or "thought": false
                    .filter(|part| {
                        !part
                            .get("thought")
                            .and_then(|t| t.as_bool())
                            .unwrap_or(false) // Default to false (non-thinking) if field is missing
                    })
                    .filter_map(|part| part.get("text"))
                    .filter_map(|text| text.as_str())
                    .collect::<Vec<_>>()
                    .join("") // Concatenate all non-thinking text parts
            })
            .unwrap_or_default();

        // Extract finish reason
        let finish_reason = first_candidate
            .get("finishReason")
            .and_then(|reason| reason.as_str())
            .map(|s| s.to_string());

        // Check for Gemini-specific content filter reasons in streaming
        // Similar to geminiChatV2.ts: don't throw error immediately, allow partial content through
        // Map safety-related finish reasons to "content_filter" and let the stream end gracefully
        let normalized_finish_reason_for_filter = if let Some(reason) = finish_reason.as_deref() {
            match reason {
                "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" => {
                    warn!("⚠️ Stream truncated by Gemini safety filters (proxy): {} - returning partial content", reason);
                    Some("content_filter")
                }
                _ => None,
            }
        } else {
            None
        };

        // Extract usage metadata if present
        let usage =
            gemini_json
                .get("usageMetadata")
                .map(|usage_json| crate::handlers::chat::Usage {
                    prompt_tokens: usage_json
                        .get("promptTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    completion_tokens: usage_json
                        .get("candidatesTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    total_tokens: usage_json
                        .get("totalTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                });

        // Use content_filter finish reason if safety filter triggered, otherwise normalize normally
        let normalized_finish_reason = normalized_finish_reason_for_filter.or_else(|| {
            finish_reason
                .as_deref()
                .map(AdapterUtils::normalize_finish_reason)
        });
        let mut result = AdapterUtils::create_stream_chunk_with_usage(
            &content,
            original_model,
            normalized_finish_reason,
            usage,
            is_completion,
        )?;

        // Add [DONE] marker if we have a finish_reason
        if normalized_finish_reason.is_some() {
            result.push_str("data: [DONE]\n\n");
        }

        Ok(result)
    }
}
