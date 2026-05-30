use std::time::Duration;

use rlm_anywhere::rlm::sandbox::{Sandbox, SandboxError, SandboxLimits};

#[test]
fn serializes_returned_json_value() {
    let result = Sandbox::default().eval_json("return { answer: 'ok', count: 2 };");

    assert_eq!(
        r#"{"answer":"ok","count":2}"#,
        result.as_deref().unwrap_or("")
    );
}

#[test]
fn does_not_expose_common_host_capabilities() {
    let result = Sandbox::default().eval_json(
        r"
        return {
            fetch: typeof fetch,
            process: typeof process,
            require: typeof require,
            readFile: typeof readFile
        };
        ",
    );

    assert_eq!(
        r#"{"fetch":"undefined","process":"undefined","require":"undefined","readFile":"undefined"}"#,
        result.as_deref().unwrap_or("")
    );
}

#[test]
fn interrupts_infinite_loop() {
    let sandbox = Sandbox::new(SandboxLimits {
        timeout: Duration::from_millis(10),
        memory_limit_bytes: 16 * 1024 * 1024,
        max_stack_bytes: 512 * 1024,
    });

    let result = sandbox.eval_json("for (;;) {}");

    assert!(matches!(result, Err(SandboxError::Timeout { .. })));
}

#[test]
fn reports_javascript_exception_message() {
    let result = Sandbox::default().eval_json("throw new Error('boom');");

    assert!(matches!(result, Err(SandboxError::Eval(message)) if message.contains("boom")));
}
