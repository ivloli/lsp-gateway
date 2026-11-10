// Test Google adapter handling of extra_body.google fields
use lsp_gateway::genai::adapters::{GoogleAdapter, ProviderAdapter};
use lsp_gateway::handlers::chat::{ChatCompletionRequest, ChatMessage, MessageContent};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_no_extra_body_google_fields() {
    // Test that adapter works correctly when no extra_body.google fields are present
    let adapter = GoogleAdapter::new();

    let extra = HashMap::new();

    let request = ChatCompletionRequest {
        model: "gemini-1.5-pro".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Hello".to_string()),
            name: None,
        }],
        stream: false,
        temperature: Some(0.7),
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra,
    };

    let result = adapter.transform_chat_request(
        &request,
        "gemini-1.5-pro",
        "ChatGemini",
        "https://generativelanguage.googleapis.com",
    );
    assert!(result.is_ok());

    let gemini_request = result.unwrap();

    // Should have basic fields
    assert!(gemini_request.get("contents").is_some());
    assert!(gemini_request.get("model").is_some());

    // Should not have thinking_config
    assert!(gemini_request.get("thinkingConfig").is_none());
}

#[test]
fn test_empty_google_object() {
    // Test that empty google object doesn't cause issues
    let adapter = GoogleAdapter::new();

    let mut extra = HashMap::new();
    extra.insert(
        "extra_body".to_string(),
        json!({
            "google": {}
        }),
    );

    let request = ChatCompletionRequest {
        model: "gemini-1.5-pro".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Test".to_string()),
            name: None,
        }],
        stream: false,
        temperature: None,
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra,
    };

    let result = adapter.transform_chat_request(
        &request,
        "gemini-1.5-pro",
        "ChatGemini",
        "https://generativelanguage.googleapis.com",
    );
    assert!(result.is_ok());

    let gemini_request = result.unwrap();

    // Should still have basic structure
    assert!(gemini_request.get("contents").is_some());
    assert!(gemini_request.get("model").is_some());
}
