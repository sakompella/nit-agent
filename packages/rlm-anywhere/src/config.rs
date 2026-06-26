use std::env;

use clap::ValueEnum;
use color_eyre::{Result, eyre::WrapErr as _};
use figment::Figment;
use figment::providers::Serialized;
use serde::{Deserialize, Serialize};

use crate::rlm::RlmLoopConfig;

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = const_str::concat!("http://localhost:20128", "/v1");
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const RLM_PORT_ENV: &str = "RLM_ANYWHERE_PORT";
const RLM_MODE_ENV: &str = "RLM_ANYWHERE_MODE";
const RLM_UPSTREAM_PROVIDER_ENV: &str = "RLM_ANYWHERE_UPSTREAM_PROVIDER";
const RLM_UPSTREAM_BASE_URL_ENV: &str = "RLM_ANYWHERE_UPSTREAM_BASE_URL";
const RLM_UPSTREAM_API_KEY_ENV: &str = "RLM_ANYWHERE_UPSTREAM_API_KEY";
const RLM_MAX_STEPS_ENV: &str = "RLM_ANYWHERE_MAX_STEPS";
const RLM_MAX_SUBCALLS_ENV: &str = "RLM_ANYWHERE_MAX_SUBCALLS";
const RLM_MAX_WALL_MS_ENV: &str = "RLM_ANYWHERE_MAX_WALL_MS";
const RLM_TOOL_RESULT_PREVIEW_BYTES_ENV: &str = "RLM_ANYWHERE_TOOL_RESULT_PREVIEW_BYTES";
const RLM_MAX_TOOL_ARG_BYTES_ENV: &str = "RLM_ANYWHERE_MAX_TOOL_ARG_BYTES";
const RLM_UPSTREAM_TIMEOUT_MS_ENV: &str = "RLM_ANYWHERE_UPSTREAM_TIMEOUT_MS";
const RLM_MAX_BODY_BYTES_ENV: &str = "RLM_ANYWHERE_MAX_BODY_BYTES";
const RLM_MAX_CONCURRENT_REQUESTS_ENV: &str = "RLM_ANYWHERE_MAX_CONCURRENT_REQUESTS";
const RLM_UPSTREAM_MAX_RETRIES_ENV: &str = "RLM_ANYWHERE_UPSTREAM_MAX_RETRIES";

const DEFAULT_UPSTREAM_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4_194_304;
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 1024;
const DEFAULT_UPSTREAM_MAX_RETRIES: usize = 2;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamProvider {
    OpenAiCompatible,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum RequestMode {
    Rlm,
    Passthrough,
}

/// Settings created from config providers before building `AppConfig`.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub mode: RequestMode,
    pub upstream_provider: UpstreamProvider,
    pub upstream_base_url: String,
    pub upstream_api_key: Option<String>,
    pub rlm_max_steps: u64,
    pub rlm_max_subcalls: u64,
    pub rlm_max_wall_ms: u64,
    pub rlm_tool_result_preview_bytes: usize,
    pub rlm_max_tool_arg_bytes: usize,
    pub upstream_timeout_ms: u64,
    pub max_request_body_bytes: usize,
    pub max_concurrent_requests: usize,
    pub upstream_max_retries: usize,
}

impl Settings {
    fn normalize(self) -> Self {
        Self {
            upstream_api_key: self.upstream_api_key.and_then(|v| trim_str_or_empty(&v)),
            ..self
        }
    }
}

