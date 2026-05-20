use serde_json::Value;

// these are mostly for testing
// todo replace with actual RLM stuff / classification stuff down the line

pub(crate) fn uppercase_request_message_text(request: &mut Value) {
    let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };

    for message in messages {
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        transform_content_text(content, |text| text.to_uppercase());
    }
}

pub(crate) fn lowercase_assistant_output(response: &mut Value) {
    let Some(choices) = response.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };

    for choice in choices {
        let Some(message) = choice.get_mut("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        transform_content_text(content, |text| text.to_lowercase());
    }
}

fn transform_content_text(content: &mut Value, transform: fn(&str) -> String) {
    match content {
        Value::String(text) => {
            *text = transform(text);
        }
        Value::Array(parts) => {
            for part in parts {
                let is_text_part = part.get("type").and_then(Value::as_str) == Some("text");
                if !is_text_part {
                    continue;
                }
                if let Some(Value::String(text)) = part.get_mut("text") {
                    *text = transform(text);
                }
            }
        }
        _ => {}
    }
}
