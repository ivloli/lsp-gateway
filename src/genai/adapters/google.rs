// Google Direct Adapter - 直接调用 Gemini API，不使用 genai-proxy
use super::google_common::{
    build_generation_config, convert_gemini_to_openai_response,
    convert_messages_to_gemini_contents, extract_content_from_candidate,
    extract_system_instruction, is_vertex_from_extra, should_use_system_instruction,
};
use super::{EndpointType, ProviderAdapter};
use crate::error::{AppError, Result};
use crate::genai::utils::AdapterUtils;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionRequest, CompletionResponse,
    ToProviderRequest,
};
use axum::http::HeaderMap;
use serde_json::{json, Value as JsonValue};
use tracing::{debug, warn};

pub struct GoogleAdapter;

impl GoogleAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Normalize base URL for Gemini-compatible endpoints
    /// Removes /v1 suffix if present for Gemini API compatibility
    fn normalize_gemini_base_url(&self, base_url: &str) -> String {
        // Remove trailing slash if present
        let mut normalized_url = base_url.trim_end_matches('/').to_string();

        // Remove /v1 if present
        if normalized_url.ends_with("/v1") {
            normalized_url = normalized_url[..normalized_url.len() - 3].to_string();
        }

        normalized_url
    }

    /// Build endpoint URL for Google/Vertex AI
    fn build_endpoint_url(
        &self,
        base_url: &str,
        model_name: &str,
        action: &str,
        api_key: Option<&str>,
    ) -> String {
        let normalized_base = self.normalize_gemini_base_url(base_url);

        let url = if normalized_base.contains("aiplatform.googleapis.com") {
            format!(
                "{}/v1/publishers/google/models/{}:{}",
                normalized_base, model_name, action
            )
        } else {
            format!(
                "{}/v1beta/models/{}:{}",
                normalized_base, model_name, action
            )
        };

        // For Vertex AI, add API key as query parameter
        if let Some(key) = api_key {
            if url.contains('?') {
                format!("{}&key={}", url, key)
            } else {
                format!("{}?key={}", url, key)
            }
        } else {
            url
        }
    }

    /// Convert Gemini streaming chunk to OpenAI streaming format
    fn convert_gemini_stream_chunk_to_openai(
        &self,
        gemini_chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        let chunk_data = if gemini_chunk.starts_with("data: ") {
            &gemini_chunk[6..]
        } else {
            gemini_chunk
        };

        if chunk_data.trim() == "[DONE]" || chunk_data.trim().is_empty() {
            return AdapterUtils::process_stream_chunk(gemini_chunk, original_model, is_completion);
        }

        let gemini_json: JsonValue = AdapterUtils::parse_response(chunk_data, "Gemini")?;
        AdapterUtils::check_api_error(&gemini_json, "Gemini")?;

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

        // Extract content from either array or single object format
        let (content, finish_reason) = if let Some(array) = gemini_json.as_array() {
            extract_content_from_candidate(array.first())
        } else {
            extract_content_from_candidate(Some(&gemini_json))
        };

        // Check for Gemini-specific content filter reasons in streaming
        if let Some(reason) = finish_reason.as_deref() {
            match reason {
                "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" => {
                    warn!("⚠️ Stream blocked by Gemini safety filters: {}", reason);
                    return Err(AppError::ContentFilter(format!(
                        "Stream blocked by Gemini safety filters: {}",
                        reason
                    )));
                }
                _ => {}
            }
        }

        let normalized_finish_reason = finish_reason
            .as_deref()
            .map(AdapterUtils::normalize_finish_reason);
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

#[async_trait::async_trait]
impl ProviderAdapter for GoogleAdapter {
    fn transform_chat_request(
        &self,
        request: &ChatCompletionRequest,
        target_model: &str,
        model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        debug!(
            "🔄 Processing {} messages for Gemini (model_class: {})",
            request.messages.len(),
            model_class_name
        );

        let use_system_instruction = should_use_system_instruction("google", model_class_name);

        let (system_instruction, processed_chat_request) = if use_system_instruction {
            // Path 1: System instruction enabled
            // 1. Extract ALL system messages (for systemInstruction)
            // 2. Remove system messages from the list
            // 3. Apply squash and role validation to remaining messages

            let system_instruction = extract_system_instruction(&request.messages, &request.extra)?;

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
                    AppError::Internal(format!("Failed to deserialize processed request: {}", e))
                })?;

            (system_instruction, processed_chat_request)
        } else {
            // Path 2: System instruction disabled
            // Apply squash and role validation to ALL messages (including system)
            // System messages will be converted to user messages as needed

            let processed_request = request.to_provider_request(target_model, model_class_name);
            let processed_chat_request: ChatCompletionRequest =
                serde_json::from_value(processed_request).map_err(|e| {
                    AppError::Internal(format!("Failed to deserialize processed request: {}", e))
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

        // Convert processed messages to Gemini contents format
        let contents = convert_messages_to_gemini_contents(&processed_chat_request.messages, true)?;

        let generation_config = build_generation_config(request, target_model);

        // Build Gemini API request format
        let mut gemini_request = json!({
            "contents": contents,
            "model": target_model,
        });

        // Add system instruction if present
        if let Some(sys_inst) = &system_instruction {
            gemini_request["systemInstruction"] = sys_inst.clone();
            debug!("✅ Added systemInstruction to Gemini request");
        }

        // Add generation config if not empty
        if !generation_config.as_object().unwrap().is_empty() {
            gemini_request["generationConfig"] = generation_config;
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
            gemini_request["safetySettings"] = custom_settings.clone();
            debug!("✅ Using custom safety settings from request");
        } else {
            gemini_request["safetySettings"] = default_safety_settings;
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
                        // 2. Place it inside generationConfig object (not at top level)
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

                            // Place thinkingConfig inside generationConfig object
                            if let Some(gen_config_obj) = gemini_request
                                .get_mut("generationConfig")
                                .and_then(|c| c.as_object_mut())
                            {
                                gen_config_obj
                                    .insert("thinkingConfig".to_string(), thinking_config.clone());
                            } else {
                                // If generationConfig doesn't exist, create it
                                gemini_request["generationConfig"] =
                                    json!({"thinkingConfig": thinking_config});
                            }
                            debug!("Added extra_body.google.thinking_config to generationConfig.thinkingConfig");
                        } else {
                            // For other google fields, add to top level with camelCase conversion
                            debug!("Added extra_body.google.{} to Gemini request", key);
                            gemini_request[key.clone()] = value.clone();
                        }
                    }
                }
            }
        }

        // Debug log: show what we're sending to Gemini
        debug!(
            "Gemini request to be sent: {}",
            serde_json::to_string(&gemini_request).unwrap_or_default()
        );

        Ok(gemini_request)
    }

    fn transform_completion_request(
        &self,
        request: &CompletionRequest,
        target_model: &str,
        _model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        // Convert completion request to chat format for Gemini
        let contents = vec![json!({
            "role": "user",
            "parts": [{"text": request.prompt}]
        })];

        let generation_config = build_generation_config(request, target_model);

        let mut gemini_request = json!({
            "contents": contents,
            "model": target_model,
        });

        // Add generation config if not empty
        if !generation_config.as_object().unwrap().is_empty() {
            gemini_request["generationConfig"] = generation_config;
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
            gemini_request["safetySettings"] = custom_settings.clone();
        } else {
            gemini_request["safetySettings"] = default_safety_settings;
        }

        Ok(gemini_request)
    }

    fn transform_chat_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse> {
        convert_gemini_to_openai_response(response, original_model)
    }

    fn transform_completion_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<CompletionResponse> {
        let chat_response = convert_gemini_to_openai_response(response, original_model)?;
        Ok(AdapterUtils::chat_to_completion_response(chat_response))
    }

    fn process_headers(&self, headers: &mut HeaderMap, _base_url: &str, request_extra: &JsonValue) {
        // For Vertex AI, remove any existing authorization header to avoid conflicts
        // Vertex AI uses API key in URL query parameter, not in headers
        if is_vertex_from_extra(request_extra) {
            if headers.remove("authorization").is_some() {
                debug!("Removed authorization header for Vertex AI (uses API key in URL)");
            }
        }
    }

    fn get_headers(&self, api_key: &str, _base_url: &str, request_extra: &JsonValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());

        // For Gemini API (not Vertex AI), add API key to headers
        if !is_vertex_from_extra(request_extra) {
            headers.insert(
                "authorization",
                format!("Bearer {}", api_key).parse().unwrap(),
            );
        }

        headers
    }

    fn provider_name(&self) -> &'static str {
        "google-direct"
    }

    fn get_endpoint_url(
        &self,
        base_url: &str,
        model_name: &str,
        endpoint_type: EndpointType,
        api_key: Option<&str>,
        request_extra: &JsonValue,
    ) -> String {
        let action = match endpoint_type {
            EndpointType::Chat | EndpointType::Completion => "generateContent",
            EndpointType::ChatStream | EndpointType::CompletionStream => "streamGenerateContent",
        };

        // For Vertex AI, add API key to URL; for Gemini API, don't add it (it goes in headers)
        let key_for_url = if is_vertex_from_extra(request_extra) {
            api_key
        } else {
            None
        };
        self.build_endpoint_url(base_url, model_name, action, key_for_url)
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        self.convert_gemini_stream_chunk_to_openai(chunk, original_model, is_completion)
    }
}
