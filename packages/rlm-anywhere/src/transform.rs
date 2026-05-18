use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{lowercase_assistant_output, uppercase_request_message_text};

    #[test]
    fn uppercases_only_message_text_fields() {
        let mut request = json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "system",
                    "content": "stay concise",
                    "metadata": { "label": "do-not-change" }
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "hello there" },
                        { "type": "image_url", "image_url": { "url": "https://example.test/image.png" } }
                    ]
                }
            ],
            "tool_choice": "auto",
            "x_unknown": "preserve me"
        });

        uppercase_request_message_text(&mut request);

        assert_eq!(request["model"], "local-model");
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(request["x_unknown"], "preserve me");
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["messages"][0]["content"], "STAY CONCISE");
        assert_eq!(request["messages"][0]["metadata"]["label"], "do-not-change");
        assert_eq!(request["messages"][1]["content"][0]["text"], "HELLO THERE");
        assert_eq!(
            request["messages"][1]["content"][1]["image_url"]["url"],
            "https://example.test/image.png"
        );
    }

    #[test]
    fn lowercases_assistant_output_content() {
        let mut response = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "HELLO FROM UPSTREAM"
                    }
                },
                {
                    "index": 1,
                    "message": {
                        "role": "tool",
                        "content": "DO NOT LOWERCASE"
                    }
                }
            ]
        });

        lowercase_assistant_output(&mut response);

        assert_eq!(
            response["choices"][0]["message"]["content"],
            "hello from upstream"
        );
        assert_eq!(
            response["choices"][1]["message"]["content"],
            "DO NOT LOWERCASE"
        );
    }
}
