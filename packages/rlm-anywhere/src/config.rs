use color_eyre::{Result, eyre::WrapErr as _};
use figment::providers::{Env, Serialized};
use figment::{Figment, Profile};
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_UPSTREAM_BASE_URL: &str = "http://localhost:20128/v1";

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Settings {
    pub port: u16,
    pub upstream_base_url: String,
    pub upstream_api_key: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct SettingsOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_api_key: Option<String>,
}

pub fn load_settings(overrides: SettingsOverrides) -> Result<Settings> {
    let mut settings: Settings = Figment::new()
        .merge(Serialized::defaults(default_settings()))
        .merge(Env::prefixed("RLM_ANYWHERE_"))
        .merge(Serialized::from(overrides, Profile::Default))
        .extract()
        .wrap_err("failed to load rlm-anywhere settings")?;

    settings.upstream_api_key = settings.upstream_api_key.and_then(non_empty_string);

    Ok(settings)
}

fn default_settings() -> Settings {
    Settings {
        port: DEFAULT_PORT,
        upstream_base_url: DEFAULT_UPSTREAM_BASE_URL.to_owned(),
        upstream_api_key: None,
    }
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
