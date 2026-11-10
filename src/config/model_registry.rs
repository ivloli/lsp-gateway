use crate::genai::ProviderType;
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Template definitions - all templates in one place
#[derive(Debug)]
pub struct TemplateDefinition {
    pub name: &'static str,
    pub content: &'static str,
    pub description: &'static str,
}

/// Complete model configuration including template content
#[derive(Debug, Clone)]
pub struct ModelRegistryEntry {
    pub class_name: &'static str,
    pub provider_type: ProviderType,
    pub template_name: Option<&'static str>, // Store template name instead of reference
    pub description: &'static str,
    pub endpoint_type: EndpointType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EndpointType {
    ChatCompletions, // /v1/chat/completions
    Completions,     // /v1/completions
}

/// All template definitions in one place
pub const TEMPLATES: &[TemplateDefinition] = &[
    TemplateDefinition {
        name: "CHATML",
        content: r#"{%- for message in messages -%}
<|im_start|>{{ message.role }}
{{ message.content }}<|im_end|>
{% endfor -%}
<|im_start|>assistant
"#,
        description: "ChatML format for OpenAI-compatible models",
    },
    TemplateDefinition {
        name: "CHATML_THINKING",
        content: r#"{%- for message in messages -%}
<|im_start|>{{ message.role }}
{{ message.content }}<|im_end|>
{% endfor -%}
<|im_start|>assistant
<think>"#,
        description: "ChatML format with thinking support",
    },
    TemplateDefinition {
        name: "LLAMA3",
        content: r#"<|begin_of_text|>{%- for message in messages -%}
<|start_header_id|>{{ message.role }}<|end_header_id|>
{{ message.content }}<|eot_id|>{% endfor -%}
<|start_header_id|>assistant<|end_header_id|>

"#,
        description: "Llama 3 chat format",
    },
    TemplateDefinition {
        name: "MISTRAL_INSTRUCT",
        content: r#"<s> [INST] {%- if messages[0].role == 'system' -%}{{ messages[0].content | trim }}{%- endif -%}{%- for message in messages[1:-1] -%}{%- if message.role == 'assistant' -%} [/INST] {{ message.content | trim }}{%- elif message.role == 'user' -%} [INST] {{ message.content | trim }}{%- endif -%}{%- endfor -%}</s> [INST] {{ messages[-1].content | trim }} [/INST]"#,
        description: "Mistral Instruct format matching Node.js formatInstruct()",
    },
    TemplateDefinition {
        name: "PLAIN",
        content: r#"{%- for message in messages -%}{{ message.content }}{%- endfor -%}"#,
        description: "Simple plain concatenation for MixtralPlainChat",
    },
    TemplateDefinition {
        name: "GEMMA3",
        content: r#"{%- for message in messages -%}
{%- if message.role == 'system' -%}
<start_of_turn>user
{{ message.content | trim }}<end_of_turn>
{%- elif message.role == 'user' -%}
<start_of_turn>user
{{ message.content | trim }}<end_of_turn>
{%- elif message.role == 'assistant' -%}
<start_of_turn>model
{{ message.content | trim }}<end_of_turn>
{%- endif -%}
{%- endfor -%}
<start_of_turn>model
"#,
        description: "Gemma3 format matching Node.js formatInstructGemma3()",
    },
    TemplateDefinition {
        name: "ALPACA",
        content: r#"{%- if messages[0].role == 'system' -%}
{{ messages[0].content }}

{%- set loop_messages = messages[1:] -%}
{%- else -%}
{%- set loop_messages = messages -%}
{%- endif -%}
### Instruction:
This is the conversation so far:
{%- for message in loop_messages[:-1] -%}
{%- if message.role == 'user' -%}
USER: {{ message.content | trim }}
{%- elif message.role == 'assistant' -%}
ASSISTANT: {{ message.content | trim }}
{%- endif -%}
{%- endfor %}

### Input:
USER: {{ loop_messages[-1].content | trim }}

### Response:
ASSISTANT: "#,
        description: "Alpaca format",
    },
    TemplateDefinition {
        name: "PHI",
        content: r#"{%- for message in messages -%}
{%- if message.role == 'system' -%}
<|system|>
{{ message.content }}<|end|>
{%- elif message.role == 'user' -%}
<|user|>
{{ message.content }}<|end|>
{%- elif message.role == 'assistant' -%}
<|assistant|>
{{ message.content }}<|end|>
{%- endif -%}
{%- endfor -%}
<|assistant|>
"#,
        description: "Phi format",
    },
    TemplateDefinition {
        name: "COMMAND_R",
        content: r#"{%- if messages[0].role == 'system' -%}
<|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>{{ messages[0].content }}<|END_OF_TURN_TOKEN|>
{%- set loop_messages = messages[1:] -%}
{%- else -%}
{%- set loop_messages = messages -%}
{%- endif -%}
{%- for message in loop_messages -%}
{%- if message.role == 'user' -%}
<|START_OF_TURN_TOKEN|><|USER_TOKEN|>{{ message.content }}<|END_OF_TURN_TOKEN|>
{%- elif message.role == 'assistant' -%}
<|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>{{ message.content }}<|END_OF_TURN_TOKEN|>
{%- endif -%}
{%- endfor -%}
<|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>"#,
        description: "Command-R format",
    },
    TemplateDefinition {
        name: "VICUNA",
        content: r#"{%- if messages[0].role == 'system' -%}
{{ messages[0].content }}

{%- set loop_messages = messages[1:] -%}
{%- else -%}
{%- set loop_messages = messages -%}
{%- endif -%}
{%- for message in loop_messages -%}
{%- if message.role == 'user' -%}
USER: {{ message.content }}
{%- elif message.role == 'assistant' -%}
ASSISTANT: {{ message.content }}</s>
{%- endif -%}
{%- endfor -%}
ASSISTANT:"#,
        description: "Vicuna format",
    },
    TemplateDefinition {
        name: "ZEPHYR",
        content: r#"{%- for message in messages -%}
{%- if message.role == 'system' -%}
<|system|>
{{ message.content }}</s>
{%- elif message.role == 'user' -%}
<|user|>
{{ message.content }}</s>
{%- elif message.role == 'assistant' -%}
<|assistant|>
{{ message.content }}</s>
{%- endif -%}
{%- endfor -%}
<|assistant|>
"#,
        description: "Zephyr format",
    },
];

/// Find template by name
pub fn get_template_by_name(name: &str) -> Option<&'static TemplateDefinition> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// Complete model registry - all models and their configurations
pub const MODEL_REGISTRY: &[ModelRegistryEntry] = &[
    // ========== TEMPLATED MODELS (use /v1/completions) ==========

    // ChatML format models
    ModelRegistryEntry {
        class_name: "ChatMlChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("CHATML"),
        description: "ChatML format model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "ChatMlChatV2",
        provider_type: ProviderType::OpenAI,
        template_name: Some("CHATML"),
        description: "ChatML format model v2",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "ChatCustomMIChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("CHATML_THINKING"),
        description: "ChatML with thinking support",
        endpoint_type: EndpointType::Completions,
    },
    // Llama models
    ModelRegistryEntry {
        class_name: "LlamaChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("LLAMA3"),
        description: "Llama chat model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "Llama3Chat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("LLAMA3"),
        description: "Llama 3 chat model",
        endpoint_type: EndpointType::Completions,
    },
    // Mixtral models
    ModelRegistryEntry {
        class_name: "ChatMixtral",
        provider_type: ProviderType::OpenAI,
        template_name: Some("MISTRAL_INSTRUCT"),
        description: "Mixtral Instruct format",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "ChatMixtralPlain",
        provider_type: ProviderType::OpenAI,
        template_name: Some("PLAIN"),
        description: "Mixtral plain concatenation",
        endpoint_type: EndpointType::Completions,
    },
    // Gemma models
    ModelRegistryEntry {
        class_name: "ChatGemma3",
        provider_type: ProviderType::OpenAI,
        template_name: Some("GEMMA3"),
        description: "Gemma 3 chat model",
        endpoint_type: EndpointType::Completions,
    },
    // Other templated models
    ModelRegistryEntry {
        class_name: "AlpacaChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("ALPACA"),
        description: "Alpaca format model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "PhiChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("PHI"),
        description: "Phi format model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "CommandRChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("COMMAND_R"),
        description: "Command-R format model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "VicunaChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("VICUNA"),
        description: "Vicuna format model",
        endpoint_type: EndpointType::Completions,
    },
    ModelRegistryEntry {
        class_name: "ZephyrChat",
        provider_type: ProviderType::OpenAI,
        template_name: Some("ZEPHYR"),
        description: "Zephyr format model",
        endpoint_type: EndpointType::Completions,
    },
    // ========== STANDARD MODELS (use /v1/chat/completions) ==========

    // OpenAI-compatible models
    ModelRegistryEntry {
        class_name: "BaseOpenAIChat",
        provider_type: ProviderType::OpenAI,
        template_name: None,
        description: "Base OpenAI chat model",
        endpoint_type: EndpointType::ChatCompletions,
    },
    ModelRegistryEntry {
        class_name: "LSPGatewayChat",
        provider_type: ProviderType::OpenAI,
        template_name: None,
        description: "LSP Gateway chat model",
        endpoint_type: EndpointType::ChatCompletions,
    },
    ModelRegistryEntry {
        class_name: "MultiModelOpenAIChat",
        provider_type: ProviderType::OpenAI,
        template_name: None,
        description: "Multi-model OpenAI chat",
        endpoint_type: EndpointType::ChatCompletions,
    },
    ModelRegistryEntry {
        class_name: "ByteplusChat",
        provider_type: ProviderType::OpenAI,
        template_name: None,
        description: "Byteplus chat model",
        endpoint_type: EndpointType::ChatCompletions,
    },
    // Google/Gemini models
    ModelRegistryEntry {
        class_name: "ChatGemini",
        provider_type: ProviderType::Google,
        template_name: None,
        description: "Google Gemini chat model",
        endpoint_type: EndpointType::ChatCompletions,
    },
    ModelRegistryEntry {
        class_name: "ChatGeminiV2",
        provider_type: ProviderType::Google,
        template_name: None,
        description: "Google Gemini V2 chat model",
        endpoint_type: EndpointType::ChatCompletions,
    },
    // Anthropic models
    ModelRegistryEntry {
        class_name: "ChatSonnet",
        provider_type: ProviderType::Anthropic,
        template_name: None,
        description: "Anthropic Claude Sonnet model",
        endpoint_type: EndpointType::ChatCompletions,
    },
];

/// Lazy-initialized lookup maps for fast access
static REGISTRY_BY_CLASS: Lazy<HashMap<&'static str, &'static ModelRegistryEntry>> =
    Lazy::new(|| {
        MODEL_REGISTRY
            .iter()
            .map(|entry| (entry.class_name, entry))
            .collect()
    });

static TEMPLATES_BY_NAME: Lazy<HashMap<&'static str, &'static TemplateDefinition>> =
    Lazy::new(|| {
        TEMPLATES
            .iter()
            .map(|template| (template.name, template))
            .collect()
    });

/// Registry API - all model operations through one interface
pub struct ModelRegistry;

impl ModelRegistry {
    /// Get complete model entry
    pub fn get_model(class_name: &str) -> Option<&'static ModelRegistryEntry> {
        REGISTRY_BY_CLASS.get(class_name).copied()
    }

