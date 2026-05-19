#![expect(
    clippy::result_large_err,
    reason = "figment Jail test closures return figment's native error type"
)]

use figment::Jail;
use rlm_anywhere::{Settings, SettingsOverrides, load_settings};

#[test]
fn loads_defaults_without_env_or_cli() {
    Jail::expect_with(|jail| {
        jail.clear_env();

        let settings =
            load_settings(SettingsOverrides::none()).expect("default settings should load");

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

        let settings = load_settings(SettingsOverrides::none()).expect("env settings should load");

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

        let settings = load_settings(SettingsOverrides {
            port: Some(5151),
            upstream_base_url: Some("http://cli.example/v1".to_owned()),
            upstream_api_key: Some("cli-key".to_owned()),
        })
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

        let settings = load_settings(SettingsOverrides {
            port: None,
            upstream_base_url: None,
            upstream_api_key: None,
        })
        .expect("partial cli settings should load");

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

        let error = load_settings(SettingsOverrides::none()).expect_err("port should fail");

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

        let settings = load_settings(SettingsOverrides::none()).expect("empty api key should load");

        assert_eq!(settings.upstream_api_key, None);
        Ok(())
    });
}
