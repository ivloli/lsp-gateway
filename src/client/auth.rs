use crate::models::{ModelConfig, ModelProvider};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::env;

/// Authentication configuration for different providers
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub api_key: Option<String>,
}

/// Static provider authentication configuration
static PROVIDER_AUTH_MAP: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert("openai", "OPENAI_CHAT_API_KEY");
    map.insert("openai_nsfw", "OPENAI_NSFW_API_KEY");
    map.insert("google", "GOOGLE_AI_API_KEY");
    map.insert("lepton", "LEPTON_API_KEY");
    map.insert("sonnet", "SONNET_API_KEY");
    map.insert("sonnet-provider-2", "SONNET_API_KEY_BACKUP");
    map.insert("sonnet-provider-3", "SONNET_API_KEY_BACKUP_2");
    map.insert("other-provider-1", "OTHER_PROVIDER_1_API_KEY");
    map.insert("other-provider-2", "OTHER_PROVIDER_2_API_KEY");
    map.insert("deepseek", "DEEPSEEK_API_KEY");
    map.insert("openrouter", "OPENROUTER_API_KEY");
    map.insert("novita", "NOVITA_API_KEY");
    map.insert("featherless", "FEATHERLESS_API_KEY");
    map.insert("paintbot", "PAINT_BOT_KEY");
    map
});

/// Get authentication configuration for a provider (legacy function)
pub fn get_auth_config(provider_type: &str) -> AuthConfig {
    let env_var = PROVIDER_AUTH_MAP.get(provider_type);
    let api_key = env_var.and_then(|var| env::var(var).ok());

    AuthConfig { api_key }
}

/// Get authentication configuration with priority-based API key selection
/// Priority order:
/// 1. ModelConfig.apiKeyEnvName
/// 2. SelectedProvider.apiKeyEnvName (from ModelProvider)
/// 3. Default Provider API Key (from static mapping)
pub fn get_auth_config_with_priority(
    model_config: &ModelConfig,
    selected_provider: Option<&ModelProvider>,
    provider_type: &str,
) -> AuthConfig {
    // Priority 1: ModelConfig.apiKeyEnvName
    if let Some(api_key_env_name) = &model_config.api_key_env_name {
        if let Ok(api_key) = env::var(api_key_env_name) {
            return AuthConfig {
                api_key: Some(api_key),
            };
        }
    }

    // Priority 2: SelectedProvider.apiKeyEnvName (from ModelProvider)
    if let Some(provider) = selected_provider {
        if let Some(api_key_env_name) = &provider.api_key_env_name {
            if let Ok(api_key) = env::var(api_key_env_name) {
                return AuthConfig {
                    api_key: Some(api_key),
                };
            }
        }
    }

    // Priority 3: Default Provider API Key (fallback to existing logic)
    get_auth_config(provider_type)
}

/// Get default base URL for a provider
pub fn get_default_base_url(provider: &str) -> Option<String> {
    match provider {
        "openai" | "openai_nsfw" => Some("https://api.openai.com/v1".to_string()),
        "google" => Some("https://api.openai.com/v1".to_string()),
        "openrouter" => Some("https://openrouter.ai/api/v1".to_string()),
        "featherless" => Some("https://api.featherless.ai/v1".to_string()),
        "paintbot" => Some("https://oneapi.paintbot.top/v1".to_string()),
        _ => None,
    }
}