    /// Get provider type - returns Unknown if model not found
    pub fn get_provider_type(class_name: &str) -> ProviderType {
        Self::get_model(class_name)
            .map(|entry| entry.provider_type.clone())
            .unwrap_or(ProviderType::Unknown)
    }

    /// Get template content
    pub fn get_template_content(class_name: &str) -> Option<&'static str> {
        Self::get_model(class_name)
            .and_then(|entry| entry.template_name)
            .and_then(|name| TEMPLATES_BY_NAME.get(name))
            .map(|template| template.content)
    }

    /// Get template definition
    pub fn get_template(class_name: &str) -> Option<&'static TemplateDefinition> {
        Self::get_model(class_name)
            .and_then(|entry| entry.template_name)
            .and_then(|name| TEMPLATES_BY_NAME.get(name).copied())
    }

    /// Check if model needs template
    pub fn needs_template(class_name: &str) -> bool {
        Self::get_model(class_name)
            .map(|entry| entry.template_name.is_some())
            .unwrap_or(false)
    }

    /// Get endpoint type
    pub fn get_endpoint_type(class_name: &str) -> EndpointType {
        Self::get_model(class_name)
            .map(|entry| entry.endpoint_type.clone())
            .unwrap_or(EndpointType::ChatCompletions)
    }

    /// Check if model uses completions endpoint
    pub fn uses_completions_endpoint(class_name: &str) -> bool {
        matches!(
            Self::get_endpoint_type(class_name),
            EndpointType::Completions
        )
    }

    /// Get all models by provider
    pub fn get_models_by_provider(provider_type: ProviderType) -> Vec<&'static str> {
        MODEL_REGISTRY
            .iter()
            .filter(|entry| entry.provider_type == provider_type)
            .map(|entry| entry.class_name)
            .collect()
    }

    /// Get all templated models
    pub fn get_templated_models() -> Vec<&'static str> {
        MODEL_REGISTRY
            .iter()
            .filter(|entry| entry.template_name.is_some())
            .map(|entry| entry.class_name)
            .collect()
    }

    /// Get all models using specific endpoint
    pub fn get_models_by_endpoint(endpoint_type: EndpointType) -> Vec<&'static str> {
        MODEL_REGISTRY
            .iter()
            .filter(|entry| entry.endpoint_type == endpoint_type)
            .map(|entry| entry.class_name)
            .collect()
    }

    /// Get template by name
    pub fn get_template_by_name(name: &str) -> Option<&'static TemplateDefinition> {
        TEMPLATES_BY_NAME.get(name).copied()
    }

    /// Get all available templates
    pub fn get_all_templates() -> &'static [TemplateDefinition] {
        TEMPLATES
    }

    /// Get registry statistics
    pub fn get_stats() -> RegistryStats {
        let total_models = MODEL_REGISTRY.len();
        let templated_models = Self::get_templated_models().len();
        let completions_models = Self::get_models_by_endpoint(EndpointType::Completions).len();
        let chat_completions_models =
            Self::get_models_by_endpoint(EndpointType::ChatCompletions).len();

        RegistryStats {
            total_models,
            templated_models,
            completions_models,
            chat_completions_models,
            openai_models: Self::get_models_by_provider(ProviderType::OpenAI).len(),
            google_models: Self::get_models_by_provider(ProviderType::Google).len(),
            anthropic_models: Self::get_models_by_provider(ProviderType::Anthropic).len(),
            total_templates: TEMPLATES.len(),
        }
    }
}

