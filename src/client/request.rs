use axum::http::HeaderMap;
use serde_json::Value as JsonValue;

/// Unified request type for different API endpoints
/// Now holds pre-transformed JSON payloads instead of original request objects
#[derive(Debug)]
pub enum ApiRequest {
    ChatCompletion(JsonValue),
    Completion(JsonValue),
}

/// Request configuration
#[derive(Debug)]
pub struct RequestConfig {
    pub base_url: String,
    pub model_name: String,
    pub headers: HeaderMap,
    pub is_streaming: bool,
    pub model_config_id: String,
}

/// Response type for different endpoints
pub enum ApiResponse {
    ChatCompletion(crate::handlers::chat::ChatCompletionResponse),
    Completion(crate::handlers::chat::CompletionResponse),
    Stream(crate::handlers::chat::response::StreamResponse),
}

impl ApiRequest {
    /// Get the pre-transformed JSON payload
    pub fn get_json_payload(&self) -> &JsonValue {
        match self {
            ApiRequest::ChatCompletion(json) => json,
            ApiRequest::Completion(json) => json,
        }
    }

    /// Get the endpoint path for this request type
    pub fn endpoint_path(&self) -> &'static str {
        match self {
            ApiRequest::ChatCompletion(_) => "/chat/completions",
            ApiRequest::Completion(_) => "/completions",
        }
    }

    /// Get the request type name for logging
    pub fn request_type(&self) -> &'static str {
        match self {
            ApiRequest::ChatCompletion(_) => "chat_completion",
            ApiRequest::Completion(_) => "completion",
        }
    }

    /// Inject a field into the JSON payload
    /// Returns a new ApiRequest with the field added to the root object
    pub fn with_injected_field(self, key: &str, value: JsonValue) -> Self {
        match self {
            ApiRequest::ChatCompletion(mut json) => {
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(key.to_string(), value);
                }
                ApiRequest::ChatCompletion(json)
            }
            ApiRequest::Completion(mut json) => {
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(key.to_string(), value);
                }
                ApiRequest::Completion(json)
            }
        }
    }
}

impl RequestConfig {
    pub fn new(
        base_url: String,
        model_name: String,
        headers: HeaderMap,
        is_streaming: bool,
        model_config_id: String,
    ) -> Self {
        Self {
            base_url,
            model_name,
            headers,
            is_streaming,
            model_config_id,
        }
    }
}
