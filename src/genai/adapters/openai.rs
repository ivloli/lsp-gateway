use super::{EndpointType, ProviderAdapter};
use crate::error::Result;
use crate::genai::utils::AdapterUtils;
use crate::handlers::chat::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionRequest, CompletionResponse,
    ToProviderRequest,
};
use axum::http::HeaderMap;
use serde_json::Value as JsonValue;

pub struct OpenAIAdapter;

impl OpenAIAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for OpenAIAdapter {
    fn transform_chat_request(
        &self,
        request: &ChatCompletionRequest,
        target_model: &str,
        model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        Ok(request.to_provider_request(target_model, model_class_name))
    }

    fn transform_completion_request(
        &self,
        request: &CompletionRequest,
        target_model: &str,
        model_class_name: &str,
        _base_url: &str,
    ) -> Result<JsonValue> {
        Ok(request.to_provider_request(target_model, model_class_name))
    }

    fn transform_chat_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<ChatCompletionResponse> {
        let mut parsed_response: ChatCompletionResponse =
            AdapterUtils::parse_response(response, "OpenAI")?;
        parsed_response.model = original_model.to_string();
        Ok(parsed_response)
    }

    fn transform_completion_response(
        &self,
        response: &str,
        original_model: &str,
    ) -> Result<CompletionResponse> {
        let mut parsed_response: CompletionResponse =
            AdapterUtils::parse_response(response, "OpenAI")?;
        parsed_response.model = original_model.to_string();
        Ok(parsed_response)
    }

    fn get_headers(&self, api_key: &str, _base_url: &str, _request_extra: &JsonValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", api_key).parse().unwrap(),
        );
        headers.insert("content-type", "application/json".parse().unwrap());
        headers
    }

    fn provider_name(&self) -> &'static str {
        "openai"
    }

    fn get_endpoint_url(
        &self,
        base_url: &str,
        _model_name: &str,
        endpoint_type: EndpointType,
        _api_key: Option<&str>,
        request_extra: &JsonValue,
    ) -> String {
        let clean_base = base_url.trim_end_matches('/');

        // Check if skipNormalize is set to true in parameters
        let skip_normalize = request_extra
            .get("skipNormalize")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let base_path = if skip_normalize {
            // Skip adding /v1, use base_url as-is
            clean_base.to_string()
        } else if clean_base.ends_with("/v1") {
            clean_base.to_string()
        } else {
            format!("{}/v1", clean_base)
        };

        match endpoint_type {
            EndpointType::Chat | EndpointType::ChatStream => {
                format!("{}/chat/completions", base_path)
            }
            EndpointType::Completion | EndpointType::CompletionStream => {
                format!("{}/completions", base_path)
            }
        }
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        original_model: &str,
        is_completion: bool,
    ) -> Result<String> {
        // Process chunk to ensure proper SSE format and model field replacement
        AdapterUtils::process_stream_chunk(chunk, original_model, is_completion)
    }
}
