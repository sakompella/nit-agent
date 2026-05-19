use std::sync::LazyLock;

use color_eyre::{Result, eyre::WrapErr as _};
use figment::providers::{Env, Serialized};
use figment::{Figment, Profile};
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = "http://localhost:20128/v1";
const ENV_PREFIX: &str = "RLM_ANYWHERE_";
static DEFAULT_SETTINGS: LazyLock<Settings> = LazyLock::new(|| Settings {
    port: DEFAULT_PORT,
    upstream_base_url: DEFAULT_UPSTREAM_BASE_URL.to_owned(),
    upstream_api_key: None,
});

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub upstream_base_url: String,
    pub upstream_api_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SettingsOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_api_key: Option<String>,
}

impl SettingsOverrides {
    #[must_use]
    pub fn none() -> Self {
        Self {
            port: None,
            upstream_base_url: None,
            upstream_api_key: None,
        }
    }
}

pub fn load_settings(overrides: SettingsOverrides) -> Result<Settings> {
    let mut settings: Settings = Figment::new()
        .merge(default_profile(&*DEFAULT_SETTINGS))
        .merge(Env::prefixed(ENV_PREFIX))
        .merge(default_profile(overrides))
        .extract()
        .wrap_err("failed to load rlm-anywhere settings")?;

    settings.upstream_api_key = settings.upstream_api_key.and_then(non_empty_string);

    Ok(settings)
}

fn default_profile<T>(value: T) -> Serialized<T>
where
    T: Serialize,
{
    Serialized::from(value, Profile::Default)
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
