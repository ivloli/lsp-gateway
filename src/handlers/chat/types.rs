use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tracing::debug;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

/// Content can be either a simple string or an array of content parts (multimodal)
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// Individual content part for multimodal messages
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    InputAudio { input_audio: InputAudio },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InputAudio {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl MessageContent {
    /// Convert content to a simple string (for backward compatibility)
    pub fn to_string(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    #[serde(default = "generate_default_id")]
    pub id: String,
    #[serde(default = "default_object")]
    pub object: String,
    #[serde(default = "default_created")]
    pub created: u64,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChatChoice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompletionRequest {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionResponse {
    #[serde(default = "generate_default_id")]
    pub id: String,
    #[serde(default = "default_completion_object")]
    pub object: String,
    #[serde(default = "default_created")]
    pub created: u64,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<CompletionChoice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub text: String,
    pub finish_reason: Option<String>,
}

/// Trait for converting requests to provider format
pub trait ToProviderRequest {
    fn to_provider_request(&self, target_model: &str, model_class_name: &str) -> JsonValue;
}

/// Trait for merging model configuration parameters with request parameters
pub trait WithModelParameters {
    fn temperature(&self) -> Option<f32>;
    fn set_temperature(&mut self, temp: Option<f32>);
    fn max_tokens(&self) -> Option<u32>;
    fn set_max_tokens(&mut self, tokens: Option<u32>);
    fn top_p(&self) -> Option<f32>;
    fn set_top_p(&mut self, top_p: Option<f32>);
    fn frequency_penalty(&self) -> Option<f32>;
    fn set_frequency_penalty(&mut self, penalty: Option<f32>);
    fn presence_penalty(&self) -> Option<f32>;
    fn set_presence_penalty(&mut self, penalty: Option<f32>);
    fn stop(&self) -> Option<Vec<String>>;
    fn set_stop(&mut self, stop: Option<Vec<String>>);
    fn extra(&mut self) -> &mut HashMap<String, JsonValue>;

    fn with_model_parameters(mut self, model_parameters: &JsonValue) -> Self
    where
        Self: Sized,
    {
        if let Some(params_obj) = model_parameters.as_object() {
            // Apply model config parameters as defaults
            if self.temperature().is_none() {
                if let Some(temp) = params_obj.get("temperature").and_then(|v| v.as_f64()) {
                    self.set_temperature(Some(temp as f32));
                }
            }

            if self.max_tokens().is_none() {
                if let Some(max_tokens) = params_obj.get("max_tokens").and_then(|v| v.as_u64()) {
                    self.set_max_tokens(Some(max_tokens as u32));
                }
            }

            // Handle maxTokens parameter - if max_tokens > maxTokens, use maxTokens value
            if let Some(max_tokens_limit) = params_obj.get("maxTokens").and_then(|v| v.as_u64()) {
                let max_tokens_limit = max_tokens_limit as u32;
                if let Some(current_max_tokens) = self.max_tokens() {
                    if current_max_tokens > max_tokens_limit {
                        self.set_max_tokens(Some(max_tokens_limit));
                    }
                } else {
                    self.set_max_tokens(Some(max_tokens_limit));
                }
            }

            if self.top_p().is_none() {
                if let Some(top_p) = params_obj.get("top_p").and_then(|v| v.as_f64()) {
                    self.set_top_p(Some(top_p as f32));
                }
            }

            if self.frequency_penalty().is_none() {
                if let Some(freq_penalty) =
                    params_obj.get("frequency_penalty").and_then(|v| v.as_f64())
                {
                    self.set_frequency_penalty(Some(freq_penalty as f32));
                }
            }

            if self.presence_penalty().is_none() {
                if let Some(pres_penalty) =
                    params_obj.get("presence_penalty").and_then(|v| v.as_f64())
                {
                    self.set_presence_penalty(Some(pres_penalty as f32));
                }
            }

            // Always use parameter.stop to replace the stop field (regardless of whether request has stop)
            if let Some(stop_val) = params_obj.get("stop") {
                if let Some(stop_array) = stop_val.as_array() {
                    let stop_strings: Vec<String> = stop_array
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    if !stop_strings.is_empty() {
                        self.set_stop(Some(stop_strings));
                    }
                } else if let Some(stop_str) = stop_val.as_str() {
                    self.set_stop(Some(vec![stop_str.to_string()]));
                }
            }

            // Merge extra parameters from model config
            for (key, value) in params_obj {
                if !matches!(
                    key.as_str(),
                    "temperature"
                        | "max_tokens"
                        | "maxTokens"
                        | "top_p"
                        | "frequency_penalty"
                        | "presence_penalty"
                        | "stop"
                ) {
                    // Special handling for extra_body: deep merge
                    if key == "extra_body" {
                        let extra = self.extra();
                        if let Some(existing_extra_body) = extra.get_mut("extra_body") {
                            // Deep merge: merge model_config extra_body into request extra_body
                            if let (Some(existing_obj), Some(config_obj)) =
                                (existing_extra_body.as_object_mut(), value.as_object())
                            {
                                for (k, v) in config_obj {
                                    existing_obj.entry(k.clone()).or_insert_with(|| v.clone());
                                }
                            }
                        } else {
                            // No existing extra_body, insert the whole thing
                            extra.insert(key.clone(), value.clone());
                        }
                    } else {
                        // For other fields, use simple or_insert (request priority)
                        self.extra()
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                }
            }
        }

        self
    }
}

/// Internal parameters that should not be sent to LLM providers
/// These are used for lsp-gateway configuration only
const INTERNAL_PARAMETERS: &[&str] = &[
    "skipNormalize",
    "stream_no_token_timeout_ms",
    "response_finish_timeout_ms",
    "thinking",
    "isVertex",
];

/// Float fields that need precision rounding to avoid issues like 0.800000011920929
const FLOAT_FIELDS: &[&str] = &[
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
];

/// Round float values to avoid precision issues (e.g., 0.800000011920929 -> 0.8)
fn round_float_values(obj: &mut serde_json::Map<String, JsonValue>) {
    for key in FLOAT_FIELDS {
        if let Some(value) = obj.get_mut(*key) {
            if let Some(num) = value.as_f64() {
                // Round to 2 decimal places for common LLM parameters
                let rounded = (num * 100.0).round() / 100.0;
                if let Some(new_num) = serde_json::Number::from_f64(rounded) {
                    *value = JsonValue::Number(new_num);
                }
            }
        }
    }
}

impl ToProviderRequest for ChatCompletionRequest {
    fn to_provider_request(&self, target_model: &str, model_class_name: &str) -> JsonValue {
        // Determine processing mode based on model class
        // - Gemini/Vertex models use mergeMessages(strict=true, placeholders=true)
        // - Other OpenAI-compatible models use squashContinueSameRoleMessages() only
        let needs_strict_alternation = Self::needs_strict_role_alternation(model_class_name);

        // Step 1: Squash consecutive same-role messages (all models need this)
        let squashed_request = self.clone().squash_consecutive_messages();

        // Step 2: Apply strict role validation only for models that need it
        let fixed_request = if needs_strict_alternation {
            squashed_request.with_validated_roles(target_model)
        } else {
            squashed_request
        };

        let mut request = serde_json::to_value(fixed_request).unwrap_or_default();
        if let Some(obj) = request.as_object_mut() {
            obj.insert(
                "model".to_string(),
                JsonValue::String(target_model.to_string()),
            );
            // Remove any null values to prevent provider errors
            remove_null_values(obj);
            // Round float values to avoid precision issues (e.g., 0.800000011920929 -> 0.8)
            round_float_values(obj);
            // Remove internal parameters that should not be sent to LLM providers
            for param in INTERNAL_PARAMETERS {
                obj.remove(*param);
            }
        }
        request
    }
}

impl WithModelParameters for ChatCompletionRequest {
    fn temperature(&self) -> Option<f32> {
        self.temperature
    }
    fn set_temperature(&mut self, temp: Option<f32>) {
        self.temperature = temp;
    }
    fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }
    fn set_max_tokens(&mut self, tokens: Option<u32>) {
        self.max_tokens = tokens;
    }
    fn top_p(&self) -> Option<f32> {
        self.top_p
    }
    fn set_top_p(&mut self, top_p: Option<f32>) {
        self.top_p = top_p;
    }
    fn frequency_penalty(&self) -> Option<f32> {
        self.frequency_penalty
    }
    fn set_frequency_penalty(&mut self, penalty: Option<f32>) {
        self.frequency_penalty = penalty;
    }
    fn presence_penalty(&self) -> Option<f32> {
        self.presence_penalty
    }
    fn set_presence_penalty(&mut self, penalty: Option<f32>) {
        self.presence_penalty = penalty;
    }
    fn stop(&self) -> Option<Vec<String>> {
        self.stop.clone()
    }
    fn set_stop(&mut self, stop: Option<Vec<String>>) {
        self.stop = stop;
    }
    fn extra(&mut self) -> &mut HashMap<String, JsonValue> {
        &mut self.extra
    }
}

impl ChatCompletionRequest {
    /// Check if a model class requires strict role alternation
    ///
    /// Models that use mergeMessages(strict=true, placeholders=true):
    /// - geminiChat.ts, geminiChatV2.ts
    /// - geminiThinkingChat.ts, geminiCustomThinkingChat.ts, geminiCustomThinkingChatV2.ts
    /// - vertexChat.ts
    ///
    /// Models that only use squashContinueSameRoleMessages():
    /// - baseOpenAIChat.ts (and all other OpenAI-compatible models)
    /// - mixtralChat.ts, llama3Chat.ts, gemma3Chat.ts, byteplusChat.ts, etc.
    fn needs_strict_role_alternation(model_class_name: &str) -> bool {
        matches!(
            model_class_name,
            "ChatGemini"
                | "ChatGeminiV2"
                | "ChatGeminiThinking"
                | "ChatGeminiCustomThinking"
                | "ChatGeminiCustomThinkingV2"
                | "VertexChat"
                | "ChatVertex"
        )
    }

    /// Squash consecutive messages with the same role
    /// This merges content from consecutive same-role messages with "\n\n" separator
    pub fn squash_consecutive_messages(mut self) -> Self {
        debug!("MESSAGE_PROCESSING: Starting squash_consecutive_messages");
        let original_count = self.messages.len();

        if self.messages.len() <= 1 {
            return self;
        }

        let mut squashed = Vec::new();
        let mut current_role = self.messages[0].role.clone();
        let mut current_content = self.messages[0].content.clone();
        let mut current_name = self.messages[0].name.clone();
        let mut merge_count = 0usize;

        for (idx, message) in self.messages.into_iter().skip(1).enumerate() {
            if message.role == current_role {
                // Merge with previous message
                debug!(
                    "MESSAGE_PROCESSING: Squashing consecutive '{}' message at position {}",
                    message.role,
                    idx + 1
                );
                let current_str = current_content.to_string();
                let message_str = message.content.to_string();
                if !current_str.trim().is_empty() && !message_str.trim().is_empty() {
                    let merged = format!("{}\n\n{}", current_str, message_str);
                    current_content = MessageContent::Text(merged);
                } else if message_str.trim().is_empty() {
                    // Keep current
                } else {
                    current_content = message.content;
                }
                merge_count += 1;
            } else {
                // Save previous and start new
                squashed.push(ChatMessage {
                    role: current_role,
                    content: current_content,
                    name: current_name,
                });
                current_role = message.role;
                current_content = message.content;
                current_name = message.name;
            }
        }

        // Push last message
        squashed.push(ChatMessage {
            role: current_role,
            content: current_content,
            name: current_name,
        });

        // Remove messages with only whitespace
        let before_filter = squashed.len();
        squashed.retain(|msg| !msg.content.to_string().trim().is_empty());
        let removed_empty = before_filter - squashed.len();

        if merge_count > 0 || removed_empty > 0 {
            debug!(
                "MESSAGE_PROCESSING: Squashed {} -> {} messages (merged: {}, removed empty: {})",
                original_count,
                squashed.len(),
                merge_count,
                removed_empty
            );
        }

        self.messages = squashed;
        self
    }

    /// Create a new request with validated role alternation for universal provider compatibility
    ///
    /// This follows the same logic as the upstream mergeMessages with strict + placeholders mode:
    ///
    /// mergeMessages(strict=true, placeholders=true) does:
    /// 1. Squash consecutive same-role messages (already done before this method is called)
    /// 2. Remove whitespace-only messages (already done in squash_consecutive_messages)
    /// 3. If empty, add a placeholder user message
    /// 4. Convert mid-prompt system messages to user messages
    /// 5. Insert placeholder messages to ensure proper alternation when needed
    /// 6. Recursively call mergeMessages (strict=false) to re-squash
    ///
    pub fn with_validated_roles(&self, target_model: &str) -> Self {
        debug!(
            "MESSAGE_PROCESSING: Starting with_validated_roles for model: {}",
            target_model
        );
        const PROMPT_PLACEHOLDER: &str = "__placeholder__";

        let mut merged_messages: Vec<ChatMessage> = self.messages.clone();

        // Step 1: If messages is empty, add a placeholder user message
        // Mirrors the upstream mergeMessages behavior: ensure the first message is from the user
        if merged_messages.is_empty() {
            debug!(
                "MESSAGE_PROCESSING: Adding placeholder user message for model: {}",
                target_model
            );
            merged_messages.push(ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text(PROMPT_PLACEHOLDER.to_string()),
                name: None,
            });
        }

        // Step 2: Force mid-prompt system messages to be user messages
        for i in 0..merged_messages.len() {
            if i > 0 && merged_messages[i].role == "system" {
                debug!("MESSAGE_PROCESSING: Converting mid-prompt system->user at position {} for model: {}", i, target_model);
                merged_messages[i].role = "user".to_string();
            }
        }

        // Step 3: Insert placeholder messages to ensure proper alternation
        if !merged_messages.is_empty() {
            if merged_messages[0].role == "system" {
                // If first is system and (only one message OR second is not user), insert user placeholder at position 1
                if merged_messages.len() == 1 || merged_messages[1].role != "user" {
                    let reason = if merged_messages.len() == 1 {
                        "only system message exists"
                    } else {
                        "second message is not user role"
                    };
                    debug!("MESSAGE_PROCESSING: Inserting user placeholder at position 1 ({}) for model: {}", reason, target_model);
                    merged_messages.insert(
                        1,
                        ChatMessage {
                            role: "user".to_string(),
                            content: MessageContent::Text(PROMPT_PLACEHOLDER.to_string()),
                            name: None,
                        },
                    );
                }
            } else if merged_messages[0].role != "system" && merged_messages[0].role != "user" {
                // If first is not system and not user (e.g., assistant), prepend a user placeholder
                debug!("MESSAGE_PROCESSING: Prepending user placeholder (first message is '{}', not user/system) for model: {}", merged_messages[0].role, target_model);
                merged_messages.insert(
                    0,
                    ChatMessage {
                        role: "user".to_string(),
                        content: MessageContent::Text(PROMPT_PLACEHOLDER.to_string()),
                        name: None,
                    },
                );
            }
        }

        // Step 4: Recursively call with strict=false to re-squash
        // This merges any consecutive same-role messages created by step 2
        let intermediate_request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: merged_messages,
            stream: self.stream,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            frequency_penalty: self.frequency_penalty,
            presence_penalty: self.presence_penalty,
            stop: self.stop.clone(),
            user: self.user.clone(),
            extra: self.extra.clone(),
        };

        // Re-squash to merge consecutive same-role messages
        intermediate_request.squash_consecutive_messages()
    }
}

impl ToProviderRequest for CompletionRequest {
    fn to_provider_request(&self, target_model: &str, _model_class_name: &str) -> JsonValue {
        // CompletionRequest already has prompt as string
        // Just add model and remove nulls
        let mut request = serde_json::to_value(self).unwrap_or_default();
        if let Some(obj) = request.as_object_mut() {
            obj.insert(
                "model".to_string(),
                JsonValue::String(target_model.to_string()),
            );
            // Remove any null values to prevent provider errors
            remove_null_values(obj);
            // Round float values to avoid precision issues (e.g., 0.800000011920929 -> 0.8)
            round_float_values(obj);
            // Remove internal parameters that should not be sent to LLM providers
            for param in INTERNAL_PARAMETERS {
                obj.remove(*param);
            }
        }
        request
    }
}

impl WithModelParameters for CompletionRequest {
    fn temperature(&self) -> Option<f32> {
        self.temperature
    }
    fn set_temperature(&mut self, temp: Option<f32>) {
        self.temperature = temp;
    }
    fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }
    fn set_max_tokens(&mut self, tokens: Option<u32>) {
        self.max_tokens = tokens;
    }
    fn top_p(&self) -> Option<f32> {
        self.top_p
    }
    fn set_top_p(&mut self, top_p: Option<f32>) {
        self.top_p = top_p;
    }
    fn frequency_penalty(&self) -> Option<f32> {
        self.frequency_penalty
    }
    fn set_frequency_penalty(&mut self, penalty: Option<f32>) {
        self.frequency_penalty = penalty;
    }
    fn presence_penalty(&self) -> Option<f32> {
        self.presence_penalty
    }
    fn set_presence_penalty(&mut self, penalty: Option<f32>) {
        self.presence_penalty = penalty;
    }
    fn stop(&self) -> Option<Vec<String>> {
        self.stop.clone()
    }
    fn set_stop(&mut self, stop: Option<Vec<String>>) {
        self.stop = stop;
    }
    fn extra(&mut self) -> &mut HashMap<String, JsonValue> {
        &mut self.extra
    }
}

// Default value functions
fn generate_default_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4().simple())
}

fn default_object() -> String {
    "chat.completion".to_string()
}

fn default_completion_object() -> String {
    "text_completion".to_string()
}

fn default_created() -> u64 {
    crate::utils::TimeUtils::current_timestamp_secs()
}

fn default_model() -> String {
    "unknown".to_string()
}

/// Remove null values from a JSON object to prevent provider errors
fn remove_null_values(obj: &mut serde_json::Map<String, JsonValue>) {
    obj.retain(|_key, value| !value.is_null());
}
