use async_openai::Client;
use async_openai::config::Config;
use color_eyre::Result;
use color_eyre::eyre::WrapErr as _;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use secrecy::{ExposeSecret as _, SecretString};

pub(crate) type UpstreamClient = Client<UpstreamConfig>;

#[derive(Clone, Debug)]
pub(crate) struct UpstreamConfig {
    api_base: String,
    api_key: SecretString,
    authorization: Option<HeaderValue>,
}

impl UpstreamConfig {
    pub(crate) fn new(api_base: String, api_key: Option<SecretString>) -> Result<Self> {
        let authorization = api_key
            .as_ref()
            .map(bearer_header_value)
            .transpose()
            .wrap_err("failed to build upstream authorization header")?;
        Ok(Self {
            api_base,
            api_key: api_key.unwrap_or_else(|| SecretString::from(String::new())),
            authorization,
        })
    }

    pub(crate) fn into_client(self) -> UpstreamClient {
        Client::with_config(self)
    }
}

impl Config for UpstreamConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(authorization) = &self.authorization {
            headers.insert(AUTHORIZATION, authorization.clone());
        }
        headers
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }

    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn api_key(&self) -> &SecretString {
        &self.api_key
    }
}

fn bearer_header_value(api_key: &SecretString) -> Result<HeaderValue> {
    HeaderValue::from_str(&format!("Bearer {}", api_key.expose_secret()))
        .wrap_err("upstream API key cannot be represented as an HTTP header")
}
