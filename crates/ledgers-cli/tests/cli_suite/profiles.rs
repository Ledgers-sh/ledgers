//! Profiles: config file contents, key references, and connection resolution.

use ledgers_cli::app::GlobalFlags;
use ledgers_cli::profile::{Config, KeyRef, Profile, resolve};
use ledgers_cli::{CliError, Context};
use tempfile::TempDir;

const SECRET: &str = "sk_live_must_never_be_written_to_config";

fn config_in(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("ledgers").join("config.toml")
}

fn context(config: std::path::PathBuf) -> Context {
    Context {
        config_path: Some(config),
        url: None,
        api_key: None,
        profile: None,
    }
}

fn globals() -> GlobalFlags {
    GlobalFlags::default()
}

#[test]
fn key_refs_accept_keychain_and_env_only() {
    assert_eq!(KeyRef::parse("keychain").expect("parses"), KeyRef::Keychain);
    assert_eq!(
        KeyRef::parse("env:PROD_KEY").expect("parses"),
        KeyRef::Env("PROD_KEY".to_string())
    );
    assert!(KeyRef::parse(SECRET).is_err(), "literal keys are rejected");
    assert!(KeyRef::parse("env:").is_err(), "env: needs a variable name");
}

#[test]
fn saved_config_holds_key_references_never_the_key() {
    let dir = TempDir::new().expect("temp dir");
    let path = config_in(&dir);
    let mut config = Config::default();
    config.profiles.insert(
        "default".to_string(),
        Profile {
            url: "http://localhost:3000".to_string(),
            key: KeyRef::Keychain,
        },
    );
    config.profiles.insert(
        "production".to_string(),
        Profile {
            url: "https://ledgers.example.com".to_string(),
            key: KeyRef::Env("PROD_LEDGERS_KEY".to_string()),
        },
    );
    config.save_to(&path).expect("saves");

    let text = std::fs::read_to_string(&path).expect("reads back");
    assert!(text.contains(r#"key = "keychain""#), "{text}");
    assert!(text.contains(r#"key = "env:PROD_LEDGERS_KEY""#), "{text}");
    assert_eq!(Config::load_from(&path).expect("loads"), config);
}

#[test]
fn a_config_with_a_literal_key_is_rejected() {
    let dir = TempDir::new().expect("temp dir");
    let path = config_in(&dir);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        format!("[profiles.default]\nurl = \"http://localhost:3000\"\nkey = \"{SECRET}\"\n"),
    )
    .expect("write");

    let err = Config::load_from(&path).expect_err("literal key must not load");
    assert!(matches!(err, CliError::Config(_)), "{err}");
    assert!(
        !err.to_string().contains(SECRET),
        "error must not echo the key"
    );
}

#[test]
fn a_missing_config_file_is_an_empty_config() {
    let dir = TempDir::new().expect("temp dir");
    let config = Config::load_from(&config_in(&dir)).expect("loads");
    assert!(config.profiles.is_empty());
}

#[test]
fn profile_supplies_url_and_env_key_supplies_the_key() {
    let dir = TempDir::new().expect("temp dir");
    let path = config_in(&dir);
    let mut config = Config::default();
    config.profiles.insert(
        "default".to_string(),
        Profile {
            url: "http://from-profile:3000".to_string(),
            key: KeyRef::Keychain,
        },
    );
    config.save_to(&path).expect("saves");

    let mut ctx = context(path);
    ctx.api_key = Some("env-key".to_string());
    let connection = resolve(&globals(), &ctx).expect("resolves");
    assert_eq!(connection.url, "http://from-profile:3000");
    assert_eq!(connection.key, "env-key");
}

#[test]
fn flag_url_overrides_env_and_profile() {
    let dir = TempDir::new().expect("temp dir");
    let path = config_in(&dir);
    let mut config = Config::default();
    config.profiles.insert(
        "default".to_string(),
        Profile {
            url: "http://from-profile:3000".to_string(),
            key: KeyRef::Keychain,
        },
    );
    config.save_to(&path).expect("saves");

    let mut ctx = context(path);
    ctx.url = Some("http://from-env:3000".to_string());
    ctx.api_key = Some("k".to_string());
    let flags = GlobalFlags {
        url: Some("http://from-flag:3000".to_string()),
        ..GlobalFlags::default()
    };
    assert_eq!(
        resolve(&flags, &ctx).expect("resolves").url,
        "http://from-flag:3000"
    );
}

#[test]
fn a_named_profile_that_does_not_exist_is_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let mut ctx = context(config_in(&dir));
    ctx.api_key = Some("k".to_string());
    ctx.url = Some("http://x".to_string());
    let flags = GlobalFlags {
        profile: Some("staging".to_string()),
        ..GlobalFlags::default()
    };
    let err = resolve(&flags, &ctx).expect_err("no such profile");
    assert!(
        err.to_string().contains("profile 'staging' not found"),
        "{err}"
    );
}

#[test]
fn no_url_anywhere_is_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let mut ctx = context(config_in(&dir));
    ctx.api_key = Some("k".to_string());
    let err = resolve(&globals(), &ctx).expect_err("no URL");
    assert!(err.to_string().contains("no server URL"), "{err}");
}

#[test]
fn no_key_anywhere_is_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let mut ctx = context(config_in(&dir));
    ctx.url = Some("http://x".to_string());
    let err = resolve(&globals(), &ctx).expect_err("no key");
    assert!(err.to_string().contains("LEDGERS_API_KEY"), "{err}");
}

#[test]
fn an_env_key_reference_to_an_unset_variable_names_the_variable() {
    let dir = TempDir::new().expect("temp dir");
    let path = config_in(&dir);
    let mut config = Config::default();
    config.profiles.insert(
        "default".to_string(),
        Profile {
            url: "http://x".to_string(),
            key: KeyRef::Env("LEDGERS_TEST_VAR_THAT_IS_NEVER_SET_8c1f".to_string()),
        },
    );
    config.save_to(&path).expect("saves");

    let err = resolve(&globals(), &context(path)).expect_err("unset variable");
    assert!(
        err.to_string()
            .contains("LEDGERS_TEST_VAR_THAT_IS_NEVER_SET_8c1f"),
        "{err}"
    );
}
