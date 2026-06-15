use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rquickjs::{Context, Error as QuickJsError, Runtime, Value, context::intrinsic};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxLimits {
    pub timeout: Duration,
    pub memory_limit_bytes: usize,
    pub max_stack_bytes: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(250),
            memory_limit_bytes: 16 * 1024 * 1024,
            max_stack_bytes: 512 * 1024,
        }
    }
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("failed to initialize QuickJS sandbox: {0}")]
    Init(#[source] QuickJsError),
    #[error("QuickJS evaluation exceeded {timeout:?}")]
    Timeout { timeout: Duration },
    #[error("QuickJS evaluation failed: {0}")]
    Eval(String),
}

pub struct Sandbox {
    limits: SandboxLimits,
}

impl Sandbox {
    #[must_use]
    pub const fn new(limits: SandboxLimits) -> Self {
        Self { limits }
    }

    /// # Errors
    /// Returns an error if the QuickJS runtime fails to initialize, evaluation times out, or evaluation fails.
    pub fn eval_json(&self, source: &str) -> Result<String, SandboxError> {
        let runtime = Runtime::new().map_err(SandboxError::Init)?;
        runtime.set_memory_limit(self.limits.memory_limit_bytes);
        runtime.set_max_stack_size(self.limits.max_stack_bytes);

        let timed_out = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + self.limits.timeout;
        let interrupt_timed_out = Arc::clone(&timed_out);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let should_interrupt = Instant::now() >= deadline;
            if should_interrupt {
                interrupt_timed_out.store(true, Ordering::Relaxed);
            }
            should_interrupt
        })));

        let context = Context::builder()
            .with::<intrinsic::Eval>()
            .with::<intrinsic::Json>()
            .build(&runtime)
            .map_err(SandboxError::Init)?;

        let result = context.with(|ctx| {
            let wrapped = format!("JSON.stringify((() => {{\n{source}\n}})())");
            ctx.eval::<String, _>(wrapped)
                .map_err(|error| eval_error_message(&ctx, &error))
        });

        if timed_out.load(Ordering::Relaxed) {
            return Err(SandboxError::Timeout {
                timeout: self.limits.timeout,
            });
        }

        result.map_err(SandboxError::Eval)
    }
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new(SandboxLimits::default())
    }
}

fn eval_error_message(ctx: &rquickjs::Ctx<'_>, error: &QuickJsError) -> String {
    if matches!(error, QuickJsError::Exception) {
        let exception = ctx.catch();
        return stringify_exception(&exception);
    }

    error.to_string()
}

fn stringify_exception(exception: &Value<'_>) -> String {
    exception
        .as_exception()
        .map_or_else(|| format!("{exception:?}"), ToString::to_string)
}