#[derive(Debug)]
pub struct RegistryStats {
    pub total_models: usize,
    pub templated_models: usize,
    pub completions_models: usize,
    pub chat_completions_models: usize,
    pub openai_models: usize,
    pub google_models: usize,
    pub anthropic_models: usize,
    pub total_templates: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_basic_operations() {
        // Test model lookup
        assert!(ModelRegistry::get_model("ChatMlChat").is_some());
        assert!(ModelRegistry::get_model("NonExistentModel").is_none());

        // Test provider type
        assert_eq!(
            ModelRegistry::get_provider_type("ChatMlChat"),
            ProviderType::OpenAI
        );
        assert_eq!(
            ModelRegistry::get_provider_type("ChatGemini"),
            ProviderType::Google
        );
        assert_eq!(
            ModelRegistry::get_provider_type("ChatSonnet"),
            ProviderType::Anthropic
        );

        // Test template operations
        assert!(ModelRegistry::needs_template("ChatMlChat"));
        assert!(!ModelRegistry::needs_template("BaseOpenAIChat"));
        assert!(ModelRegistry::get_template_content("ChatMlChat").is_some());

        // Test endpoint type
        assert!(ModelRegistry::uses_completions_endpoint("ChatMlChat"));
        assert!(!ModelRegistry::uses_completions_endpoint("BaseOpenAIChat"));
    }

