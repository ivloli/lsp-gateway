use super::adapters::{
    AnthropicAdapter, EndpointType, GoogleAdapter, GoogleProxyAdapter, OpenAIAdapter,
    ProviderAdapter,
};
use super::utils::AdapterUtils;
use super::ProviderType;
use crate::client::{request::ApiRequest, LlmClient};
use crate::error::{AppError, Result};
use crate::handlers::chat::{ChatCompletionRequest, CompletionRequest};
use crate::routing::provider::SelectedProvider;
use axum::http::HeaderMap;
use tracing::{debug, error, info, warn};

/// GenAI client that provides a unified interface for different AI providers
pub struct GenAIClient {
    provider_type: ProviderType,
    llm_client: LlmClient,
    selected_provider: SelectedProvider,
    adapter: Box<dyn ProviderAdapter>,
    /// Stream no-token timeout in milliseconds (for passing to genai-proxy)
    stream_no_token_timeout_ms: u64,
    /// Response finish timeout in milliseconds (for passing to genai-proxy)
    response_finish_timeout_ms: u64,
}

impl GenAIClient {
    /// Get provider-specific headers
    fn get_provider_headers(
        &self,
        mut headers: HeaderMap,
        request_extra: &serde_json::Value,
    ) -> HeaderMap {
        // First, let adapter process/clean existing headers
        self.adapter.process_headers(
            &mut headers,
            &self.selected_provider.base_url,
            request_extra,
        );

        if let Some(api_key) = self.llm_client.get_api_key() {
            let adapter_headers =
                self.adapter
                    .get_headers(api_key, &self.selected_provider.base_url, request_extra);

            for (key, value) in adapter_headers.iter() {
                headers.insert(key, value.clone());
            }

            debug!(
                "Added provider-specific headers for {}",
                self.adapter.provider_name()
            );
        } else {
            warn!("No API key available for provider-specific headers");
        }

        headers
    }

    /// Create adapter for the provider type
    fn create_adapter(provider_type: &ProviderType) -> Box<dyn ProviderAdapter> {
        match provider_type {
            ProviderType::OpenAI => Box::new(OpenAIAdapter::new()),
            ProviderType::Anthropic => Box::new(AnthropicAdapter::new()),
            ProviderType::Google => {
                // 根据环境变量选择使用 Direct 还是 Proxy
                if std::env::var("GOOGLE_PROXY_URL").is_ok() {
                    debug!("Using GoogleProxyAdapter (genai-proxy)");
                    Box::new(GoogleProxyAdapter::new())
                } else {
                    debug!("Using GoogleDirectAdapter (direct API call)");
                    Box::new(GoogleAdapter::new())
                }
            }
            ProviderType::Unknown => {
                warn!("Unknown provider type, falling back to OpenAI adapter");
                Box::new(OpenAIAdapter::new())
            }
        }
    }

    /// Create a new GenAI client based on the model class name and selected provider
    pub fn new(selected_provider: SelectedProvider) -> Result<Self> {
        Self::with_timeout_config(
            selected_provider,
            crate::gateway::StreamTimeoutConfig::default(),
        )
    }

    /// Create a GenAI client with timeout configuration
    pub fn with_timeout_config(
        selected_provider: SelectedProvider,
        timeout_config: crate::gateway::StreamTimeoutConfig,
    ) -> Result<Self> {
        let provider_type =
            ProviderType::from_model_class_name(&selected_provider.model_config.model_class_name);

        info!(
            "🤖 Creating GenAI client for provider type: {:?} (model_class_name: {})",
            provider_type, selected_provider.model_config.model_class_name
        );

        let stream_no_token_timeout_ms = timeout_config.stream_no_token_timeout_ms;
        let response_finish_timeout_ms = timeout_config.response_finish_timeout_ms;
        let adapter = Self::create_adapter(&provider_type);
        let llm_client = LlmClient::with_selected_provider(&selected_provider, timeout_config)?;

        Ok(Self {
            provider_type,
            llm_client,
            selected_provider,
            adapter,
            stream_no_token_timeout_ms,
            response_finish_timeout_ms,
        })
    }

