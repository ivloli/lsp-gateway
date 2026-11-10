// Test extra_body handling in LSP Gateway
use lsp_gateway::genai::adapters::google_common::convert_messages_to_gemini_with_system;
use lsp_gateway::handlers::chat::{ChatMessage, ContentPart, ImageUrl, MessageContent};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_gemini_system_instruction_extraction() {
    // Test that gemini_system_instruction is correctly extracted from extra_body
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: MessageContent::Text("Hello".to_string()),
        name: None,
    }];

    let mut extra = HashMap::new();
    extra.insert(
        "extra_body".to_string(),
        json!({
            "gemini_system_instruction": {
                "parts": [
                    { "text": "You are a helpful assistant" }
                ]
            }
        }),
    );

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // Verify contents
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");

    // Verify system instruction was extracted
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    assert_eq!(sys_inst["parts"][0]["text"], "You are a helpful assistant");
}

#[test]
fn test_system_message_auto_extraction() {
    // Test that system messages are automatically extracted to systemInstruction
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("You are helpful".to_string()),
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

    // System instruction should be automatically extracted
    assert!(system_instruction.is_some());
    let sys_inst = system_instruction.unwrap();
    assert_eq!(sys_inst["parts"][0]["text"], "You are helpful");

    // Only user message should be in contents
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[0]["parts"][0]["text"], "Hello");
}

#[test]
fn test_system_message_skipped_when_instruction_present() {
    // Test that system messages are skipped when gemini_system_instruction is provided
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: MessageContent::Text("This should be skipped".to_string()),
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

    // System instruction should be extracted
    assert!(system_instruction.is_some());
    assert_eq!(
        system_instruction.unwrap()["parts"][0]["text"],
        "Use this instead"
    );

    // Only user message should be in contents (system message skipped)
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[0]["parts"][0]["text"], "Hello");
}

#[test]
fn test_multimodal_content_with_image() {
    // Test that multimodal content (images) is correctly converted
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "Describe this image".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/jpeg;base64,/9j/4AAQSkZJRg==".to_string(),
                    detail: None,
                },
            },
        ]),
        name: None,
    }];

    let extra = HashMap::new();

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, _) = result.unwrap();

    assert_eq!(contents.len(), 1);
    let parts = contents[0]["parts"].as_array().unwrap();

    // Should have text part and inline data part
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "Describe this image");

    // Check inline data
    assert!(parts[1].get("inlineData").is_some());
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/jpeg");
    assert_eq!(parts[1]["inlineData"]["data"], "/9j/4AAQSkZJRg==");
}

#[test]
fn test_mime_type_detection() {
    // Test MIME type auto-detection from file extensions
    let test_cases = vec![
        ("image.png", "image/png"),
        ("photo.jpg", "image/jpeg"),
        ("pic.webp", "image/webp"),
        ("audio.mp3", "audio/mpeg"),
        ("sound.wav", "audio/wav"),
        ("video.mp4", "video/mp4"),
    ];

    for (url, expected_mime) in test_cases {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: format!("gs://bucket/{}", url),
                    detail: None,
                },
            }]),
            name: None,
        }];

        let extra = HashMap::new();
        let result = convert_messages_to_gemini_with_system(&messages, &extra);
        assert!(result.is_ok());

        let (contents, _) = result.unwrap();
        let parts = contents[0]["parts"].as_array().unwrap();

        // Should detect correct MIME type
        assert_eq!(parts[0]["fileData"]["mimeType"], expected_mime);
    }
}

#[test]
fn test_combined_system_instruction_and_multimodal() {
    // Test that both system instruction and multimodal content work together
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "What is this?".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,iVBORw0KGg==".to_string(),
                    detail: None,
                },
            },
        ]),
        name: None,
    }];

    let mut extra = HashMap::new();
    extra.insert(
        "extra_body".to_string(),
        json!({
            "gemini_system_instruction": {
                "parts": [
                    { "text": "Be concise" }
                ]
            }
        }),
    );

    let result = convert_messages_to_gemini_with_system(&messages, &extra);
    assert!(result.is_ok());

    let (contents, system_instruction) = result.unwrap();

    // System instruction should be extracted
    assert!(system_instruction.is_some());
    assert_eq!(
        system_instruction.unwrap()["parts"][0]["text"],
        "Be concise"
    );

    // Message should have both text and image
    assert_eq!(contents.len(), 1);
    let parts = contents[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "What is this?");
    assert!(parts[1].get("inlineData").is_some());
}
