pub mod anthropic;
pub mod google;
pub mod google_common;
pub mod google_proxy;
pub mod openai;

use crate::error::Result;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionRequest, CompletionResponse,
};
use axum::http::HeaderMap;
use serde_json::Value as JsonValue;

pub use anthropic::AnthropicAdapter;
pub use google::GoogleAdapter;
pub use google_proxy::GoogleProxyAdapter;
pub use openai::OpenAIAdapter;

/// Simplified trait for provider-specific request/response handling
#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// Transform chat completion request for the specific provider
    fn transform_chat_request(
        &self,
        request: &ChatCompletionRequest,
        target_model: &str,
        model_class_name: &str,
        base_url: &str,
    ) -> Result<JsonValue>;

    /// Transform completion request for the specific provider
    fn transform_completion_request(
        &self,
        request: &CompletionRequest,
        target_model: &str,
        model_class_name: &str,
        base_url: &str,
    ) -> Result<JsonValue>;

    /// Transform chat completion response from the provider
    fn transform_chat_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse>;

    /// Transform completion response from the provider
    fn transform_completion_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<CompletionResponse>;

    /// Get provider-specific headers with base URL context
    /// Returns headers to be added/merged with existing headers
    fn get_headers(&self, api_key: &str, base_url: &str, request_extra: &JsonValue) -> HeaderMap;

    /// Process and modify existing headers (e.g., remove conflicting auth headers)
    /// Default implementation does nothing
    fn process_headers(
        &self,
        headers: &mut HeaderMap,
        _base_url: &str,
        _request_extra: &JsonValue,
    ) {
        // Default: no processing
        let _ = headers;
    }

    /// Get the provider name
    fn provider_name(&self) -> &'static str;

    /// Get the complete endpoint URL with API key support
    fn get_endpoint_url(
        &self,
        base_url: &str,
        model_name: &str,
        endpoint_type: EndpointType,
        api_key: Option<&str>,
        request_extra: &JsonValue,
    ) -> String;

    /// Transform streaming chunk from provider format to OpenAI format
    fn transform_stream_chunk(
        &self,
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String>;
}

/// Endpoint types for different operations
#[derive(Debug, Clone, Copy)]
pub enum EndpointType {
    Chat,
    Completion,
    ChatStream,
    CompletionStream,
}