/// # Errors
/// Returns an error if settings cannot be deserialized or an env var contains an invalid value.
pub fn load_settings(overrides: Figment) -> Result<Settings> {
    warn_on_conflicting_env_alias(
        RLM_UPSTREAM_BASE_URL_ENV,
        OPENAI_BASE_URL_ENV,
        "upstream base URL",
    );
    warn_on_conflicting_env_alias(
        RLM_UPSTREAM_API_KEY_ENV,
        OPENAI_API_KEY_ENV,
        "upstream API key",
    );

    let openai_aliases = env_settings([
        (OPENAI_BASE_URL_ENV, "upstream_base_url"),
        (OPENAI_API_KEY_ENV, "upstream_api_key"),
    ])?;
    // Keep this explicit so empty primary env vars are ignored instead of
    // clobbering OpenAI aliases or defaults.
    let rlm_env = env_settings([
        (RLM_MODE_ENV, "mode"),
        (RLM_UPSTREAM_PROVIDER_ENV, "upstream_provider"),
        (RLM_UPSTREAM_BASE_URL_ENV, "upstream_base_url"),
        (RLM_UPSTREAM_API_KEY_ENV, "upstream_api_key"),
    ])?;
    let rlm_env = merge_parsed_env::<u16>(rlm_env, RLM_PORT_ENV, "port")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_STEPS_ENV, "rlm_max_steps")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_SUBCALLS_ENV, "rlm_max_subcalls")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_WALL_MS_ENV, "rlm_max_wall_ms")?;
    let rlm_env = merge_parsed_env::<usize>(
        rlm_env,
        RLM_TOOL_RESULT_PREVIEW_BYTES_ENV,
        "rlm_tool_result_preview_bytes",
    )?;
    let rlm_env = merge_parsed_env::<usize>(
        rlm_env,
        RLM_MAX_TOOL_ARG_BYTES_ENV,
        "rlm_max_tool_arg_bytes",
    )?;
    let rlm_env =
        merge_parsed_env::<u64>(rlm_env, RLM_UPSTREAM_TIMEOUT_MS_ENV, "upstream_timeout_ms")?;
    let rlm_env =
        merge_parsed_env::<usize>(rlm_env, RLM_MAX_BODY_BYTES_ENV, "max_request_body_bytes")?;
    let rlm_env = merge_parsed_env::<usize>(
        rlm_env,
        RLM_MAX_CONCURRENT_REQUESTS_ENV,
        "max_concurrent_requests",
    )?;
    let rlm_env = merge_parsed_env::<usize>(
        rlm_env,
        RLM_UPSTREAM_MAX_RETRIES_ENV,
        "upstream_max_retries",
    )?;
    // Keep application defaults explicit without making `Settings::default()` a
    // general construction pattern for callers or future config paths.
    let settings = Figment::new()
        .merge(Serialized::defaults(Settings {
            port: DEFAULT_PORT,
            mode: RequestMode::Rlm,
            upstream_provider: UpstreamProvider::OpenAiCompatible,
            upstream_base_url: DEFAULT_UPSTREAM_BASE_URL.to_owned(),
            upstream_api_key: None,
            rlm_max_steps: RlmLoopConfig::DEFAULT_MAX_STEPS,
            rlm_max_subcalls: RlmLoopConfig::DEFAULT_MAX_SUBCALLS,
            rlm_max_wall_ms: RlmLoopConfig::DEFAULT_MAX_WALL_MS,
            rlm_tool_result_preview_bytes: RlmLoopConfig::DEFAULT_TOOL_RESULT_PREVIEW_BYTES,
            rlm_max_tool_arg_bytes: RlmLoopConfig::DEFAULT_MAX_TOOL_ARG_BYTES,
            upstream_timeout_ms: DEFAULT_UPSTREAM_TIMEOUT_MS,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            upstream_max_retries: DEFAULT_UPSTREAM_MAX_RETRIES,
        }))
        .merge(openai_aliases)
        .merge(rlm_env)
        .merge(overrides)
        .extract::<Settings>()
        .map(Settings::normalize)
        .wrap_err("failed to load rlm-anywhere settings")?;

    reject_degenerate_budgets(&settings)?;

    Ok(settings)
}

/// The RLM loop must take at least one step, allow at least one subcall, and run
/// for a nonzero wall-clock budget; a zero here would make every request fail
/// immediately, so it is rejected at load time rather than per request.
fn reject_degenerate_budgets(settings: &Settings) -> Result<()> {
    for (value, name) in [
        (settings.rlm_max_steps, "rlm_max_steps"),
        (settings.rlm_max_subcalls, "rlm_max_subcalls"),
        (settings.rlm_max_wall_ms, "rlm_max_wall_ms"),
    ] {
        if value == 0 {
            return Err(color_eyre::eyre::eyre!("{name} must be greater than 0"))
                .wrap_err("failed to load rlm-anywhere settings");
        }
    }
    Ok(())
}

fn warn_on_conflicting_env_alias(rlm_name: &str, openai_name: &'static str, setting: &str) {
    let Ok(Some(rlm_value)) = trimmed_env(rlm_name) else {
        return;
    };
    let Ok(Some(openai_value)) = trimmed_env(openai_name) else {
        return;
    };

    if rlm_value != openai_value {
        tracing::warn!(
            setting,
            rlm_env = rlm_name,
            openai_env = openai_name,
            "{} and {} both set different values; using {}",
            rlm_name,
            openai_name,
            rlm_name
        );
    }
}

fn env_settings<const N: usize>(pairs: [(&str, &str); N]) -> Result<Figment> {
    pairs
        .into_iter()
        .try_fold(Figment::new(), |figment, (env_var, key)| {
            Ok(match trimmed_env(env_var)? {
                Some(value) => figment.merge(Serialized::default(key, value)),
                None => figment,
            })
        })
}

fn merge_parsed_env<T>(mut figment: Figment, env_name: &str, key: &str) -> Result<Figment>
where
    T: Serialize + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    if let Some(value) = trimmed_env(env_name)? {
        let parsed = value
            .parse::<T>()
            .map_err(|e| color_eyre::eyre::eyre!("invalid value in {env_name}: {e}"))
            .wrap_err("failed to load rlm-anywhere settings")?;
        figment = figment.merge(Serialized::default(key, parsed));
    }
    Ok(figment)
}

fn trimmed_env(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(v) => Ok(trim_str_or_empty(&v)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(raw)) => Err(color_eyre::eyre::eyre!(
            "{name} contains a non-Unicode value: {raw:?}"
        )),
    }
}

/// Takes a string and trims it & if empty returns None
fn trim_str_or_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed.to_owned())
}
