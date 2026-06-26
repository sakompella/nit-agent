use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMessage {
    role: Option<String>,
    text: String,
    raw: Value,
}

impl ContextMessage {
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn raw(&self) -> &Value {
        &self.raw
    }

    fn from_value(value: Value) -> Self {
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let text = extract_content_text(value.get("content"));

        Self {
            role,
            text,
            raw: value,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextStore {
    messages: Vec<ContextMessage>,
}

impl ContextStore {
    #[must_use]
    pub fn from_chat_messages(messages: &[Value]) -> Self {
        let messages = messages
            .iter()
            .cloned()
            .map(ContextMessage::from_value)
            .collect();

        Self { messages }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&ContextMessage> {
        self.messages.get(index)
    }

    #[must_use]
    pub fn slice(&self, start: usize, end: usize) -> &[ContextMessage] {
        let start = start.min(self.messages.len());
        let end = end.min(self.messages.len());

        if start > end {
            return &[];
        }

        &self.messages[start..end]
    }

    /// Case-insensitively search message text and roles; returns each match
    /// paired with its zero-based index in the store.
    #[must_use]
    pub fn grep_indexed(&self, needle: &str) -> Vec<(usize, &ContextMessage)> {
        if needle.is_empty() {
            return Vec::new();
        }

        let needle = needle.to_lowercase();
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                message.text.to_lowercase().contains(&needle)
                    || message
                        .role
                        .as_deref()
                        .is_some_and(|role| role.to_lowercase().contains(&needle))
            })
            .collect()
    }

    #[must_use]
    pub fn grep(&self, needle: &str) -> Vec<&ContextMessage> {
        self.grep_indexed(needle)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[must_use]
    pub fn describe(&self) -> ContextSummary {
        let text_bytes = self.messages.iter().map(|message| message.text.len()).sum();
        let roles = self
            .messages
            .iter()
            .filter_map(|message| message.role.as_deref())
            .map(ToOwned::to_owned)
            .collect();

        ContextSummary {
            messages: self.messages.len(),
            text_bytes,
            roles,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ContextSummary {
    pub messages: usize,
    pub text_bytes: usize,
    pub roles: Vec<String>,
}

fn extract_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(extract_content_part_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) if other.is_null() => String::new(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn extract_content_part_text(part: &Value) -> Option<String> {
    match part {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("content"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}
