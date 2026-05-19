use std::sync::LazyLock;

use color_eyre::{Result, eyre::WrapErr as _};
use figment::Figment;
use figment::providers::{Env, Serialized};
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = "http://localhost:20128/v1";
const ENV_PREFIX: &str = "RLM_ANYWHERE_";
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
    let settings = Figment::new()
        .merge(Serialized::defaults(LazyLock::force(&DEFAULT_SETTINGS)))
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
