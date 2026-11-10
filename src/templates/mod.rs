use minijinja::{context, Environment};

/// Get template string for a given model class name using centralized registry
pub fn get_template_for_model(model_class_name: &str) -> Option<&'static str> {
    crate::config::ModelRegistry::get_template_content(model_class_name)
}

/// Render a template with messages
pub fn render_template(template: &str, messages: &[serde_json::Value]) -> Result<String, String> {
    let mut env = Environment::new();

    // Add the template
    env.add_template("chat", template)
        .map_err(|e| format!("Failed to add template: {}", e))?;

    // Get the template
    let tmpl = env
        .get_template("chat")
        .map_err(|e| format!("Failed to get template: {}", e))?;

    // Render with context
    let rendered = tmpl
        .render(context! { messages => messages })
        .map_err(|e| format!("Failed to render template: {}", e))?;

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_chatml_template() {
        let messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "Hello"}),
            json!({"role": "assistant", "content": "Hi there!"}),
        ];

        let chatml_template =
            crate::config::ModelRegistry::get_template_content("ChatMlChat").unwrap();
        let result = render_template(chatml_template, &messages).unwrap();
        assert!(result.contains("<|im_start|>system"));
        assert!(result.contains("<|im_start|>user"));
        assert!(result.contains("<|im_start|>assistant\nHi there!"));
        // Check if it ends with the assistant prompt (may or may not have trailing newline)
        assert!(
            result.trim().ends_with("<|im_start|>assistant")
                || result.ends_with("<|im_start|>assistant\n")
        );
    }

    #[test]
    fn test_get_template_for_model() {
        // Test ChatML models
        assert!(get_template_for_model("ChatMlChat").is_some());
        assert!(get_template_for_model("ChatCustomMIChat").is_some());

        // Test Llama3 models
        assert!(get_template_for_model("Llama3Chat").is_some());

        // Test Mixtral models (corrected names)
        assert!(get_template_for_model("ChatMixtral").is_some());
        assert!(get_template_for_model("ChatMixtralPlain").is_some());

        // Test Gemma3 models
        assert!(get_template_for_model("ChatGemma3").is_some());

        // Test standard OpenAI models (should return None)
        assert!(get_template_for_model("BaseOpenAIChat").is_none());
        assert!(get_template_for_model("LSPGatewayChat").is_none());
        assert!(get_template_for_model("ChatOpenAI").is_none());
    }

    #[test]
    fn test_llama3_template_with_system() {
        let messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "Hello"}),
        ];

        let llama3_template =
            crate::config::ModelRegistry::get_template_content("Llama3Chat").unwrap();
        let result = render_template(llama3_template, &messages).unwrap();
        assert!(result.contains("<|begin_of_text|>"));
        assert!(result.contains("<|start_header_id|>system<|end_header_id|>"));
        assert!(result.contains("<|start_header_id|>user<|end_header_id|>"));
        assert!(result.contains("<|eot_id|>"));
    }

    #[test]
    fn test_gemma3_template() {
        let messages = vec![
            json!({"role": "user", "content": "Hello"}),
            json!({"role": "assistant", "content": "Hi!"}),
        ];

        let gemma3_template =
            crate::config::ModelRegistry::get_template_content("ChatGemma3").unwrap();
        let result = render_template(gemma3_template, &messages).unwrap();
        assert!(result.contains("<start_of_turn>user"));
        assert!(result.contains("<start_of_turn>model"));
        // Check if it ends with the model prompt (may or may not have trailing newline)
        assert!(
            result.trim().ends_with("<start_of_turn>model")
                || result.ends_with("<start_of_turn>model\n")
        );
    }
}
