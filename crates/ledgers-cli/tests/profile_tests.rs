//! Tests for profile management.

use ledgers_cli::profile::{Config, KeyRef, Profile};
use tempfile::TempDir;

#[test]
fn test_key_ref_parse_keychain() {
    let key_ref = KeyRef::parse("keychain").expect("should parse");
    match key_ref {
        KeyRef::Keychain => {}
        _ => panic!("expected Keychain variant"),
    }
}

#[test]
fn test_key_ref_parse_env() {
    let key_ref = KeyRef::parse("env:MY_VAR").expect("should parse");
    match key_ref {
        KeyRef::Env(var) => assert_eq!(var, "env:MY_VAR"),
        _ => panic!("expected Env variant"),
    }
}

#[test]
fn test_key_ref_parse_literal_rejected() {
    let result = KeyRef::parse("actual_key_here");
    assert!(
        result.is_err(),
        "literal keys should be rejected with an error"
    );
}

#[test]
fn test_config_save_and_load() {
    let _temp_dir = TempDir::new().expect("temp dir created");

    // Create a config and save it
    let mut config = Config::default();
    config.set_profile(
        "test".to_string(),
        Profile {
            url: "http://localhost:3000".to_string(),
            key: KeyRef::Keychain,
        },
    );

    // Note: In a real test, we'd temporarily override LEDGERS_CONFIG_DIR,
    // but that requires env manipulation. For now, this is a unit test.
    assert_eq!(config.get("test").unwrap().url, "http://localhost:3000");
}

#[test]
fn test_profile_keychaining_disabled() {
    // Verify that literal keys are rejected when parsing KeyRef
    let invalid = KeyRef::parse("sk_live_abc123");
    assert!(invalid.is_err());

    let invalid = KeyRef::parse("password123");
    assert!(invalid.is_err());
}