    /// Execute a chat completion request
    pub async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
        headers: HeaderMap,
    ) -> Result<(String, Option<u64>)> {
        let model_class_name = &self.selected_provider.model_config.model_class_name;
        let transformed_request = self.adapter.transform_chat_request(
            &request,
            &self.selected_provider.override_model_name,
            model_class_name,
            &self.selected_provider.base_url,
        )?;
        let mut api_request = ApiRequest::ChatCompletion(transformed_request);

        // 🔑 For Google provider (genai-proxy), inject timeout parameters into the request
        if matches!(self.provider_type, ProviderType::Google) {
            api_request = api_request
                .with_injected_field(
                    "streamNoTokenTimeoutMs",
                    serde_json::json!(self.stream_no_token_timeout_ms),
                )
                .with_injected_field(
                    "responseFinishTimeoutMs",
                    serde_json::json!(self.response_finish_timeout_ms),
                );
            debug!("Injected timeouts into non-streaming genai-proxy request: streamNoTokenTimeoutMs={}ms, responseFinishTimeoutMs={}ms", 
                self.stream_no_token_timeout_ms, self.response_finish_timeout_ms);
        }

        let request_extra = serde_json::to_value(&request.extra).unwrap_or_default();
        let provider_headers = self.get_provider_headers(headers, &request_extra);
        let api_key = self.llm_client.get_api_key();
        let endpoint_url = self.adapter.get_endpoint_url(
            &self.selected_provider.base_url,
            &self.selected_provider.override_model_name,
            EndpointType::Chat,
            api_key,
            &request_extra,
        );

        // 🔍 DEBUG: Log outgoing request details
        debug!(
            "🚀 [OUTGOING_REQUEST_CHAT] provider={}, model={}, url={}, body={}",
            self.selected_provider.id,
            self.selected_provider.override_model_name,
            endpoint_url,
            api_request.get_json_payload()
        );

        let response_text = self
            .llm_client
            .chat_completions_raw_response(
                &endpoint_url,
                &self.selected_provider.override_model_name,
                api_request,
                provider_headers,
                self.selected_provider.model_config.id.clone(),
            )
            .await?;

        // DEBUG: Log the raw response from LLM
        debug!(
            provider = ?self.provider_type,
            model = %self.selected_provider.override_model_name,
            response_length = response_text.len(),
            "📤 [DEBUG] Raw response from LLM (chat completion): {}",
            response_text
        );

        // Optimization: For OpenAI provider, avoid parse→serialize cycle
        if matches!(self.provider_type, ProviderType::OpenAI) {
            // Fast path: Replace model field directly in JSON string
            let final_response_json =
                AdapterUtils::replace_model_in_response(&response_text, &request.model);

            // Extract token count without full parsing
            let token_count =
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response_text) {
                    json.get("usage")
                        .and_then(|u| u.get("total_tokens"))
                        .and_then(|t| t.as_u64())
                } else {
                    None
                };

            Ok((final_response_json, token_count))
        } else {
            // Slow path: Full transformation for other providers
            let openai_response = self
                .adapter
                .transform_chat_response(&response_text, &request.model)?;
            let final_response_json = serde_json::to_string(&openai_response).map_err(|e| {
                AppError::Internal(format!("Failed to serialize final response: {}", e))
            })?;

            let token_count = openai_response.usage.map(|usage| usage.total_tokens as u64);
            Ok((final_response_json, token_count))
        }
    }

    /// Execute a text completion request
    pub async fn text_completion(
        &self,
        request: CompletionRequest,
        headers: HeaderMap,
    ) -> Result<(String, Option<u64>)> {
        let model_class_name = &self.selected_provider.model_config.model_class_name;
        let transformed_request = self.adapter.transform_completion_request(
            &request,
            &self.selected_provider.override_model_name,
            model_class_name,
            &self.selected_provider.base_url,
        )?;
        let mut api_request = ApiRequest::Completion(transformed_request);

        // 🔑 For Google provider (genai-proxy), inject timeout parameters into the request
        if matches!(self.provider_type, ProviderType::Google) {
            api_request = api_request
                .with_injected_field(
                    "streamNoTokenTimeoutMs",
                    serde_json::json!(self.stream_no_token_timeout_ms),
                )
                .with_injected_field(
                    "responseFinishTimeoutMs",
                    serde_json::json!(self.response_finish_timeout_ms),
                );
            debug!("Injected timeouts into non-streaming text completion genai-proxy request: streamNoTokenTimeoutMs={}ms, responseFinishTimeoutMs={}ms", 
                self.stream_no_token_timeout_ms, self.response_finish_timeout_ms);
        }

        let request_extra = serde_json::to_value(&request.extra).unwrap_or_default();
        let provider_headers = self.get_provider_headers(headers, &request_extra);
        let api_key = self.llm_client.get_api_key();
        let endpoint_url = self.adapter.get_endpoint_url(
            &self.selected_provider.base_url,
            &self.selected_provider.override_model_name,
            EndpointType::Completion,
            api_key,
            &request_extra,
        );

        // 🔍 DEBUG: Log outgoing request details
        debug!(
            "🚀 [OUTGOING_REQUEST_COMPLETION] provider={}, model={}, url={}, body={}",
            self.selected_provider.id,
            self.selected_provider.override_model_name,
            endpoint_url,
            api_request.get_json_payload()
        );

        let response_text = self
            .llm_client
            .completions_raw_response(
                &endpoint_url,
                &self.selected_provider.override_model_name,
                api_request,
                provider_headers,
                self.selected_provider.model_config.id.clone(),
            )
            .await?;

        // DEBUG: Log the raw response from LLM
        debug!(
            provider = ?self.provider_type,
            model = %self.selected_provider.override_model_name,
            response_length = response_text.len(),
            "📤 [DEBUG] Raw response from LLM (text completion): {}",
            response_text
        );

        // Optimization: For OpenAI provider, avoid parse→serialize cycle
        if matches!(self.provider_type, ProviderType::OpenAI) {
            // Fast path: Replace model field directly in JSON string
            let final_response_json =
                AdapterUtils::replace_model_in_response(&response_text, &request.model);

            // Extract token count without full parsing
            let token_count =
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response_text) {
                    json.get("usage")
                        .and_then(|u| u.get("total_tokens"))
                        .and_then(|t| t.as_u64())
                } else {
                    None
                };

            Ok((final_response_json, token_count))
        } else {
            // Slow path: Full transformation for other providers
            let openai_response = self
                .adapter
                .transform_completion_response(&response_text, &request.model)?;
            let final_response_json = serde_json::to_string(&openai_response).map_err(|e| {
                AppError::Internal(format!(
                    "Failed to serialize final completion response: {}",
                    e
                ))
            })?;

            let token_count = openai_response.usage.map(|usage| usage.total_tokens as u64);
            Ok((final_response_json, token_count))
        }
    }

    /// Execute a streaming chat completion request
    pub async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
        headers: HeaderMap,
    ) -> Result<crate::handlers::chat::response::StreamResponse> {
        let model_class_name = self.selected_provider.model_config.model_class_name.clone();
        let request_extra = serde_json::to_value(&request.extra).unwrap_or_default();
        self.stream_request(
            request.model.clone(),
            EndpointType::ChatStream,
            headers,
            &request_extra,
            move |adapter, target_model, base_url| {
                let transformed_request = adapter.transform_chat_request(
                    &request,
                    target_model,
                    &model_class_name,
                    base_url,
                )?;
                Ok(ApiRequest::ChatCompletion(transformed_request))
            },
            false,
        )
        .await
    }

    /// Execute a streaming text completion request
    pub async fn text_completion_stream(
        &self,
        request: CompletionRequest,
        headers: HeaderMap,
    ) -> Result<crate::handlers::chat::response::StreamResponse> {
        let model_class_name = self.selected_provider.model_config.model_class_name.clone();
        let request_extra = serde_json::to_value(&request.extra).unwrap_or_default();
        self.stream_request(
            request.model.clone(),
            EndpointType::CompletionStream,
            headers,
            &request_extra,
            move |adapter, target_model, base_url| {
                let transformed_request = adapter.transform_completion_request(
                    &request,
                    target_model,
                    &model_class_name,
                    base_url,
                )?;
                Ok(ApiRequest::Completion(transformed_request))
            },
            true,
        )
        .await
    }

    /// Common streaming request handler
    async fn stream_request<F>(
        &self,
        original_model: String,
        endpoint_type: EndpointType,
        headers: HeaderMap,
        request_extra: &serde_json::Value,
        request_builder: F,
        is_completion: bool,
    ) -> Result<crate::handlers::chat::response::StreamResponse>
    where
        F: FnOnce(&dyn ProviderAdapter, &str, &str) -> Result<ApiRequest>,
    {
        let mut api_request = request_builder(
            self.adapter.as_ref(),
            &self.selected_provider.override_model_name,
            &self.selected_provider.base_url,
        )?;

        // 🔑 For Google provider (genai-proxy), inject timeout parameters into the request
        if matches!(self.provider_type, ProviderType::Google) {
            api_request = api_request
                .with_injected_field(
                    "streamNoTokenTimeoutMs",
                    serde_json::json!(self.stream_no_token_timeout_ms),
                )
                .with_injected_field(
                    "responseFinishTimeoutMs",
                    serde_json::json!(self.response_finish_timeout_ms),
                );
            debug!("Injected streamNoTokenTimeoutMs={}ms, responseFinishTimeoutMs={}ms into genai-proxy request", 
                self.stream_no_token_timeout_ms, self.response_finish_timeout_ms);
        }

        let provider_headers = self.get_provider_headers(headers, request_extra);
        let api_key = self.llm_client.get_api_key();
        let endpoint_url = self.adapter.get_endpoint_url(
            &self.selected_provider.base_url,
            &self.selected_provider.override_model_name,
            endpoint_type,
            api_key,
            request_extra,
        );

        // 🔍 DEBUG: Log outgoing streaming request details
        debug!(
            "🚀 [OUTGOING_REQUEST_STREAM] provider={}, model={}, endpoint_type={:?}, url={}, body={}",
            self.selected_provider.id,
            self.selected_provider.override_model_name,
            endpoint_type,
            endpoint_url,
            api_request.get_json_payload()
        );

        let raw_stream = match endpoint_type {
            EndpointType::ChatStream => {
                self.llm_client
                    .chat_completions_stream_with_api_request(
                        &endpoint_url,
                        &self.selected_provider.override_model_name,
                        api_request,
                        provider_headers,
                        self.selected_provider.model_config.id.clone(),
                    )
                    .await?
            }
            EndpointType::CompletionStream => {
                self.llm_client
                    .completions_stream_with_api_request(
                        &endpoint_url,
                        &self.selected_provider.override_model_name,
                        api_request,
                        provider_headers,
                        self.selected_provider.model_config.id.clone(),
                    )
                    .await?
            }
            _ => {
                return Err(AppError::Internal(
                    "Invalid endpoint type for streaming".to_string(),
                ))
            }
        };

        // 🚀 OpenAI Fast Path: Skip transformation for OpenAI provider
        // OpenAI responses are already in the correct format, no need to parse/transform each chunk
        if matches!(self.provider_type, ProviderType::OpenAI) {
            debug!("🚀 OpenAI fast path: skipping stream transformation");
            return Ok(raw_stream);
        }

        // For non-OpenAI providers, apply transformation
        let provider_type = self.provider_type.clone();
        use futures_util::StreamExt;
        use std::sync::Arc;

        let original_model = Arc::new(original_model);

        // Create adapter once and share it across all chunks using Arc
        let adapter = Arc::new(Self::create_adapter(&provider_type));

        let transformed_stream = raw_stream.filter_map(move |chunk_result: std::result::Result<bytes::Bytes, reqwest::Error>| {
            let original_model = Arc::clone(&original_model);
            let adapter = Arc::clone(&adapter);
            async move {
                match chunk_result {
                    Ok(chunk_bytes) => {
                        let chunk_str = match std::str::from_utf8(&chunk_bytes) {
                            Ok(s) => s,
                            Err(e) => {
                                error!("❌ Invalid UTF-8 in stream chunk: {:?}, attempting lossy conversion", e);
                                // 使用有损转换，避免内容完全丢失
                                return Some(Ok(bytes::Bytes::from(String::from_utf8_lossy(&chunk_bytes).to_string())));
                            }
                        };

                        match adapter.transform_stream_chunk(chunk_str, &original_model, is_completion) {
                            Ok(transformed_chunk) => {
                                // Don't filter out any chunks (including whitespace)
                                // to preserve formatting like spaces and newlines
                                debug!("✅ Transformed chunk length: {}", transformed_chunk.len());
                                Some(Ok(bytes::Bytes::from(transformed_chunk)))
                            }
                            Err(e) => {
                                // ⚠️ 转换失败：不返回原始 chunk，避免 Node-API 收到错误格式
                                // 原来的逻辑会返回 Gemini 格式的 chunk，导致 Node-API 解析失败和内容丢失
                                error!(
                                    "❌ Failed to transform stream chunk: {:?}\nOriginal chunk: {}\n⚠️ Skipping this chunk to avoid format mismatch",
                                    e,
                                    chunk_str
                                );
                                None  // 跳过这个 chunk，避免格式混乱
                            }
                        }
                    }
                    Err(e) => Some(Err(e)),
                }
            }
        });

        Ok(Box::pin(transformed_stream))
    }

    /// Get the provider type
    pub fn provider_type(&self) -> &ProviderType {
        &self.provider_type
    }

    /// Check if this client should be used for the given model class name
    pub fn supports_model_class(&self, model_class_name: &str) -> bool {
        let detected_type = ProviderType::from_model_class_name(model_class_name);
        detected_type != ProviderType::Unknown && detected_type == self.provider_type
    }
}
