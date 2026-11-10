mod auth;
pub mod growthbook;
pub mod request;

use crate::error::{AppError, Result};
use crate::gateway::{StreamHandler, StreamTimeoutConfig};
use crate::handlers::chat::{
    types::MessageContent, ChatCompletionResponse, CompletionRequest, CompletionResponse,
    ToProviderRequest,
};
use axum::http::HeaderMap;
use once_cell::sync::Lazy;
use opentelemetry::global;
use opentelemetry_http::HeaderInjector;
use parking_lot::RwLock;
use reqwest::{Client, RequestBuilder};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid;

use crate::routing::provider::SelectedProvider;
pub use auth::get_default_base_url as get_provider_default_base_url;
use auth::{get_auth_config, get_auth_config_with_priority, AuthConfig};
use request::{ApiRequest, ApiResponse, RequestConfig};

fn format_list_truncated(items: &[String], max_items: usize) -> String {
    if items.is_empty() {
        return String::new();
    }

    if items.len() <= max_items {
        return items.join(", ");
    }

    let shown: Vec<_> = items.iter().take(max_items).cloned().collect();
    format!(
        "{}, ... and {} more",
        shown.join(", "),
        items.len() - max_items
    )
}

/// Inject OpenTelemetry trace context headers into the request
fn inject_trace_headers(mut req_builder: RequestBuilder) -> RequestBuilder {
    let context = tracing::Span::current().context();
    let mut headers = axum::http::HeaderMap::new();
    let mut injector = HeaderInjector(&mut headers);
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut injector);
    });

    // Add injected headers to request
    for (key, value) in headers.iter() {
        if let Ok(header_value) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
            req_builder = req_builder.header(key.as_str(), header_value);
        }
    }

    req_builder
}

/// Global HTTP client cache to avoid repeated SSL certificate loading
/// Key: timeout configuration hash
/// Value: Shared reqwest Client
static HTTP_CLIENT_CACHE: Lazy<RwLock<HashMap<u64, Arc<Client>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Get or create a cached HTTP client with the given timeout configuration
fn get_or_create_client(timeout_ms: u64) -> Result<Arc<Client>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Calculate cache key based on timeout
    let mut hasher = DefaultHasher::new();
    timeout_ms.hash(&mut hasher);
    let cache_key = hasher.finish();

    // Try to get from cache first (read lock)
    {
        let cache = HTTP_CLIENT_CACHE.read();
        if let Some(client) = cache.get(&cache_key) {
            debug!("Reusing cached HTTP client for timeout {}ms", timeout_ms);
            return Ok(client.clone());
        }
    }

    // Not in cache, create new client (write lock)
    let mut cache = HTTP_CLIENT_CACHE.write();

    // Double-check in case another thread created it
    if let Some(client) = cache.get(&cache_key) {
        return Ok(client.clone());
    }

    debug!(
        "Creating new HTTP client with timeout {}ms (using rustls)",
        timeout_ms
    );

    let client = Arc::new(
        Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .connect_timeout(Duration::from_secs(10))
            .http1_only()
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(100)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::HttpClient(e))?,
    );

    cache.insert(cache_key, client.clone());

    debug!("HTTP client cache size: {}", cache.len());

    Ok(client)
}

pub struct LlmClient {
    client: Client,
    #[allow(dead_code)]
    provider_type: String,
    stream_handler: StreamHandler,
    auth_config: AuthConfig, // Cache auth config to avoid repeated env var reads
}

impl LlmClient {
    /// Get the API key from the auth config (for GenAI adapters)
    pub fn get_api_key(&self) -> Option<&str> {
        self.auth_config.api_key.as_deref()
    }

    pub fn new(provider_type: &str) -> Result<Self> {
        let default_config = StreamTimeoutConfig::default();
        let http_timeout_ms = default_config.response_finish_timeout_ms;

        // Use cached client
        let client = get_or_create_client(http_timeout_ms)?;
        let auth_config = get_auth_config(provider_type);

        Ok(Self {
            client: (*client).clone(),
            provider_type: provider_type.to_string(),
            stream_handler: StreamHandler::with_default_config(),
            auth_config,
        })
    }

    pub fn with_timeout_config(
        provider_type: &str,
        timeout_config: StreamTimeoutConfig,
    ) -> Result<Self> {
        // HTTP client timeout should be max of both timeouts to avoid premature disconnection
        let http_timeout_ms = std::cmp::max(
            std::cmp::max(
                timeout_config.response_finish_timeout_ms,
                timeout_config.stream_no_token_timeout_ms,
            ),
            300_000, // minimum 5 minutes
        );

        // Use cached client
        let client = get_or_create_client(http_timeout_ms)?;
        let auth_config = get_auth_config(provider_type);

        Ok(Self {
            client: (*client).clone(),
            provider_type: provider_type.to_string(),
            stream_handler: StreamHandler::new(timeout_config),
            auth_config,
        })
    }

