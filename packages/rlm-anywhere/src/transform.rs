use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestAssistantMessageContentPart,
    ChatCompletionRequestDeveloperMessageContent, ChatCompletionRequestDeveloperMessageContentPart,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestSystemMessageContentPart,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    CreateChatCompletionRequest, CreateChatCompletionResponse, Role,
};

// these are mostly for testing
// todo replace with actual RLM stuff / classification stuff down the line

pub(crate) fn uppercase_request_message_text(request: &mut CreateChatCompletionRequest) {
    for message in &mut request.messages {
        match message {
            ChatCompletionRequestMessage::Developer(message) => {
                transform_developer_content(&mut message.content, str::to_uppercase);
            }
            ChatCompletionRequestMessage::System(message) => {
                transform_system_content(&mut message.content, str::to_uppercase);
            }
            ChatCompletionRequestMessage::User(message) => {
                transform_user_content(&mut message.content, str::to_uppercase);
            }
            ChatCompletionRequestMessage::Assistant(message) => {
                if let Some(content) = &mut message.content {
                    transform_assistant_content(content, str::to_uppercase);
                }
            }
            ChatCompletionRequestMessage::Tool(_) | ChatCompletionRequestMessage::Function(_) => {}
        }
    }
}

pub(crate) fn lowercase_assistant_output(response: &mut CreateChatCompletionResponse) {
    for choice in &mut response.choices {
        if choice.message.role != Role::Assistant {
            continue;
        }
        if let Some(content) = &mut choice.message.content {
            *content = content.to_lowercase();
        }
    }
}

fn transform_developer_content(
    content: &mut ChatCompletionRequestDeveloperMessageContent,
    transform: fn(&str) -> String,
) {
    match content {
        ChatCompletionRequestDeveloperMessageContent::Text(text) => {
            *text = transform(text);
        }
        ChatCompletionRequestDeveloperMessageContent::Array(parts) => {
            for part in parts {
                let ChatCompletionRequestDeveloperMessageContentPart::Text(part) = part;
                transform_text_part(part, transform);
            }
        }
    }
}

fn transform_system_content(
    content: &mut ChatCompletionRequestSystemMessageContent,
    transform: fn(&str) -> String,
) {
    match content {
        ChatCompletionRequestSystemMessageContent::Text(text) => {
            *text = transform(text);
        }
        ChatCompletionRequestSystemMessageContent::Array(parts) => {
            for part in parts {
                let ChatCompletionRequestSystemMessageContentPart::Text(part) = part;
                transform_text_part(part, transform);
            }
        }
    }
}

fn transform_user_content(
    content: &mut ChatCompletionRequestUserMessageContent,
    transform: fn(&str) -> String,
) {
    match content {
        ChatCompletionRequestUserMessageContent::Text(text) => {
            *text = transform(text);
        }
        ChatCompletionRequestUserMessageContent::Array(parts) => {
            for part in parts {
                if let ChatCompletionRequestUserMessageContentPart::Text(part) = part {
                    transform_text_part(part, transform);
                }
            }
        }
    }
}

fn transform_assistant_content(
    content: &mut ChatCompletionRequestAssistantMessageContent,
    transform: fn(&str) -> String,
) {
    match content {
        ChatCompletionRequestAssistantMessageContent::Text(text) => {
            *text = transform(text);
        }
        ChatCompletionRequestAssistantMessageContent::Array(parts) => {
            for part in parts {
                if let ChatCompletionRequestAssistantMessageContentPart::Text(part) = part {
                    transform_text_part(part, transform);
                }
            }
        }
    }
}

fn transform_text_part(
    part: &mut ChatCompletionRequestMessageContentPartText,
    transform: fn(&str) -> String,
) {
    part.text = transform(&part.text);
}
