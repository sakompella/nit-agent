#![expect(
    clippy::result_large_err,
    reason = "figment Jail test closures return figment's native error type"
)]

use figment::Figment;
use figment::Jail;
use figment::providers::Serialized;
use figment::value::Dict;
use rlm_anywhere::{AppConfig, RequestMode, Settings, UpstreamProvider, load_settings};

#[test]
fn loads_defaults_without_env_or_cli() {
    Jail::expect_with(|jail| {
        jail.clear_env();

        let settings = load_settings(Figment::new()).expect("default settings should load");

        assert_eq!(
            settings,
            Settings {
                port: 3000,
                mode: RequestMode::Rlm,
                upstream_provider: UpstreamProvider::OpenAiCompatible,
                upstream_base_url: "http://localhost:20128/v1".to_owned(),
                upstream_api_key: None,
                rlm_max_steps: 20,
                rlm_max_subcalls: 64,
                rlm_max_wall_ms: 120_000,
                rlm_tool_result_preview_bytes: 8_192,
                upstream_timeout_ms: 60_000,
            }
        );
        Ok(())
    });
}

#[test]
fn unknown_upstream_provider_returns_config_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_PROVIDER", "not-a-provider");

        let error = load_settings(Figment::new()).expect_err("provider should fail");

        assert!(error.to_string().contains("failed to load"));
        assert!(format!("{error:?}").contains("not-a-provider"));
        Ok(())
    });
}

#[test]
fn env_overrides_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "4242");
        jail.set_env("RLM_ANYWHERE_MODE", "passthrough");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "http://example.test/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "env-key");
        jail.set_env("RLM_ANYWHERE_MAX_STEPS", "3");
        jail.set_env("RLM_ANYWHERE_MAX_SUBCALLS", "5");
        jail.set_env("RLM_ANYWHERE_MAX_WALL_MS", "99");
        jail.set_env("RLM_ANYWHERE_TOOL_RESULT_PREVIEW_BYTES", "256");

        let settings = load_settings(Figment::new()).expect("env settings should load");

        assert_eq!(settings.port, 4242);
        assert_eq!(settings.mode, RequestMode::Passthrough);
        assert_eq!(settings.upstream_base_url, "http://example.test/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("env-key"));
        assert_eq!(settings.rlm_max_steps, 3);
        assert_eq!(settings.rlm_max_subcalls, 5);
        assert_eq!(settings.rlm_max_wall_ms, 99);
        assert_eq!(settings.rlm_tool_result_preview_bytes, 256);
        Ok(())
    });
}

#[test]
fn cli_overrides_env() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "4242");
        jail.set_env("RLM_ANYWHERE_MODE", "rlm");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "http://env.example/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "env-key");
        jail.set_env("RLM_ANYWHERE_MAX_STEPS", "7");

        let settings = load_settings(
            Figment::new()
                .merge(Serialized::default("port", 5151))
                .merge(Serialized::default("mode", RequestMode::Passthrough))
                .merge(Serialized::default(
                    "upstream_base_url",
                    "http://cli.example/v1",
                ))
                .merge(Serialized::default("upstream_api_key", "cli-key"))
                .merge(Serialized::default("rlm_max_steps", 11u64)),
        )
        .expect("cli settings should load");

        assert_eq!(settings.port, 5151);
        assert_eq!(settings.mode, RequestMode::Passthrough);
        assert_eq!(settings.upstream_base_url, "http://cli.example/v1");
        assert_eq!(settings.upstream_api_key.as_deref(), Some("cli-key"));
        assert_eq!(settings.rlm_max_steps, 11);
        Ok(())
    });
}

#[test]
fn whitespace_cli_api_key_becomes_none() {
    Jail::expect_with(|jail| {
        jail.clear_env();

        let settings =
            load_settings(Figment::new().merge(Serialized::default("upstream_api_key", "  \t\n")))
                .expect("whitespace cli api key should load");

        assert_eq!(settings.upstream_api_key, None);
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
        assert!(format!("{error:?}").to_lowercase().contains("port"));
        Ok(())
    });
}

#[test]
fn invalid_env_mode_returns_config_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_MODE", "not-a-mode");

        let error = load_settings(Figment::new()).expect_err("mode should fail");

        assert!(error.to_string().contains("failed to load"));
        assert!(format!("{error:?}").contains("not-a-mode"));
        Ok(())
    });
}

#[test]
fn empty_env_mode_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_MODE", "  ");

        let settings = load_settings(Figment::new()).expect("empty mode should load");

        assert_eq!(settings.mode, RequestMode::Rlm);
        Ok(())
    });
}

#[test]
fn empty_env_port_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_PORT", "  ");

        let settings = load_settings(Figment::new()).expect("empty port should load");

        assert_eq!(settings.port, 3000);
        Ok(())
    });
}

