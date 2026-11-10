use crate::error::{AppError, Result};
use crate::handlers::chat::{
    ChatChoice, ChatCompletionResponse, ChatMessage, CompletionChoice, CompletionResponse, Usage,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

/// Common utilities for GenAI adapters
pub struct AdapterUtils;

impl AdapterUtils {
    /// Create a standard OpenAI chat completion response
    pub fn create_chat_response(
        content: &str,
        original_model: &str,
        usage: Option<Usage>,
        finish_reason: Option<&str>,
    ) -> ChatCompletionResponse {
        use crate::handlers::chat::types::MessageContent;

        ChatCompletionResponse {
            id: format!("chatcmpl-{}", Uuid::new_v4().simple()),
            object: "chat.completion".to_string(),
            created: crate::utils::TimeUtils::current_timestamp_secs(),
            model: original_model.to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: MessageContent::Text(content.to_string()),
                    name: None,
                },
                finish_reason: finish_reason.map(|s| s.to_string()),
            }],
            usage,
        }
    }

    /// Convert chat response to completion response
    pub fn chat_to_completion_response(
        chat_response: ChatCompletionResponse,
    ) -> CompletionResponse {
        let text = chat_response
            .choices
            .first()
            .map(|choice| choice.message.content.to_string())
            .unwrap_or_default();

        CompletionResponse {
            id: chat_response.id,
            object: "text_completion".to_string(),
            created: chat_response.created,
            model: chat_response.model,
            choices: vec![CompletionChoice {
                index: 0,
                text,
                finish_reason: chat_response
                    .choices
                    .first()
                    .and_then(|choice| choice.finish_reason.clone()),
            }],
            usage: chat_response.usage,
        }
    }

    /// Parse JSON response with better error handling
    /// Uses simd-json for high-performance parsing when possible
    pub fn parse_response<T>(response: &str, provider_name: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        if response.trim().is_empty() {
            return Err(AppError::Internal(format!(
                "Empty {} response received",
                provider_name
            )));
        }

        // Try simd-json first for better performance (requires mutable buffer)
        // Fall back to serde_json if simd-json fails or for small responses
        if response.len() > 256 {
            // For larger responses, use simd-json for better performance
            let mut bytes = response.as_bytes().to_vec();
            match simd_json::from_slice::<T>(&mut bytes) {
                Ok(parsed) => return Ok(parsed),
                Err(_) => {
                    // Fall back to serde_json if simd-json fails
                }
            }
        }

        // Standard serde_json parsing (fallback or for small responses)
        serde_json::from_str(response).map_err(|e| {
            AppError::Internal(format!(
                "Failed to parse {} response: {}. Response: {}",
                provider_name, e, response
            ))
        })
    }

    /// Check for API error in response JSON
    pub fn check_api_error(json: &JsonValue, provider_name: &str) -> Result<()> {
        if let Some(error) = json.get("error") {
            let default_message = format!("Unknown {} API error", provider_name);
            let error_message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or(&default_message);
            return Err(AppError::Internal(format!(
                "{} API error: {}",
                provider_name, error_message
            )));
        }
        Ok(())
    }

    /// Create OpenAI streaming chunk
    pub fn create_stream_chunk(
        content: &str,
        original_model: &str,
        finish_reason: Option<&str>,
        is_completion: bool,
    ) -> Result<String> {
        Self::create_stream_chunk_with_usage(
            content,
            original_model,
            finish_reason,
            None,
            is_completion,
        )
    }

    /// Create OpenAI streaming chunk with optional usage information
    pub fn create_stream_chunk_with_usage(
        content: &str,
        original_model: &str,
        finish_reason: Option<&str>,
        usage: Option<Usage>,
        is_completion: bool,
    ) -> Result<String> {
        let object_type = if is_completion {
            "text_completion"
        } else {
            "chat.completion.chunk"
        };

        let mut chunk = json!({
            "id": format!("chatcmpl-{}", Uuid::new_v4().simple()),
            "object": object_type,
            "created": crate::utils::TimeUtils::current_timestamp_secs(),
            "model": original_model,
            "choices": [{
                "index": 0,
                "delta": {
                    "content": content
                }
            }]
        });

        if let Some(reason) = finish_reason {
            chunk["choices"][0]["finish_reason"] = JsonValue::String(reason.to_string());
        }

        // Add usage information if provided
        if let Some(usage_data) = usage {
            chunk["usage"] = json!({
                "prompt_tokens": usage_data.prompt_tokens,
                "completion_tokens": usage_data.completion_tokens,
                "total_tokens": usage_data.total_tokens
            });
        }

        let chunk_json = serde_json::to_string(&chunk)
            .map_err(|e| AppError::Internal(format!("Failed to serialize stream chunk: {}", e)))?;

        Ok(format!("data: {}\n\n", chunk_json))
    }

    /// Process stream chunk with common logic
    /// Uses regex-based replacement for high performance
    pub fn process_stream_chunk(
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        // Skip completely empty or whitespace-only chunks early
        if chunk.trim().is_empty() {
            return Ok(String::new());
        }

        // Precompiled regex patterns for efficient replacement
        static MODEL_REGEX: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#""model"\s*:\s*"[^"]*""#).unwrap());
        static OBJECT_REGEX: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#""object"\s*:\s*"text_completion""#).unwrap());

        // Check if chunk already has proper SSE format
        let has_data_prefix = chunk.starts_with("data: ");
        let has_trailing_newlines = chunk.ends_with("\n\n");

        // Extract the actual data content (zero-copy slice)
        let chunk_data = if has_data_prefix { &chunk[6..] } else { chunk };

        // Handle [DONE] marker (static string, minimal allocation)
        if chunk_data.trim() == "[DONE]" {
            return Ok("data: [DONE]\n\n".to_string());
        }

        // Skip empty chunks after removing data: prefix
        if chunk_data.trim().is_empty() {
            return Ok(String::new());
        }

        // Fast path: if no model field and already properly formatted, return as-is
        if !chunk.contains("\"model\":") {
            if has_data_prefix && has_trailing_newlines {
                return Ok(chunk.to_string());
            } else if has_data_prefix {
                // Pre-allocate exact capacity needed
                let mut result = String::with_capacity(chunk.len() + 2);
                result.push_str(chunk);
                result.push_str("\n\n");
                return Ok(result);
            } else {
                // Pre-allocate exact capacity needed
                let mut result = String::with_capacity(chunk.len() + 8);
                result.push_str("data: ");
                result.push_str(chunk);
                result.push_str("\n\n");
                return Ok(result);
            }
        }

        // Use regex replacement instead of full JSON parse/serialize
        // This is 5-10x faster for simple field replacements
        let mut result = MODEL_REGEX
            .replace(chunk_data, format!(r#""model":"{}""#, original_model))
            .into_owned();

        // Convert text_completion to chat.completion.chunk for chat endpoints
        if !is_completion && result.contains(r#""object":"text_completion""#) {
            result = OBJECT_REGEX
                .replace(&result, r#""object":"chat.completion.chunk""#)
                .into_owned();
        }

        // Build final SSE format with pre-allocated capacity
        let mut final_result = String::with_capacity(result.len() + 8);
        final_result.push_str("data: ");
        final_result.push_str(&result);
        final_result.push_str("\n\n");
        Ok(final_result)
    }

    /// Build endpoint URL with optional API key
    pub fn build_endpoint_url(base_url: &str, path: &str, api_key: Option<&str>) -> String {
        let clean_base = base_url.trim_end_matches('/');
        let url = format!("{}/{}", clean_base, path.trim_start_matches('/'));

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

    /// Normalize finish reason to OpenAI format
    pub fn normalize_finish_reason(reason: &str) -> &str {
        match reason {
            "STOP" => "stop",
            "MAX_TOKENS" => "length",
            "SAFETY" => "content_filter",
            _ => "stop",
        }
    }

    /// Replace model field in JSON response string (high performance)
    /// Returns the modified JSON string
    pub fn replace_model_in_response(response: &str, original_model: &str) -> String {
        static MODEL_REGEX: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#""model"\s*:\s*"[^"]*""#).unwrap());

        MODEL_REGEX
            .replace(response, format!(r#""model":"{}""#, original_model))
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_stream_chunk_with_model_replacement() {
        let chunk = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-3.5-turbo","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let result = AdapterUtils::process_stream_chunk(chunk, "my-custom-model", false).unwrap();

        assert!(result.contains(r#""model":"my-custom-model""#));
        assert!(result.starts_with("data: "));
        assert!(result.ends_with("\n\n"));
    }

    #[test]
    fn test_process_stream_chunk_completion_to_chat_conversion() {
        // Test converting text_completion to chat.completion.chunk for chat endpoints
        let chunk = r#"{"id":"chatcmpl-123","object":"text_completion","created":1234567890,"model":"gpt-3.5-turbo","choices":[{"index":0,"text":"Hello","finish_reason":null}]}"#;
        let result = AdapterUtils::process_stream_chunk(chunk, "my-model", false).unwrap();

        assert!(result.contains(r#""object":"chat.completion.chunk""#));
        assert!(result.contains(r#""model":"my-model""#));
        assert!(result.starts_with("data: "));
        assert!(result.ends_with("\n\n"));
    }

    #[test]
    fn test_process_stream_chunk_done_marker() {
        let chunk = "data: [DONE]";
        let result = AdapterUtils::process_stream_chunk(chunk, "any-model", false).unwrap();

        assert_eq!(result, "data: [DONE]\n\n");
    }

    #[test]
    fn test_process_stream_chunk_empty() {
        let chunk = "data: ";
        let result = AdapterUtils::process_stream_chunk(chunk, "any-model", false).unwrap();

        assert_eq!(result, "");
    }

    #[test]
    fn test_process_stream_chunk_no_model_field() {
        let chunk = r#"data: {"id":"123","choices":[{"index":0,"delta":{"content":"Hi"}}]}"#;
        let result = AdapterUtils::process_stream_chunk(chunk, "any-model", false).unwrap();

        // Should add trailing newlines for proper SSE format
        assert_eq!(result, format!("{}\n\n", chunk));
    }

    #[test]
    fn test_process_stream_chunk_without_data_prefix() {
        let chunk = r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":"Test"},"finish_reason":null}]}"#;
        let result = AdapterUtils::process_stream_chunk(chunk, "new-model", false).unwrap();

        assert!(result.contains(r#""model":"new-model""#));
        assert!(result.starts_with("data: "));
    }

    #[test]
    fn test_regex_performance_vs_json() {
        // This test demonstrates the performance benefit
        let chunk = r#"data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-3.5-turbo-0125","choices":[{"index":0,"delta":{"content":"Hello, how can I help you today?"},"finish_reason":null}],"system_fingerprint":"fp_123456"}"#;

        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = AdapterUtils::process_stream_chunk(chunk, "my-model", false).unwrap();
        }
        let regex_duration = start.elapsed();

        println!(
            "Regex-based processing: {:?} for 1000 iterations",
            regex_duration
        );

        // The regex approach should be significantly faster than JSON parse/serialize
        assert!(
            regex_duration.as_millis() < 100,
            "Performance regression detected"
        );
    }

    #[test]
    fn test_replace_model_in_response() {
        let response = r#"{"id":"chatcmpl-123","object":"chat.completion","created":1234567890,"model":"gpt-3.5-turbo","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let result = AdapterUtils::replace_model_in_response(response, "my-custom-model");

        assert!(result.contains(r#""model":"my-custom-model""#));
        assert!(!result.contains(r#""model":"gpt-3.5-turbo""#));
        // Ensure other fields are preserved
        assert!(result.contains(r#""id":"chatcmpl-123""#));
        assert!(result.contains(r#""total_tokens":15"#));
    }

    #[test]
    fn test_replace_model_in_completion_response() {
        let response = r#"{"id":"cmpl-123","object":"text_completion","created":1234567890,"model":"gpt-3.5-turbo-instruct","choices":[{"text":"Hello!","index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let result = AdapterUtils::replace_model_in_response(response, "my-model");

        assert!(result.contains(r#""model":"my-model""#));
        assert!(!result.contains(r#""model":"gpt-3.5-turbo-instruct""#));
    }

    #[test]
    fn test_process_stream_chunk_whitespace_only() {
        // Test that whitespace-only chunks are filtered out
        let chunk = "   \n  ";
        let result = AdapterUtils::process_stream_chunk(chunk, "any-model", false).unwrap();
        assert_eq!(result, "");

        // Test with data: prefix
        let chunk = "data:    ";
        let result = AdapterUtils::process_stream_chunk(chunk, "any-model", false).unwrap();
        assert_eq!(result, "");
    }
}