    /// Create LlmClient with priority-based API key selection
    pub fn with_selected_provider(
        selected_provider: &SelectedProvider,
        timeout_config: StreamTimeoutConfig,
    ) -> Result<Self> {
        debug!(
            "LlmClient stream_no_token_timeout_ms={}, response_finish_timeout_ms={}",
            timeout_config.stream_no_token_timeout_ms, timeout_config.response_finish_timeout_ms
        );

        // HTTP client timeout should be max of both timeouts to avoid premature disconnection
        let http_timeout_ms = std::cmp::max(
            std::cmp::max(
                timeout_config.response_finish_timeout_ms,
                timeout_config.stream_no_token_timeout_ms,
            ),
            5000,
        );
        debug!("HTTP client timeout set to {}ms", http_timeout_ms);

        // Use cached client
        let client = get_or_create_client(http_timeout_ms)?;

        // Use priority-based API key selection
        let auth_config = get_auth_config_with_priority(
            &selected_provider.model_config,
            selected_provider.selected_model_provider.as_ref(),
            &selected_provider.provider,
        );

        Ok(Self {
            client: (*client).clone(),
            provider_type: selected_provider.provider.clone(),
            stream_handler: StreamHandler::new(timeout_config),
            auth_config,
        })
    }

    /// Chat completions stream with pre-transformed ApiRequest (for GenAI adapters)
    pub async fn chat_completions_stream_with_api_request(
        &self,
        base_url: &str,
        model_name: &str,
        api_request: ApiRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<crate::handlers::chat::response::StreamResponse> {
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            true,
            model_config_id,
        );

        match self.send_request_stream(api_request, config).await? {
            ApiResponse::Stream(stream) => Ok(stream),
            _ => Err(AppError::Internal(
                "Expected streaming response".to_string(),
            )),
        }
    }

    /// Completions stream with pre-transformed ApiRequest (for GenAI adapters)
    pub async fn completions_stream_with_api_request(
        &self,
        base_url: &str,
        model_name: &str,
        api_request: ApiRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<crate::handlers::chat::response::StreamResponse> {
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            true,
            model_config_id,
        );

        match self.send_request_stream(api_request, config).await? {
            ApiResponse::Stream(stream) => Ok(stream),
            _ => Err(AppError::Internal(
                "Expected streaming response".to_string(),
            )),
        }
    }

    /// Unified method for regular completions
    pub async fn completions(
        &self,
        base_url: &str,
        model_name: &str,
        request: CompletionRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<CompletionResponse> {
        // Transform the request to provider format first
        // Note: Legacy path - model_class_name not available, using empty string
        let transformed_json = request.to_provider_request(model_name, "");
        let api_request = ApiRequest::Completion(transformed_json);
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            false,
            model_config_id,
        );

        match self.send_request(api_request, config).await? {
            ApiResponse::Completion(response) => Ok(response),
            _ => Err(AppError::Internal(
                "Expected completion response".to_string(),
            )),
        }
    }

    /// Completions with pre-transformed ApiRequest (for GenAI adapters)
    pub async fn completions_with_api_request(
        &self,
        base_url: &str,
        model_name: &str,
        api_request: ApiRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<CompletionResponse> {
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            false,
            model_config_id,
        );

        match self.send_request(api_request, config).await? {
            ApiResponse::Completion(response) => Ok(response),
            _ => Err(AppError::Internal(
                "Expected completion response".to_string(),
            )),
        }
    }

    /// Send a chat completion request and return raw response text (for GenAI adapters)
    pub async fn chat_completions_raw_response(
        &self,
        base_url: &str,
        model_name: &str,
        api_request: ApiRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<String> {
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            false,
            model_config_id,
        );
        self.send_request_raw(api_request, config).await
    }

    /// Send a completion request and return raw response text (for GenAI adapters)
    pub async fn completions_raw_response(
        &self,
        base_url: &str,
        model_name: &str,
        api_request: ApiRequest,
        headers: HeaderMap,
        model_config_id: String,
    ) -> Result<String> {
        let config = RequestConfig::new(
            base_url.to_string(),
            model_name.to_string(),
            headers,
            false,
            model_config_id,
        );
        self.send_request_raw(api_request, config).await
    }

