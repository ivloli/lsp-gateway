pub mod adapters;
pub mod client;
pub mod utils;

pub use adapters::{
    AnthropicAdapter, EndpointType, GoogleAdapter, GoogleProxyAdapter, OpenAIAdapter,
    ProviderAdapter,
};
pub use client::GenAIClient;

/// Provider types supported by the GenAI system
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderType {
    OpenAI,
    Anthropic,
    Google,
    Unknown,
}

impl ProviderType {
    /// Map model class name to provider type using centralized registry
    pub fn from_model_class_name(model_class_name: &str) -> Self {
        crate::config::ModelRegistry::get_provider_type(model_class_name)
    }

    /// Get the provider name string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Unknown => "unknown",
        }
    }
}
