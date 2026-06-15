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
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
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
    pub(crate) fn grep_indexed(&self, needle: &str) -> Vec<(usize, &ContextMessage)> {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ContextStore;

    #[test]
    fn builds_context_from_chat_messages() {
        let messages = vec![
            json!({"role": "system", "content": "Be direct."}),
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "Find the answer"},
                    {"type": "image_url", "image_url": {"url": "https://example.test/image.png"}},
                    "extra text"
                ],
                "unknown": true
            }),
        ];

        let store = ContextStore::from_chat_messages(&messages);

        assert_eq!(store.len(), 2);
        assert_eq!(
            store.get(0).and_then(super::ContextMessage::role),
            Some("system")
        );
        assert_eq!(
            store.get(1).map(super::ContextMessage::text),
            Some("Find the answer\nextra text")
        );
        assert_eq!(
            store.get(1).map(super::ContextMessage::raw),
            messages.get(1)
        );
    }

    #[test]
    fn slice_clamps_to_available_messages() {
        let messages = vec![
            json!({"role": "user", "content": "one"}),
            json!({"role": "assistant", "content": "two"}),
        ];
        let store = ContextStore::from_chat_messages(&messages);

        assert_eq!(store.slice(0, 99).len(), 2);
        assert!(store.slice(2, 1).is_empty());
    }

    #[test]
    fn grep_matches_role_and_text_case_insensitively() {
        let messages = vec![
            json!({"role": "user", "content": "Need budget details"}),
            json!({"role": "assistant", "content": "Done"}),
        ];
        let store = ContextStore::from_chat_messages(&messages);

        let content_matches = store.grep("BUDGET");
        let role_matches = store.grep("assistant");

        assert_eq!(content_matches.len(), 1);
        assert_eq!(role_matches.len(), 1);
        assert!(store.grep("").is_empty());
    }

    #[test]
    fn grep_indexed_returns_correct_positions() {
        let messages = vec![
            json!({"role": "user", "content": "Need budget details"}),
            json!({"role": "assistant", "content": "Done"}),
            json!({"role": "user", "content": "More budget info"}),
        ];
        let store = ContextStore::from_chat_messages(&messages);

        let matches = store.grep_indexed("budget");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].0, 0);
        assert_eq!(matches[1].0, 2);
        assert!(store.grep_indexed("").is_empty());
    }

    #[test]
    fn describe_returns_counts_and_roles() {
        let messages = vec![
            json!({"role": "user", "content": "abc"}),
            json!({"role": "assistant", "content": null}),
        ];
        let store = ContextStore::from_chat_messages(&messages);
        let summary = store.describe();

        assert_eq!(summary.messages, 2);
        assert_eq!(summary.text_bytes, 3);
        assert_eq!(summary.roles, ["user", "assistant"]);
    }
}