#[test]
fn empty_rlm_numeric_envs_are_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_MAX_STEPS", "");
        jail.set_env("RLM_ANYWHERE_MAX_SUBCALLS", "   ");
        jail.set_env("RLM_ANYWHERE_MAX_WALL_MS", "");
        jail.set_env("RLM_ANYWHERE_TOOL_RESULT_PREVIEW_BYTES", " ");

        let settings = load_settings(Figment::new()).expect("empty numeric envs should load");

        assert_eq!(settings.rlm_max_steps, 20);
        assert_eq!(settings.rlm_max_subcalls, 64);
        assert_eq!(settings.rlm_max_wall_ms, 120_000);
        assert_eq!(settings.rlm_tool_result_preview_bytes, 8_192);
        Ok(())
    });
}

#[test]
fn upstream_timeout_env_overrides_default() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_TIMEOUT_MS", "1500");

        let settings = load_settings(Figment::new()).expect("timeout env should load");

        assert_eq!(settings.upstream_timeout_ms, 1500);
        Ok(())
    });
}

#[test]
fn empty_upstream_timeout_env_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_TIMEOUT_MS", "  ");

        let settings = load_settings(Figment::new()).expect("empty timeout env should load");

        assert_eq!(settings.upstream_timeout_ms, 60_000);
        Ok(())
    });
}

#[test]
fn invalid_upstream_timeout_env_returns_config_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_TIMEOUT_MS", "not-a-number");

        let error = load_settings(Figment::new()).expect_err("timeout env should fail");

        assert!(error.to_string().contains("failed to load"));
        assert!(format!("{error:?}").contains("RLM_ANYWHERE_UPSTREAM_TIMEOUT_MS"));
        Ok(())
    });
}

#[test]
fn invalid_rlm_numeric_env_returns_config_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_MAX_STEPS", "abc");

        let error = load_settings(Figment::new()).expect_err("numeric env should fail");

        assert!(error.to_string().contains("failed to load"));
        assert!(format!("{error:?}").contains("RLM_ANYWHERE_MAX_STEPS"));
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
fn empty_rlm_api_key_does_not_override_openai_api_key() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_API_KEY", "openai-key");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "");

        let settings = load_settings(Figment::new()).expect("empty rlm api key should load");

        assert_eq!(settings.upstream_api_key.as_deref(), Some("openai-key"));
        Ok(())
    });
}

#[test]
fn whitespace_rlm_api_key_does_not_override_openai_api_key() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_API_KEY", "openai-key");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_API_KEY", "  ");

        let settings = load_settings(Figment::new()).expect("whitespace rlm api key should load");

        assert_eq!(settings.upstream_api_key.as_deref(), Some("openai-key"));
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
fn empty_openai_base_url_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "");

        let settings = load_settings(Figment::new()).expect("empty openai base URL should load");

        assert_eq!(settings.upstream_base_url, "http://localhost:20128/v1");
        Ok(())
    });
}

#[test]
fn whitespace_openai_base_url_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "   ");

        let settings =
            load_settings(Figment::new()).expect("whitespace openai base URL should load");

        assert_eq!(settings.upstream_base_url, "http://localhost:20128/v1");
        Ok(())
    });
}

#[test]
fn empty_rlm_base_url_is_ignored() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "");

        let settings = load_settings(Figment::new()).expect("empty rlm base URL should load");

        assert_eq!(settings.upstream_base_url, "http://localhost:20128/v1");
        Ok(())
    });
}

#[test]
fn empty_rlm_base_url_does_not_override_openai_base_url() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_BASE_URL", "http://openai.example/v1");
        jail.set_env("RLM_ANYWHERE_UPSTREAM_BASE_URL", "  ");

        let settings = load_settings(Figment::new()).expect("empty rlm base URL should load");

        assert_eq!(settings.upstream_base_url, "http://openai.example/v1");
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
fn rlm_env_provider_overrides_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RLM_ANYWHERE_UPSTREAM_PROVIDER", "open-ai-compatible");

        let settings = load_settings(Figment::new()).expect("rlm provider should load");

        assert_eq!(
            settings.upstream_provider,
            UpstreamProvider::OpenAiCompatible
        );
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

#[test]
fn app_config_rejects_non_http_upstream_url() {
    let bind_address = "127.0.0.1:0"
        .parse()
        .expect("test bind address should parse");

    let error = AppConfig::new(bind_address, "file:///tmp/upstream", None)
        .expect_err("non-HTTP upstream URL should fail");

    let message = format!("{error:?}");
    assert!(message.contains("failed to normalize upstream base URL"));
    assert!(message.contains("upstream base URL must use http or https"));
}

#[test]
fn app_config_rejects_upstream_url_with_query_or_fragment() {
    let bind_address = "127.0.0.1:0"
        .parse()
        .expect("test bind address should parse");

    let query_error = AppConfig::new(bind_address, "https://example.test/v1?api=chat", None)
        .expect_err("upstream URL with query should fail");
    let fragment_error = AppConfig::new(bind_address, "https://example.test/v1#chat", None)
        .expect_err("upstream URL with fragment should fail");

    let query_message = format!("{query_error:?}");
    let fragment_message = format!("{fragment_error:?}");
    assert!(query_message.contains("failed to normalize upstream base URL"));
    assert!(query_message.contains("upstream base URL cannot include query or fragment"));
    assert!(fragment_message.contains("failed to normalize upstream base URL"));
    assert!(fragment_message.contains("upstream base URL cannot include query or fragment"));
}
