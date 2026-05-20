#![expect(
    clippy::result_large_err,
    reason = "figment Jail test closures return figment's native error type"
)]

use figment::Figment;
use figment::Jail;
use figment::providers::Serialized;
use figment::value::Dict;
use rlm_anywhere::{AppConfig, Settings, load_settings};

#[test]
fn loads_defaults_without_env_or_cli() {
    Jail::expect_with(|jail| {
        jail.clear_env();

        let settings = load_settings(Figment::new()).expect("default settings should load");

        assert_eq!(
            settings,
            Settings {
                port: 3000,
                upstream_base_url: "http://localhost:20128/v1".to_owned(),
                upstream_api_key: None,
            }
        );
        Ok(())
    });
}

#[test]
fn env_overrides_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "4242");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "http://example.test/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "env-key");

        let settings = load_settings(Figment::new()).expect("env settings should load");

        assert_eq!(settings.port, 4242);
        assert_eq!(settings.upstream_base_url, "http://example.test/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("env-key"));
        Ok(())
    });
}

#[test]
fn cli_overrides_env() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "4242");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "http://env.example/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "env-key");

        let settings = load_settings(
            Figment::new()
                .merge(Serialized::default("port", 5151))
                .merge(Serialized::default(
                    "upstream_base_url",
                    "http://cli.example/v1",
                ))
                .merge(Serialized::default("upstream_api_key", "cli-key")),
        )
        .expect("cli settings should load");

        assert_eq!(settings.port, 5151);
        assert_eq!(settings.upstream_base_url, "http://cli.example/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("cli-key"));
        Ok(())
    });
}

#[test]
fn absent_cli_fields_do_not_override_env_or_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "4242");

        let settings = load_settings(Figment::from(Serialized::defaults(Dict::new())))
            .expect("empty cli settings should load");

        assert_eq!(settings.port, 4242);
        assert_eq!(settings.upstream_base_url, "http://localhost:20128/v1");
        assert_eq!(settings.upstream_api_key, None);
        Ok(())
    });
}

#[test]
fn invalid_env_port_returns_config_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "not-a-port");

        let error = load_settings(Figment::new()).expect_err("port should fail");

        assert!(error.to_string().contains("failed to load"));
        assert!(format!("{error:?}").contains("port"));
        Ok(())
    });
}

#[test]
fn empty_api_key_becomes_none() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "  ");

        let settings = load_settings(Figment::new()).expect("empty api key should load");

        assert_eq!(settings.upstream_api_key, None);
        Ok(())
    });
}

#[test]
fn openai_env_overrides_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "http://openai.example/v1");
        jail.set_env("OPENAI_API_KEY", "openai-key");

        let settings = load_settings(Figment::new()).expect("openai env settings should load");

        assert_eq!(settings.upstream_base_url, "http://openai.example/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("openai-key"));
        Ok(())
    });
}

#[test]
fn empty_openai_api_key_becomes_none() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_API_KEY", "  ");

        let settings = load_settings(Figment::new()).expect("empty openai api key should load");

        assert_eq!(settings.upstream_api_key, None);
        Ok(())
    });
}

#[test]
fn cli_overrides_openai_env() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "http://openai.example/v1");
        jail.set_env("OPENAI_API_KEY", "openai-key");

        let settings = load_settings(
            Figment::new()
                .merge(Serialized::default(
                    "upstream_base_url",
                    "http://cli.example/v1",
                ))
                .merge(Serialized::default("upstream_api_key", "cli-key")),
        )
        .expect("cli settings should load");

        assert_eq!(settings.upstream_base_url, "http://cli.example/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("cli-key"));
        Ok(())
    });
}

#[test]
fn rlm_env_overrides_openai_env() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "http://openai.example/v1");
        jail.set_env("OPENAI_API_KEY", "openai-key");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "http://rlm.example/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "rlm-key");

        let settings = load_settings(Figment::new()).expect("rlm env settings should load");

        assert_eq!(settings.upstream_base_url, "http://rlm.example/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("rlm-key"));
        Ok(())
    });
}

#[test]
fn app_config_rejects_empty_upstream_url() {
    let bind_address = "127.0.0.1:0"
        .parse()
        .expect("test bind address should parse");

    let error =
        AppConfig::new(bind_address, "   ", None).expect_err("empty upstream URL should fail");

    let message = format!("{error:?}");
    assert!(message.contains("failed to normalize upstream base URL"));
    assert!(message.contains("upstream base URL cannot be empty"));
}

#[test]
fn app_config_rejects_invalid_upstream_url() {
    let bind_address = "127.0.0.1:0"
        .parse()
        .expect("test bind address should parse");

    let error = AppConfig::new(bind_address, "not a url", None)
        .expect_err("invalid upstream URL should fail");

    let message = format!("{error:?}");
    assert!(message.contains("failed to normalize upstream base URL"));
    assert!(message.contains("invalid upstream base URL: not a url"));
}
