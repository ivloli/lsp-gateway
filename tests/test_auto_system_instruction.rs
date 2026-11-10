// Test automatic system instruction extraction from system messages
use lsp_gateway::genai::adapters::google_common::convert_messages_to_gemini_with_system;
use lsp_gateway::handlers::chat::{ChatMessage, MessageContent};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_auto_extract_single_system_message() {
    // Test that a single system message is automatically extracted to systemInstruction
    let messages = vec![
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
    ];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // System instruction should be automatically constructed
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    assert_eq!(sys_inst["parts"][0]["text"], "You are a helpful assistant");

    // Only user message should be in contents
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[0]["parts"][0]["text"], "Hello");
}

#[test]
fn test_auto_extract_multiple_system_messages() {
    // Test that multiple system messages are merged into one systemInstruction
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("You are helpful".to_string()),
            name: None,
        },
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("Be concise".to_string()),
            name: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Test".to_string()),
            name: None,
        },
    ];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // System instruction should have both parts
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    let parts = sys_inst["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "You are helpful");
    assert_eq!(parts[1]["text"], "Be concise");

    // Only user message in contents
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
}

#[test]
fn test_explicit_overrides_auto_extraction() {
    // Test that explicit extra_body.gemini_system_instruction overrides auto-extraction
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("This should be ignored".to_string()),
            name: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Hello".to_string()),
            name: None,
        },
    ];

    let mut extra = HashMap::new();
    extra.insert(
        "extra_body".to_string(),
        json!({
            "gemini_system_instruction": {
                "parts": [
                    { "text": "Use this instead" }
                ]
            }
        }),
    );

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // Should use the explicit instruction, not the one from messages
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    assert_eq!(sys_inst["parts"][0]["text"], "Use this instead");

    // Only user message in contents (system message skipped)
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
}

#[test]
fn test_auto_extract_mixed_messages() {
    // Test system messages interspersed with other messages
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("First instruction".to_string()),
            name: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Question 1".to_string()),
            name: None,
        },
        ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("Answer 1".to_string()),
            name: None,
        },
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("Second instruction".to_string()),
            name: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Question 2".to_string()),
            name: None,
        },
    ];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // Both system messages should be extracted
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    let parts = sys_inst["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "First instruction");
    assert_eq!(parts[1]["text"], "Second instruction");

    // Should have user-assistant-user messages
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[0]["parts"][0]["text"], "Question 1");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][0]["text"], "Answer 1");
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(contents[2]["parts"][0]["text"], "Question 2");
}

#[test]
fn test_no_system_messages() {
    // Test that no system instruction is created when there are no system messages
    let messages = vec![
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Hello".to_string()),
            name: None,
        },
        ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("Hi".to_string()),
            name: None,
        },
    ];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // No system instruction should be created
    assert!(system_instruction.is_none());

    // All messages should be in contents
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
}

#[test]
fn test_auto_extract_with_multimodal_system_message() {
    // Test that multimodal system messages (though unusual) are handled
    use lsp_gateway::handlers::chat::ContentPart;

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "You are helpful".to_string(),
                },
                ContentPart::Text {
                    text: "Be concise".to_string(),
                },
            ]),
            name: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Text("Hello".to_string()),
            name: None,
        },
    ];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // System instruction should extract all text parts
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    let parts = sys_inst["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "You are helpful");
    assert_eq!(parts[1]["text"], "Be concise");

    // Only user message in contents
    assert_eq!(contents.len(), 1);
}
