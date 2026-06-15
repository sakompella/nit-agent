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
    ]);
    // Keep this explicit so empty primary env vars are ignored instead of
    // clobbering OpenAI aliases or defaults.
    let rlm_env = env_settings([
        (RLM_MODE_ENV, "mode"),
        (RLM_UPSTREAM_PROVIDER_ENV, "upstream_provider"),
        (RLM_UPSTREAM_BASE_URL_ENV, "upstream_base_url"),
        (RLM_UPSTREAM_API_KEY_ENV, "upstream_api_key"),
    ]);
    let rlm_env = merge_parsed_env::<u16>(rlm_env, RLM_PORT_ENV, "port")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_STEPS_ENV, "rlm_max_steps")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_SUBCALLS_ENV, "rlm_max_subcalls")?;
    let rlm_env = merge_parsed_env::<u64>(rlm_env, RLM_MAX_WALL_MS_ENV, "rlm_max_wall_ms")?;
    let rlm_env = merge_parsed_env::<usize>(
        rlm_env,
        RLM_TOOL_RESULT_PREVIEW_BYTES_ENV,
        "rlm_tool_result_preview_bytes",
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
        }))
        .merge(openai_aliases)
        .merge(rlm_env)
        .merge(overrides)
        .extract::<Settings>()
        .map(Settings::normalize)
        .wrap_err("failed to load rlm-anywhere settings")?;

    Ok(settings)
}

fn warn_on_conflicting_env_alias(rlm_name: &str, openai_name: &'static str, setting: &str) {
    let (Some(rlm_value), Some(openai_value)) = (trimmed_env(rlm_name), trimmed_env(openai_name))
    else {
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

fn env_settings<const N: usize>(pairs: [(&str, &str); N]) -> Figment {
    pairs
        .into_iter()
        .fold(Figment::new(), |figment, (env_var, key)| {
            trimmed_env(env_var)
                .map(|value| Serialized::default(key, value))
                .into_iter()
                .fold(figment, |figment, provider| figment.merge(provider))
        })
}

fn merge_parsed_env<T>(mut figment: Figment, env_name: &str, key: &str) -> Result<Figment>
where
    T: Serialize + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    if let Some(value) = trimmed_env(env_name) {
        let parsed = value
            .parse::<T>()
            .map_err(|e| color_eyre::eyre::eyre!("invalid value in {env_name}: {e}"))
            .wrap_err("failed to load rlm-anywhere settings")?;
        figment = figment.merge(Serialized::default(key, parsed));
    }
    Ok(figment)
}

fn trimmed_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|v| trim_str_or_empty(&v))
}

/// Takes a string and trims it & if empty returns None
fn trim_str_or_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed.to_owned())
}
