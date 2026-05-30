use std::env;
use std::sync::LazyLock;

use clap::ValueEnum;
use color_eyre::{Result, eyre::WrapErr as _};
use figment::Figment;
use figment::providers::Serialized;
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = const_str::concat!("http://localhost:20128", "/v1");
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const RLM_PORT_ENV: &str = "RLM_ANYWHERE_PORT";
const RLM_MODE_ENV: &str = "RLM_ANYWHERE_MODE";
const RLM_UPSTREAM_PROVIDER_ENV: &str = "RLM_ANYWHERE_UPSTREAM_PROVIDER";
const RLM_UPSTREAM_BASE_URL_ENV: &str = "RLM_ANYWHERE_UPSTREAM_BASE_URL";
const RLM_UPSTREAM_API_KEY_ENV: &str = "RLM_ANYWHERE_UPSTREAM_API_KEY";
static DEFAULT_SETTINGS: LazyLock<Settings> = LazyLock::new(|| Settings {
    port: DEFAULT_PORT,
    mode: PassthroughStatus::Rlm,
    upstream_provider: UpstreamProvider::OpenAiCompatible,
    upstream_base_url: DEFAULT_UPSTREAM_BASE_URL.to_owned(),
    upstream_api_key: None,
});

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamProvider {
    OpenAiCompatible,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum PassthroughStatus {
    Rlm,
    Passthrough,
}

/// Settings created from config providers before building `AppConfig`.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub mode: PassthroughStatus,
    pub upstream_provider: UpstreamProvider,
    pub upstream_base_url: String,
    pub upstream_api_key: Option<String>,
}

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
    let rlm_env = if let Some(port) = non_empty_env(RLM_PORT_ENV) {
        let port = port
            .parse::<u16>()
            .wrap_err_with(|| format!("invalid port in {RLM_PORT_ENV}"))
            .wrap_err("failed to load rlm-anywhere settings")?;
        rlm_env.merge(Serialized::default("port", port))
    } else {
        rlm_env
    };
    let settings = Figment::new()
        .merge(Serialized::defaults(LazyLock::force(&DEFAULT_SETTINGS)))
        .merge(openai_aliases)
        .merge(rlm_env)
        .merge(overrides)
        .extract::<Settings>()
        .map(|settings| Settings {
            upstream_api_key: settings.upstream_api_key.and_then(|key| {
                let trimmed = key.trim();
                (!trimmed.is_empty()).then_some(trimmed.to_owned())
            }),
            ..settings
        })
        .wrap_err("failed to load rlm-anywhere settings")?;

    Ok(settings)
}

fn warn_on_conflicting_env_alias(rlm_name: &str, openai_name: &'static str, setting: &str) {
    let (Some(rlm_value), Some(openai_value)) =
        (non_empty_env(rlm_name), non_empty_env(openai_name))
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
            non_empty_env(env_var)
                .map(|value| Serialized::default(key, value))
                .into_iter()
                .fold(figment, |figment, provider| figment.merge(provider))
        })
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}
