use std::env;
use std::sync::LazyLock;

use color_eyre::{Result, eyre::WrapErr as _};
use figment::Figment;
use figment::providers::{Env, Serialized};
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = const_str::concat!("http://localhost:20128", "/v1");
const ENV_PREFIX: &str = "RLM_ANYWHERE_";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const RLM_UPSTREAM_BASE_URL_ENV: &str = "RLM_ANYWHERE_UPSTREAM_BASE_URL";
const RLM_UPSTREAM_API_KEY_ENV: &str = "RLM_ANYWHERE_UPSTREAM_API_KEY";
static DEFAULT_SETTINGS: LazyLock<Settings> = LazyLock::new(|| Settings {
    port: DEFAULT_PORT,
    upstream_base_url: DEFAULT_UPSTREAM_BASE_URL.to_owned(),
    upstream_api_key: None,
});

/// Settings created from config providers before building `AppConfig`.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
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

    let mut openai_aliases = Figment::new();
    if let Ok(base_url) = env::var(OPENAI_BASE_URL_ENV) {
        openai_aliases = openai_aliases.merge(Serialized::default("upstream_base_url", base_url));
    }
    if let Ok(api_key) = env::var(OPENAI_API_KEY_ENV) {
        openai_aliases = openai_aliases.merge(Serialized::default("upstream_api_key", api_key));
    }

    let settings = Figment::new()
        .merge(Serialized::defaults(LazyLock::force(&DEFAULT_SETTINGS)))
        .merge(openai_aliases)
        .merge(Env::prefixed(ENV_PREFIX))
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

fn warn_on_conflicting_env_alias(rlm_name: &'static str, openai_name: &'static str, setting: &str) {
    let Some(rlm_value) = non_empty_env(rlm_name) else {
        return;
    };
    let Some(openai_value) = non_empty_env(openai_name) else {
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

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}
