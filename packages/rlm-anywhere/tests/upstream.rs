use std::time::Duration;

use secrecy::SecretString;
use serde_json::json;

use rlm_anywhere::{ModelError, ModelRequest, RigModelBackend};

#[tokio::test]
async fn caller_authorization_header_error_is_sanitized() {
    const SENTINEL_SECRET: &str = "rlm-anywhere-test-secret";

    let backend = RigModelBackend::new(
        "http://127.0.0.1:9/v1".to_owned(),
        None,
        Duration::from_secs(30),
    )
    .expect("backend should build");

    let error = backend
        .complete(ModelRequest {
            body: json!({
                "model": "local-model",
                "messages": [{ "role": "user", "content": "hello" }]
            }),
            caller_authorization: Some(SecretString::from(format!("Bearer {SENTINEL_SECRET}\n"))),
        })
        .await
        .expect_err("invalid caller authorization should fail before upstream I/O");

    let ModelError::Request(message) = error else {
        panic!("invalid caller authorization should produce a request error");
    };
    assert!(
        message.contains("failed to build request-scoped Rig client"),
        "message should identify request-scoped client construction: {message}"
    );
    assert!(
        !message.contains(SENTINEL_SECRET),
        "message should redact caller authorization: {message}"
    );
    assert!(
        !message.contains("Backtrace"),
        "message should not expose a color-eyre backtrace: {message}"
    );
    assert!(
        !message.contains("SpanTrace"),
        "message should not expose a color-eyre span trace: {message}"
    );
}
