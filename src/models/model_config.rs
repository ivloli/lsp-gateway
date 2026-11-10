use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    #[sqlx(rename = "baseUrl")]
    pub base_url: String,
    #[sqlx(rename = "promptManagerId")]
    pub prompt_manager_id: Option<String>,
    #[sqlx(rename = "modelName")]
    pub model_name: String,
    pub parameters: JsonValue,
    #[sqlx(rename = "modelClassName")]
    pub model_class_name: String,
    #[sqlx(rename = "contextLimit")]
    pub context_limit: i32,
    pub live: bool,
    pub display: bool,
    #[sqlx(rename = "displayLabel")]
    pub display_label: Option<String>,
    #[sqlx(rename = "displayIcon")]
    pub display_icon: Option<String>,
    #[sqlx(rename = "displayDescription")]
    pub display_description: Option<String>,
    #[sqlx(rename = "displayUrl")]
    pub display_url: Option<String>,
    #[sqlx(rename = "displayCost")]
    pub display_cost: Option<String>,
    #[sqlx(rename = "displayMaxTokens")]
    pub display_max_tokens: Option<i32>,
    #[sqlx(rename = "displayTags")]
    pub display_tags: Option<Vec<String>>,
    #[sqlx(rename = "displayParams")]
    pub display_params: Option<Vec<String>>,
    #[sqlx(rename = "modelRetryConfigId")]
    pub model_retry_config_id: Option<String>,
    #[sqlx(rename = "modelProviderGroup")]
    pub model_provider_group: Option<String>,
    #[sqlx(rename = "huggingFaceModelName")]
    pub hugging_face_model_name: Option<String>,
    #[sqlx(rename = "apiKeyEnvName")]
    pub api_key_env_name: Option<String>,
}

impl ModelConfig {
    pub async fn find_by_id(pool: &sqlx::PgPool, id: &str) -> Result<Option<Self>, sqlx::Error> {
        let result = sqlx::query_as::<_, ModelConfig>(
            r#"
            SELECT id, provider, "baseUrl", "promptManagerId", "modelName", parameters, 
                   "modelClassName", "contextLimit", live, display, "displayLabel", 
                   "displayIcon", "displayDescription", "displayUrl", "displayCost", 
                   "displayMaxTokens", "displayTags", "displayParams", "modelRetryConfigId", 
                   "modelProviderGroup", "huggingFaceModelName", "apiKeyEnvName"
            FROM "ModelConfig" 
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|db_error| {
            error!(
                "❌ Failed to query model config by ID '{}': {}",
                id, db_error
            );
            db_error
        })?;

        Ok(result)
    }

    /// Extract stream_no_token_timeout_ms from parameters field
    /// Returns None if not present or invalid
    pub fn get_stream_no_token_timeout_ms(&self) -> Option<u64> {
        self.parameters
            .as_object()?
            .get("stream_no_token_timeout_ms")?
            .as_u64()
    }

    /// Extract responseFinishTimeout from parameters field
    /// Returns None if not present or invalid
    pub fn get_response_finish_timeout_ms(&self) -> Option<u64> {
        self.parameters
            .as_object()?
            .get("responseFinishTimeout")?
            .as_u64()
    }
}