    /// Send request and return raw response text (for GenAI adapters)
    async fn send_request_raw(
        &self,
        api_request: ApiRequest,
        config: RequestConfig,
    ) -> Result<String> {
        let url = config.base_url.clone();
        let request_type = api_request.request_type();

        // Get the pre-transformed JSON payload
        let provider_request = api_request.get_json_payload().clone();

        // Calculate payload size (don't log full payload - it can be HUGE in traces!)
        let payload_json =
            serde_json::to_string(&provider_request).unwrap_or_else(|_| "Invalid JSON".to_string());
        let payload_size = payload_json.len();

        // Only log summary info in traces (not full payload to avoid giant spans)
        debug!(
            url = %url,
            request_type = %request_type,
            model_config_id = %config.model_config_id,
            payload_size_bytes = payload_size,
            "Request URL: {}, type:completion , Payload (ModelConfig ID: {}): {} bytes",
            url, config.model_config_id, payload_size
        );

        // Build the HTTP request
        let mut req_builder = self.client.post(&url).json(&provider_request);
        // Note: Authentication headers are now handled by the caller (GenAI client via adapters)
        req_builder = self.forward_headers(req_builder, config.headers);
        // Inject OpenTelemetry trace context headers
        req_builder = inject_trace_headers(req_builder);

        // Send the request
        let response = req_builder.send().await.map_err(|e| {
            error!("Failed to send {} request to provider: {}", request_type, e);
            // Don't log full payload in error traces either!
            error!(
                model = %config.model_name,
                model_config_id = %config.model_config_id,
                url = %url,
                payload_size_bytes = payload_size,
                "Request failed - payload size: {} bytes", payload_size
            );
            AppError::HttpClient(e)
        })?;

        let status = response.status();
        debug!("Provider {} response status: {}", request_type, status);

        // Log response headers
        debug!(
            "Received {} response from provider, status: {}",
            request_type, status
        );

        if !status.is_success() {
            let error_result: Result<String> =
                handle_provider_error(response, status, &config.model_name, &url).await;
            return error_result;
        }

        // Get raw response text without parsing
        let response_text = self
            .stream_handler
            .process_response_with_timeout(response)
            .await?;
        let response_size = response_text.len();
        debug!("Provider response body length: {}", response_size);
        // Don't log full response body in traces - can be huge!
        debug!(
            response_size_bytes = response_size,
            "Response received: {} bytes", response_size
        );

        Ok(response_text)
    }

