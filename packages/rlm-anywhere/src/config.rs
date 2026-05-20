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
const ENV_ALIASES: &[EnvAlias] = &[
    EnvAlias {
        rlm_name: RLM_UPSTREAM_BASE_URL_ENV,
        openai_name: OPENAI_BASE_URL_ENV,
        field: "upstream_base_url",
        setting: "upstream base URL",
    },
    EnvAlias {
        rlm_name: RLM_UPSTREAM_API_KEY_ENV,
        openai_name: OPENAI_API_KEY_ENV,
        field: "upstream_api_key",
        setting: "upstream API key",
    },
];
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

struct EnvAlias {
    rlm_name: &'static str,
    openai_name: &'static str,
    field: &'static str,
    setting: &'static str,
}

pub fn load_settings(overrides: Figment) -> Result<Settings> {
    let mut openai_aliases = Figment::new();
    for alias in ENV_ALIASES {
        warn_on_conflicting_env_alias(alias);
        if let Ok(value) = env::var(alias.openai_name) {
            openai_aliases = openai_aliases.merge(Serialized::default(alias.field, value));
        }
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

fn warn_on_conflicting_env_alias(alias: &EnvAlias) {
    let Some(rlm_value) = non_empty_env(alias.rlm_name) else {
        return;
    };
    let Some(openai_value) = non_empty_env(alias.openai_name) else {
        return;
    };

    if rlm_value != openai_value {
        tracing::warn!(
            setting = alias.setting,
            rlm_env = alias.rlm_name,
            openai_env = alias.openai_name,
            "{} and {} both set different values; using {}",
            alias.rlm_name,
            alias.openai_name,
            alias.rlm_name
        );
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}
