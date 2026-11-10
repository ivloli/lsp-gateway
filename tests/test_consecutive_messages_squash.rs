use lsp_gateway::handlers::chat::{
    ChatCompletionRequest, ChatMessage, MessageContent, ToProviderRequest,
};
use std::collections::HashMap;

#[test]
fn test_consecutive_user_messages_are_squashed() {
    // Test case: consecutive user messages should be merged
    let request = ChatCompletionRequest {
        model: "gemini-2.5-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text("First user message".to_string()),
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Second user message".to_string()),
                name: None,
            },
            ChatMessage {
                role: "model".to_string(),
                content: MessageContent::Text("Model response".to_string()),
                name: None,
            },
        ],
        stream: false,
        temperature: None,
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra: HashMap::new(),
    };

    let processed = request.to_provider_request("gemini-2.5-flash", "ChatGeminiV2");
    let processed_request: ChatCompletionRequest = serde_json::from_value(processed).unwrap();

    // Should have 2 messages after squashing (merged user + model)
    assert_eq!(processed_request.messages.len(), 2);
    assert_eq!(processed_request.messages[0].role, "user");
    assert_eq!(processed_request.messages[1].role, "model");

    // First message should contain both user messages merged
    if let MessageContent::Text(content) = &processed_request.messages[0].content {
        assert!(content.contains("First user message"));
        assert!(content.contains("Second user message"));
    } else {
        panic!("Expected text content");
    }
}

#[test]
fn test_system_message_preserved_for_gemini() {
    // Test case: system message at the beginning should be preserved
    let request = ChatCompletionRequest {
        model: "gemini-2.5-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: MessageContent::Text("You are a helpful assistant".to_string()),
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Hello".to_string()),
                name: None,
            },
            ChatMessage {
                role: "model".to_string(),
                content: MessageContent::Text("Hi there!".to_string()),
                name: None,
            },
        ],
        stream: false,
        temperature: None,
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra: HashMap::new(),
    };

    let processed = request.to_provider_request("gemini-2.5-flash", "ChatGeminiV2");
    let processed_request: ChatCompletionRequest = serde_json::from_value(processed).unwrap();

    // Should have 3 messages (system, user, model)
    assert_eq!(processed_request.messages.len(), 3);
    assert_eq!(processed_request.messages[0].role, "system");
    assert_eq!(processed_request.messages[1].role, "user");
    assert_eq!(processed_request.messages[2].role, "model");
}

#[test]
fn test_mid_prompt_system_messages_converted_to_user() {
    // Test case: system messages in the middle should be converted to user
    let request = ChatCompletionRequest {
        model: "gemini-2.5-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text("First message".to_string()),
                name: None,
            },
            ChatMessage {
                role: "system".to_string(),
                content: MessageContent::Text("Mid-prompt system message".to_string()),
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Second message".to_string()),
                name: None,
            },
        ],
        stream: false,
        temperature: None,
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra: HashMap::new(),
    };

    let processed = request.to_provider_request("gemini-2.5-flash", "ChatGeminiV2");
    let processed_request: ChatCompletionRequest = serde_json::from_value(processed).unwrap();

    // Mid-prompt system message should be converted to user and merged with adjacent user messages
    assert!(processed_request
        .messages
        .iter()
        .all(|m| m.role != "system"));

    // Should have 1 message after merging all consecutive user messages
    assert_eq!(processed_request.messages.len(), 1);
    assert_eq!(processed_request.messages[0].role, "user");
}

#[test]
fn test_role_alternation_with_placeholders() {
    // Test case: ensure proper role alternation with placeholders
    let request = ChatCompletionRequest {
        model: "gemini-2.5-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: MessageContent::Text("System prompt".to_string()),
                name: None,
            },
            ChatMessage {
                role: "model".to_string(),
                content: MessageContent::Text("Model starts".to_string()),
                name: None,
            },
        ],
        stream: false,
        temperature: None,
        max_tokens: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
        extra: HashMap::new(),
    };

    let processed = request.to_provider_request("gemini-2.5-flash", "ChatGeminiV2");
    let processed_request: ChatCompletionRequest = serde_json::from_value(processed).unwrap();

    // Should insert a user placeholder between system and model
    assert!(processed_request.messages.len() >= 3);
    assert_eq!(processed_request.messages[0].role, "system");
    assert_eq!(processed_request.messages[1].role, "user"); // placeholder
    assert_eq!(processed_request.messages[2].role, "model");
}
