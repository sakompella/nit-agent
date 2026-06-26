use serde_json::json;

use rlm_anywhere::rlm::{ContextMessage, ContextStore};

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
    assert_eq!(store.get(0).and_then(ContextMessage::role), Some("system"));
    assert_eq!(
        store.get(1).map(ContextMessage::text),
        Some("Find the answer\nextra text")
    );
    assert_eq!(store.get(1).map(ContextMessage::raw), messages.get(1));
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