    /// Unified request processing method for streaming
    #[tracing::instrument(
        name = "llm_request",
        skip(self, api_request, config),
        fields(
            url = %config.base_url,
            model = %config.model_name,
            status = tracing::field::Empty,
            connect_time_ms = tracing::field::Empty,
        )
    )]
    async fn send_request_stream(
        &self,
        api_request: ApiRequest,
        config: RequestConfig,
    ) -> Result<ApiResponse> {
        let url = config.base_url.clone();
        let request_type = api_request.request_type();

        // Get the pre-transformed JSON payload
        let provider_request = api_request.get_json_payload().clone();

        // Calculate payload size (don't log full payload in traces!)
        let payload_json =
            serde_json::to_string(&provider_request).unwrap_or_else(|_| "Invalid JSON".to_string());
        let payload_size = payload_json.len();

        // Log only summary info in traces
        debug!(
            url = %url,
            request_type = %request_type,
            model_config_id = %config.model_config_id,
            payload_size_bytes = payload_size,
            "Streaming request - payload: {} bytes", payload_size
        );

        // Build the HTTP request
        let mut req_builder = self.client.post(&url).json(&provider_request);
        // Note: Authentication headers are now handled by the caller (GenAI client via adapters)
        req_builder = self.forward_headers(req_builder, config.headers);
        // Inject OpenTelemetry trace context headers
        req_builder = inject_trace_headers(req_builder);

        // Send the request
        info!(
            "📡 Sending streaming request to provider: url={}, model={}",
            url, config.model_name
        );
        let request_start = std::time::Instant::now();
        let response = req_builder
            .send()
            .await
            .map_err(|e| {
                let elapsed = request_start.elapsed();
                error!("Failed to send {} request to provider after {:?}: {} (is_timeout: {}, is_connect: {}, is_request: {})", 
                    request_type, elapsed, e, e.is_timeout(), e.is_connect(), e.is_request());
                error!("Request details - model: {}, ModelConfig ID: {}, url: {}, payload size: {} bytes", 
                    config.model_name,
                    config.model_config_id,
                    url,
                    payload_size);
                AppError::HttpClient(e)
            })?;

        let status = response.status();
        let connect_elapsed = request_start.elapsed();

        // Record span fields for tracing
        tracing::Span::current().record("status", status.as_u16());
        tracing::Span::current().record("connect_time_ms", connect_elapsed.as_millis() as u64);

        // Log response headers
        info!(
            "📥 Received {} streaming response from provider in {:?}, status: {}",
            request_type, connect_elapsed, status
        );

        if !status.is_success() {
            return handle_provider_error(response, status, &config.model_name, &url).await;
        }

        // Use real streaming with timeout protection
        let stream = response.bytes_stream();
        let timeout_stream = self
            .stream_handler
            .process_stream_with_timeout_streaming(stream)?;
        let boxed_stream: crate::handlers::chat::response::StreamResponse =
            Box::pin(timeout_stream);

        Ok(ApiResponse::Stream(boxed_stream))
    }

    /// Unified request processing method
    async fn send_request(
        &self,
        api_request: ApiRequest,
        config: RequestConfig,
    ) -> Result<ApiResponse> {
        let url = config.base_url.clone();
        let request_type = api_request.request_type();

        // Get the pre-transformed JSON payload
        let provider_request = api_request.get_json_payload().clone();

        // Calculate payload size (don't log full payload in traces!)
        let payload_json =
            serde_json::to_string(&provider_request).unwrap_or_else(|_| "Invalid JSON".to_string());
        let payload_size = payload_json.len();

        // Log only summary info in traces
        info!(
            url = %url,
            request_type = %request_type,
            model_config_id = %config.model_config_id,
            payload_size_bytes = payload_size,
            "📦 Unified request - payload: {} bytes", payload_size
        );

        // Build the HTTP request
        let mut req_builder = self.client.post(&url).json(&provider_request);
        // Note: Authentication headers are now handled by the caller (GenAI client via adapters)
        req_builder = self.forward_headers(req_builder, config.headers);
        // Inject OpenTelemetry trace context headers
        req_builder = inject_trace_headers(req_builder);

        // Send the request
        let response = req_builder.send().await.map_err(|e| {
            error!("Failed to send {} request to provider: {}", request_type, e);
            error!(
                "Request details - model: {}, ModelConfig ID: {}, url: {}, payload: {}",
                config.model_name,
                config.model_config_id,
                url,
                serde_json::to_string(&provider_request)
                    .unwrap_or_else(|_| "Invalid JSON".to_string())
            );
            AppError::HttpClient(e)
        })?;

        let status = response.status();

        // Log response headers
        info!(
            "📥 Received {} response from provider, status: {}",
            request_type, status
        );

        if !status.is_success() {
            return handle_provider_error(response, status, &config.model_name, &url).await;
        }

        // Process response based on streaming flag
        if config.is_streaming {
            // For non-streaming send_request, we still process the full response
            let response_text = self
                .stream_handler
                .process_stream_with_timeout(response)
                .await?;

            // Convert string to stream for compatibility
            use futures_util::stream;
            let bytes = bytes::Bytes::from(response_text);
            let stream = stream::once(async move { Ok(bytes) });
            let boxed_stream: crate::handlers::chat::response::StreamResponse = Box::pin(stream);
            Ok(ApiResponse::Stream(boxed_stream))
        } else {
            let response_text = self
                .stream_handler
                .process_response_with_timeout(response)
                .await?;
            let response_size = response_text.len();
            // Don't log full response body in traces!
            info!(
                response_size_bytes = response_size,
                "📦 Response received: {} bytes", response_size
            );

            // Parse response based on request type
            let parsed_response = match api_request {
                ApiRequest::ChatCompletion(_) => {
                    let chat_response: ChatCompletionResponse =
                        serde_json::from_str(&response_text)
                            .or_else(|_| {
                                self.convert_non_openai_response(&response_text, &config.model_name)
                            })
                            .map_err(|e| {
                                error!("Failed to parse provider chat completion response: {}", e);
                                error!(
                                    "Parse error details - model: {}, url: {}",
                                    config.model_name, url
                                );
                                error!("Response body: {}", response_text);
                                AppError::Serialization(e)
                            })?;
                    ApiResponse::ChatCompletion(chat_response)
                }
                ApiRequest::Completion(_) => {
                    let completion_response: CompletionResponse =
                        serde_json::from_str(&response_text)
                            .or_else(|_| {
                                self.convert_non_openai_completion_response(
                                    &response_text,
                                    &config.model_name,
                                )
                            })
                            .map_err(|e| {
                                error!("Failed to parse provider completion response: {}", e);
                                error!(
                                    "Parse error details - model: {}, url: {}",
                                    config.model_name, url
                                );
                                error!("Response body: {}", response_text);
                                AppError::Serialization(e)
                            })?;
                    ApiResponse::Completion(completion_response)
                }
            };

            Ok(parsed_response)
        }
    }

    fn forward_headers(
        &self,
        mut req_builder: RequestBuilder,
        headers: HeaderMap,
    ) -> RequestBuilder {
        // Forward specific headers that might be useful
        let headers_to_forward = [
            "user-agent",
            "x-request-id",
            "x-forwarded-for",
            "x-real-ip",
            "content-type",
            "x-api-key",
            "x-client-id",
            "x-client-secret",
            "x-client-token",
            "x-client-refresh-token",
            "x-client-access-token",
            "authorization",
        ];

        let mut forwarded_headers = Vec::new();

        for header_name in headers_to_forward {
            if let Some(header_value) = headers.get(header_name) {
                if let Ok(value_str) = header_value.to_str() {
                    // Mask sensitive headers in logs
                    let log_value = if header_name == "authorization"
                        || header_name.contains("token")
                        || header_name.contains("key")
                        || header_name.contains("secret")
                    {
                        if value_str.len() > 10 {
                            format!(
                                "{}...{}",
                                &value_str[..6],
                                &value_str[value_str.len() - 4..]
                            )
                        } else {
                            "***".to_string()
                        }
                    } else {
                        value_str.to_string()
                    };
                    forwarded_headers.push(format!("{}: {}", header_name, log_value));
                    req_builder = req_builder.header(header_name, value_str);
                }
            }
        }

        if !forwarded_headers.is_empty() {
            debug!(
                "Forwarded Headers: [{}]",
                format_list_truncated(&forwarded_headers, 6)
            );
        }

        req_builder
    }

    /// Convert non-OpenAI format responses to OpenAI format
    fn convert_non_openai_response(
        &self,
        response_text: &str,
        model_name: &str,
    ) -> serde_json::Result<ChatCompletionResponse> {
        use crate::handlers::chat::{ChatChoice, ChatCompletionResponse, ChatMessage, Usage};

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(response_text) {
            let content = extract_response_content(&value);

            Ok(ChatCompletionResponse {
                id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
                object: "chat.completion".to_string(),
                created: crate::utils::TimeUtils::current_timestamp_secs(),
                model: model_name.to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: MessageContent::Text(content),
                        name: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                }),
            })
        } else {
            Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid response",
            )))
        }
    }

    /// Convert non-OpenAI format completion responses to OpenAI format
    fn convert_non_openai_completion_response(
        &self,
        response_text: &str,
        model_name: &str,
    ) -> serde_json::Result<CompletionResponse> {
        use crate::handlers::chat::{CompletionChoice, CompletionResponse, Usage};

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(response_text) {
            let content = extract_response_content(&value);

            Ok(CompletionResponse {
                id: format!("cmpl-{}", uuid::Uuid::new_v4().simple()),
                object: "text_completion".to_string(),
                created: crate::utils::TimeUtils::current_timestamp_secs(),
                model: model_name.to_string(),
                choices: vec![CompletionChoice {
                    index: 0,
                    text: content,
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                }),
            })
        } else {
            Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid response",
            )))
        }
    }
}

/// Extract content from various response formats
fn extract_response_content(value: &serde_json::Value) -> String {
    // Try common patterns
    value
        .get("data")
        .and_then(|d| d.get("result"))
        .and_then(|r| r.as_str())
        .or_else(|| value.get("message").and_then(|m| m.as_str()))
        .or_else(|| value.get("result").and_then(|r| r.as_str()))
        .or_else(|| value.get("content").and_then(|c| c.as_str()))
        .or_else(|| value.get("text").and_then(|t| t.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            serde_json::to_string(value).unwrap_or_else(|_| "Unknown response".to_string())
        })
}

/// Handle provider error response
async fn handle_provider_error<T>(
    response: reqwest::Response,
    status: reqwest::StatusCode,
    model_name: &str,
    url: &str,
) -> Result<T> {
    let response_headers = response.headers().clone();
    let error_text = response
        .text()
        .await
        .unwrap_or_else(|_| "Unknown error".to_string());

    error!(
        "Provider error: status={}, model={}, url={}, body_len={} bytes, headers={:?}, body={}",
        status,
        model_name,
        url,
        error_text.len(),
        response_headers,
        error_text
    );

    Err(AppError::Provider(format!(
        "Provider returned status {}: {}",
        status, error_text
    )))
}