    #[test]
    fn test_registry_collections() {
        let templated = ModelRegistry::get_templated_models();
        assert!(templated.contains(&"ChatMlChat"));
        assert!(!templated.contains(&"BaseOpenAIChat"));

        let openai_models = ModelRegistry::get_models_by_provider(ProviderType::OpenAI);
        assert!(openai_models.contains(&"ChatMlChat"));
        assert!(openai_models.contains(&"BaseOpenAIChat"));

        let completions_models = ModelRegistry::get_models_by_endpoint(EndpointType::Completions);
        assert!(completions_models.contains(&"ChatMlChat"));
        assert!(!completions_models.contains(&"BaseOpenAIChat"));
    }

    #[test]
    fn test_template_lookup() {
        let chatml_template = ModelRegistry::get_template_by_name("CHATML");
        assert!(chatml_template.is_some());
        assert_eq!(chatml_template.unwrap().name, "CHATML");

        let all_templates = ModelRegistry::get_all_templates();
        assert!(!all_templates.is_empty());
    }

    #[test]
    fn test_registry_stats() {
        let stats = ModelRegistry::get_stats();
        assert!(stats.total_models > 0);
        assert!(stats.templated_models > 0);
        assert!(stats.total_templates > 0);
        assert_eq!(
            stats.completions_models + stats.chat_completions_models,
            stats.total_models
        );
    }
}
