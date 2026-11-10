// Shared utilities for Google adapters
use crate::error::{AppError, Result};
use crate::genai::utils::AdapterUtils;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, CompletionRequest, ContentPart,
    MessageContent, Usage,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::OnceLock;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
struct SystemInstructionConfig {
    google: Vec<String>,
    google_proxy: Vec<String>,
}

static SYSTEM_INSTRUCTION_CONFIG: OnceLock<Option<SystemInstructionConfig>> = OnceLock::new();

fn parse_system_instruction_config() -> Option<SystemInstructionConfig> {
    std::env::var("GEMINI_SYSTEM_INSTRUCTION_MODELS")
        .ok()
        .and_then(|json_str| {
            serde_json::from_str::<JsonValue>(&json_str)
                .ok()
                .and_then(|v| {
                    let google = v
                        .get("google")
                        .and_then(|arr| arr.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let google_proxy = v
                        .get("google_proxy")
                        .and_then(|arr| arr.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(SystemInstructionConfig {
                        google,
                        google_proxy,
                    })
                })
        })
}

fn get_system_instruction_config() -> &'static Option<SystemInstructionConfig> {
    SYSTEM_INSTRUCTION_CONFIG.get_or_init(parse_system_instruction_config)
}

pub fn should_use_system_instruction(adapter: &str, model_class_name: &str) -> bool {
    match get_system_instruction_config() {
        Some(config) => {
            let models = match adapter {
                "google" => &config.google,
                "google_proxy" => &config.google_proxy,
                _ => return false,
            };
            !models.is_empty() && models.iter().any(|m| m == model_class_name)
        }
        None => false,
    }
}

/// Check if using Vertex AI from request extra fields
pub fn is_vertex_from_extra(extra: &JsonValue) -> bool {
    extra
        .get("vertex")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Extract system instruction from messages
/// Priority 1: Check extra_body.gemini_system_instruction
/// Priority 2: Extract from system messages
pub fn extract_system_instruction(
    messages: &[ChatMessage],
    extra: &HashMap<String, JsonValue>,
) -> Result<Option<JsonValue>> {
    // Priority 1: Check for gemini_system_instruction in extra_body (explicitly provided)
    if let Some(extra_body) = extra.get("extra_body") {
        if let Some(gemini_sys_inst) = extra_body.get("gemini_system_instruction") {
            debug!("✅ GEMINI: Using explicit gemini_system_instruction from extra_body");
            return Ok(Some(gemini_sys_inst.clone()));
        }
    }

    // Priority 2: If no explicit system instruction, extract from system messages
    let system_messages: Vec<&ChatMessage> =
        messages.iter().filter(|msg| msg.role == "system").collect();

    if system_messages.is_empty() {
        return Ok(None);
    }

    let mut system_instruction_parts = Vec::new();

    for msg in system_messages.iter() {
        let parts = convert_content_to_parts(&msg.content)?;
        // Extract text from parts and add to system instruction
        for part in parts {
            if let Some(text) = part.get("text") {
                system_instruction_parts.push(json!({ "text": text }));
            }
        }
    }

    if system_instruction_parts.is_empty() {
        warn!("⚠️  GEMINI: System messages found but no text parts extracted");
        Ok(None)
    } else {
        debug!(
            "✅ Auto-constructed gemini_system_instruction from {} system messages with {} parts",
            system_messages.len(),
            system_instruction_parts.len()
        );
        Ok(Some(json!({
            "parts": system_instruction_parts
        })))
    }
}

/// Convert messages to Gemini contents format (without system instruction extraction)
pub fn convert_messages_to_gemini_contents(
    messages: &[ChatMessage],
    skip_system: bool,
) -> Result<Vec<JsonValue>> {
    let mut gemini_contents = Vec::new();

    for message in messages {
        let role = match message.role.as_str() {
            "user" => "user",
            "assistant" | "model" => "model", // Handle both OpenAI "assistant" and Gemini "model"
            "system" => {
                if skip_system {
                    continue;
                }
                // Fallback: if system messages should be included, use "System: " prefix method
                let parts = convert_content_to_parts(&message.content)?;
                gemini_contents.push(json!({
                    "role": "user",
                    "parts": parts.into_iter().map(|mut part| {
                        if let Some(text) = part.get_mut("text") {
                            if let Some(text_str) = text.as_str() {
                                *text = JsonValue::String(format!("System: {}", text_str));
                            }
                        }
                        part
                    }).collect::<Vec<_>>()
                }));
                continue;
            }
            _ => "user", // Default unknown roles to user
        };

        let parts = convert_content_to_parts(&message.content)?;

        gemini_contents.push(json!({
            "role": role,
            "parts": parts
        }));
    }

    Ok(gemini_contents)
}

/// Convert OpenAI format messages to Google Gemini format with multimodal support
/// Returns (contents, system_instruction_option)
pub fn convert_messages_to_gemini_with_system(
    messages: &[ChatMessage],
    extra: &HashMap<String, JsonValue>,
) -> Result<(Vec<JsonValue>, Option<JsonValue>)> {
    let system_instruction = extract_system_instruction(messages, extra)?;
    let skip_system = system_instruction.is_some();
    let gemini_contents = convert_messages_to_gemini_contents(messages, skip_system)?;

    Ok((gemini_contents, system_instruction))
}

/// Convert OpenAI format messages to Google Gemini format (backward compatible)
pub fn convert_messages_to_gemini(messages: &[ChatMessage]) -> Result<Vec<JsonValue>> {
    let empty_extra = HashMap::new();
    let (contents, _) = convert_messages_to_gemini_with_system(messages, &empty_extra)?;
    Ok(contents)
}

/// Detect MIME type from URL (file extension or data URL)
fn detect_mime_type_from_url(url: &str, default_type: &str) -> String {
    // First, try to extract from data URL
    if url.starts_with("data:") {
        if let Some(comma_pos) = url.find(',') {
            let mime_and_encoding = &url[5..comma_pos];
            if let Some(mime_type) = mime_and_encoding.split(';').next() {
                if !mime_type.is_empty() {
                    return mime_type.to_string();
                }
            }
        }
    }

    // Otherwise, detect from file extension or URL pattern
    let url_lower = url.to_lowercase();

    // Image types
    if url_lower.ends_with(".png") || url_lower.contains("image/png") {
        return "image/png".to_string();
    }
    if url_lower.ends_with(".jpg")
        || url_lower.ends_with(".jpeg")
        || url_lower.contains("image/jpeg")
    {
        return "image/jpeg".to_string();
    }
    if url_lower.ends_with(".webp") || url_lower.contains("image/webp") {
        return "image/webp".to_string();
    }
    if url_lower.ends_with(".gif") || url_lower.contains("image/gif") {
        return "image/gif".to_string();
    }
    if url_lower.ends_with(".bmp") || url_lower.contains("image/bmp") {
        return "image/bmp".to_string();
    }
    if url_lower.ends_with(".svg") || url_lower.contains("image/svg") {
        return "image/svg+xml".to_string();
    }

    // Audio types
    if url_lower.ends_with(".mp3") || url_lower.contains("audio/mpeg") {
        return "audio/mpeg".to_string();
    }
    if url_lower.ends_with(".wav") || url_lower.contains("audio/wav") {
        return "audio/wav".to_string();
    }
    if url_lower.ends_with(".ogg") || url_lower.contains("audio/ogg") {
        return "audio/ogg".to_string();
    }
    if url_lower.ends_with(".m4a") || url_lower.contains("audio/mp4") {
        return "audio/mp4".to_string();
    }
    if url_lower.ends_with(".aac") || url_lower.contains("audio/aac") {
        return "audio/aac".to_string();
    }
    if url_lower.ends_with(".flac") || url_lower.contains("audio/flac") {
        return "audio/flac".to_string();
    }

    // Video types
    if url_lower.ends_with(".mp4") || url_lower.contains("video/mp4") {
        return "video/mp4".to_string();
    }
    if url_lower.ends_with(".webm") || url_lower.contains("video/webm") {
        return "video/webm".to_string();
    }
    if url_lower.ends_with(".mov") || url_lower.contains("video/quicktime") {
        return "video/quicktime".to_string();
    }
    if url_lower.ends_with(".avi") || url_lower.contains("video/x-msvideo") {
        return "video/x-msvideo".to_string();
    }

    // Default
    default_type.to_string()
}

/// Convert message content to Gemini parts (supporting multimodal)
fn convert_content_to_parts(content: &MessageContent) -> Result<Vec<JsonValue>> {
    let mut parts = Vec::new();

    match content {
        MessageContent::Text(text) => {
            parts.push(json!({"text": text}));
        }
        MessageContent::Parts(content_parts) => {
            for part in content_parts {
                match part {
                    ContentPart::Text { text } => {
                        parts.push(json!({"text": text}));
                    }
                    ContentPart::ImageUrl { image_url } => {
                        let url = &image_url.url;
                        if url.starts_with("data:") {
                            // Base64 inline data
                            if let Some(comma_pos) = url.find(',') {
                                let data = &url[comma_pos + 1..];
                                let mime_type = detect_mime_type_from_url(url, "image/jpeg");

                                debug!(
                                    "Image inline data detected - MIME: {}, data size: {} bytes",
                                    mime_type,
                                    data.len()
                                );

                                parts.push(json!({
                                    "inlineData": {
                                        "data": data,
                                        "mimeType": mime_type
                                    }
                                }));
                            }
                        } else {
                            // File URI
                            let mime_type = detect_mime_type_from_url(url, "image/jpeg");

                            debug!(
                                "Image file URI detected - URI: {}, MIME: {}",
                                url, mime_type
                            );

                            parts.push(json!({
                                "fileData": {
                                    "fileUri": url,
                                    "mimeType": mime_type
                                }
                            }));
                        }
                    }
                    ContentPart::InputAudio { input_audio } => {
                        let url = &input_audio.url;
                        if url.starts_with("data:") {
                            // Base64 inline data
                            if let Some(comma_pos) = url.find(',') {
                                let data = &url[comma_pos + 1..];
                                let mime_type = detect_mime_type_from_url(url, "audio/mpeg");

                                debug!(
                                    "Audio inline data detected - MIME: {}, data size: {} bytes",
                                    mime_type,
                                    data.len()
                                );

                                parts.push(json!({
                                    "inlineData": {
                                        "data": data,
                                        "mimeType": mime_type
                                    }
                                }));
                            }
                        } else {
                            // File URI
                            let mime_type = detect_mime_type_from_url(url, "audio/mpeg");

                            debug!(
                                "Audio file URI detected - URI: {}, MIME: {}",
                                url, mime_type
                            );

                            parts.push(json!({
                                "fileData": {
                                    "fileUri": url,
                                    "mimeType": mime_type
                                }
                            }));
                        }
                    }
                }
            }
        }
    }

    Ok(parts)
}

/// Convert Gemini response to OpenAI format
pub fn convert_gemini_to_openai_response(
    gemini_response: &str,
    original_model: &str,
) -> Result<ChatCompletionResponse> {
    let gemini_json: JsonValue = AdapterUtils::parse_response(gemini_response, "Gemini")?;
    AdapterUtils::check_api_error(&gemini_json, "Gemini")?;

    let candidates = gemini_json
        .get("candidates")
        .and_then(|c| c.as_array())
        .ok_or_else(|| {
            AppError::Internal(
                "Missing or invalid 'candidates' field in Gemini response".to_string(),
            )
        })?;

    let first_candidate = candidates
        .first()
        .ok_or_else(|| AppError::Internal("No candidates found in Gemini response".to_string()))?;

    let content = first_candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(|parts| parts.as_array())
        .and_then(|parts_arr| parts_arr.first())
        .and_then(|part| part.get("text"))
        .and_then(|text| text.as_str())
        .unwrap_or("");

    // Check finishReason for safety/content filtering (before normalization)
    let raw_finish_reason = first_candidate
        .get("finishReason")
        .and_then(|reason| reason.as_str());

    // Check for Gemini-specific content filter reasons
    if let Some(reason) = raw_finish_reason {
        match reason {
            "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" => {
                warn!("⚠️ Content blocked by Gemini safety filters: {}", reason);
                return Err(AppError::ContentFilter(format!(
                    "Response blocked by Gemini safety filters: {}",
                    reason
                )));
            }
            _ => {}
        }
    }

    // Check if content is empty when not STOP
    if content.is_empty() && raw_finish_reason != Some("STOP") {
        return Err(AppError::EmptyResponse(format!(
            "Gemini returned empty content (finishReason: {:?})",
            raw_finish_reason
        )));
    }

    let usage = gemini_json.get("usageMetadata").map(|usage_json| Usage {
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

    let finish_reason = raw_finish_reason
        .map(AdapterUtils::normalize_finish_reason)
        .unwrap_or("stop");

    Ok(AdapterUtils::create_chat_response(
        content,
        original_model,
        usage,
        Some(finish_reason),
    ))
}

/// Extract content and finish reason from Gemini candidate
pub fn extract_content_from_candidate(
    candidate_json: Option<&JsonValue>,
) -> (String, Option<String>) {
    let candidate = candidate_json
        .and_then(|json| json.get("candidates"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .unwrap_or(&JsonValue::Null);

    // Extract and concatenate all text parts (not just the first one)
    // Gemini may return multiple parts in a single response
    // Filter out parts with "thought": true to exclude thinking content
    let content = candidate
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

    let finish_reason = candidate
        .get("finishReason")
        .and_then(|reason| reason.as_str())
        .map(|s| s.to_string());

    (content, finish_reason)
}

/// Trait for extracting standard fields from requests
pub trait StandardFields {
    fn temperature(&self) -> Option<f32>;
    fn top_p(&self) -> Option<f32>;
    fn max_tokens(&self) -> Option<u32>;
    fn frequency_penalty(&self) -> Option<f32>;
    fn presence_penalty(&self) -> Option<f32>;
    fn stop(&self) -> Option<Vec<String>>;
    fn extra(&self) -> &HashMap<String, JsonValue>;
}

impl StandardFields for ChatCompletionRequest {
    fn temperature(&self) -> Option<f32> {
        self.temperature
    }
    fn top_p(&self) -> Option<f32> {
        self.top_p
    }
    fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }
    fn frequency_penalty(&self) -> Option<f32> {
        self.frequency_penalty
    }
    fn presence_penalty(&self) -> Option<f32> {
        self.presence_penalty
    }
    fn stop(&self) -> Option<Vec<String>> {
        self.stop.clone()
    }
    fn extra(&self) -> &HashMap<String, JsonValue> {
        &self.extra
    }
}

impl StandardFields for CompletionRequest {
    fn temperature(&self) -> Option<f32> {
        self.temperature
    }
    fn top_p(&self) -> Option<f32> {
        self.top_p
    }
    fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }
    fn frequency_penalty(&self) -> Option<f32> {
        self.frequency_penalty
    }
    fn presence_penalty(&self) -> Option<f32> {
        self.presence_penalty
    }
    fn stop(&self) -> Option<Vec<String>> {
        self.stop.clone()
    }
    fn extra(&self) -> &HashMap<String, JsonValue> {
        &self.extra
    }
}

/// Check if model is Gemini 2.5 or later (which doesn't support frequency_penalty/presence_penalty)
fn is_gemini_25_or_later(model: &str) -> bool {
    let model_lower = model.to_lowercase();
    // Match patterns like "gemini-2.5-pro", "gemini-2.5-flash", "gemini-3.0-xxx", etc.
    model_lower.contains("gemini-2.5") || model_lower.contains("gemini-3")
}

/// Build Google generation config from request fields
/// Merges standard fields with extra.config, allowing full GenerateContentConfig support
pub fn build_generation_config<T: StandardFields>(request: &T, model: &str) -> JsonValue {
    let mut config = json!({});

    // Check if this is Gemini 2.5+ (which doesn't support frequency_penalty/presence_penalty)
    let skip_penalty_params = is_gemini_25_or_later(model);
    if skip_penalty_params {
        debug!(
            "Gemini 2.5+ detected ({}), skipping frequency_penalty and presence_penalty",
            model
        );
    }

    // Standard fields -> config mapping (backward compatibility)
    if let Some(temp) = request.temperature() {
        config["temperature"] =
            JsonValue::Number(serde_json::Number::from_f64(temp as f64).unwrap());
    }
    if let Some(top_p) = request.top_p() {
        config["topP"] = JsonValue::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
    }
    if let Some(max_tokens) = request.max_tokens() {
        config["maxOutputTokens"] = JsonValue::Number(serde_json::Number::from(max_tokens));
    }
    if let Some(stop) = request.stop() {
        config["stopSequences"] =
            JsonValue::Array(stop.iter().map(|s| JsonValue::String(s.clone())).collect());
    }
    // Skip frequency_penalty and presence_penalty for Gemini 2.5+
    if !skip_penalty_params {
        if let Some(frequency_penalty) = request.frequency_penalty() {
            config["frequencyPenalty"] =
                JsonValue::Number(serde_json::Number::from_f64(frequency_penalty as f64).unwrap());
        }
        if let Some(presence_penalty) = request.presence_penalty() {
            config["presencePenalty"] =
                JsonValue::Number(serde_json::Number::from_f64(presence_penalty as f64).unwrap());
        }
    }

    // Merge extra.config if present (allows full GenerateContentConfig support)
    // extra.config overrides standard fields
    if let Some(extra_config) = request.extra().get("config") {
        if let Some(extra_config_obj) = extra_config.as_object() {
            let config_obj = config.as_object_mut().unwrap();
            for (key, value) in extra_config_obj {
                config_obj.insert(key.clone(), value.clone());
            }
        }
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // Note: These tests modify environment variables and should be run with --test-threads=1
    // to avoid race conditions: cargo test --lib google_common::tests -- --test-threads=1

    fn check_should_use(
        config: &Option<SystemInstructionConfig>,
        adapter: &str,
        model_class_name: &str,
    ) -> bool {
        match config {
            Some(cfg) => {
                let models = match adapter {
                    "google" => &cfg.google,
                    "google_proxy" => &cfg.google_proxy,
                    _ => return false,
                };
                !models.is_empty() && models.iter().any(|m| m == model_class_name)
            }
            None => false,
        }
    }

    #[test]
    fn test_parse_config_no_env() {
        env::remove_var("GEMINI_SYSTEM_INSTRUCTION_MODELS");
        let config = parse_system_instruction_config();
        assert!(config.is_none());
        assert!(!check_should_use(&config, "google", "any-model"));
    }

    #[test]
    fn test_parse_config_empty_arrays() {
        env::set_var(
            "GEMINI_SYSTEM_INSTRUCTION_MODELS",
            r#"{"google":[],"google_proxy":[]}"#,
        );
        let config = parse_system_instruction_config();
        assert!(config.is_some());
        assert!(!check_should_use(&config, "google", "any-model"));
        assert!(!check_should_use(&config, "google_proxy", "any-model"));
        env::remove_var("GEMINI_SYSTEM_INSTRUCTION_MODELS");
    }

    #[test]
    fn test_parse_config_with_models() {
        env::set_var(
            "GEMINI_SYSTEM_INSTRUCTION_MODELS",
            r#"{"google":["gemini-pro"],"google_proxy":["gemini-flash"]}"#,
        );
        let config = parse_system_instruction_config();
        assert!(config.is_some());
        assert!(check_should_use(&config, "google", "gemini-pro"));
        assert!(!check_should_use(&config, "google", "gemini-ultra"));
        assert!(check_should_use(&config, "google_proxy", "gemini-flash"));
        assert!(!check_should_use(&config, "google_proxy", "gemini-pro"));
        env::remove_var("GEMINI_SYSTEM_INSTRUCTION_MODELS");
    }

    #[test]
    fn test_parse_config_invalid_json() {
        env::set_var("GEMINI_SYSTEM_INSTRUCTION_MODELS", "invalid json");
        let config = parse_system_instruction_config();
        assert!(config.is_none());
        assert!(!check_should_use(&config, "google", "any-model"));
        env::remove_var("GEMINI_SYSTEM_INSTRUCTION_MODELS");
    }

    #[test]
    fn test_parse_config_multiple_models() {
        env::set_var(
            "GEMINI_SYSTEM_INSTRUCTION_MODELS",
            r#"{"google":["gemini-pro","gemini-ultra"],"google_proxy":["gemini-flash","gemini-nano"]}"#,
        );
        let config = parse_system_instruction_config();
        assert!(config.is_some());
        assert!(check_should_use(&config, "google", "gemini-pro"));
        assert!(check_should_use(&config, "google", "gemini-ultra"));
        assert!(!check_should_use(&config, "google", "gemini-flash"));
        assert!(check_should_use(&config, "google_proxy", "gemini-flash"));
        assert!(check_should_use(&config, "google_proxy", "gemini-nano"));
        assert!(!check_should_use(&config, "google_proxy", "gemini-pro"));
        env::remove_var("GEMINI_SYSTEM_INSTRUCTION_MODELS");
    }
}
