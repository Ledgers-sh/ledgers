//! Profile management: config file, key storage, and profile selection.

use crate::error::{CliError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A profile: URL and key reference (keychain, env var, or rejected literal).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    /// The server URL.
    pub url: String,
    /// The key reference: "keychain" or "env:VAR_NAME".
    /// Literal keys are rejected at load time.
    pub key: KeyRef,
}

/// How to obtain the API key.
#[derive(Clone, Debug)]
pub enum KeyRef {
    /// Use the OS keychain (stored under service="ledgers", account=<profile_name>).
    Keychain,
    /// Use an environment variable.
    Env(String),
}

impl KeyRef {
    /// Parses a key ref from a string.
    ///
    /// # Errors
    /// Returns an error if the string looks like a literal key (anything other than
    /// "keychain" or "env:VAR").
    pub fn parse(s: &str) -> Result<Self> {
        if s == "keychain" {
            Ok(KeyRef::Keychain)
        } else if let Some(var) = s.strip_prefix("env:") {
            if var.is_empty() {
                Err(CliError::Config(
                    "env: prefix requires a variable name".to_string(),
                ))
            } else {
                Ok(KeyRef::Env(format!("env:{var}")))
            }
        } else {
            Err(CliError::Config(
                "literal keys must not be stored in config; use 'keychain' or 'env:VAR'"
                    .to_string(),
            ))
        }
    }
}

impl std::fmt::Display for KeyRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyRef::Keychain => write!(f, "keychain"),
            KeyRef::Env(var) => write!(f, "{var}"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for KeyRef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        KeyRef::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for KeyRef {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.to_string().serialize(serializer)
    }
}

/// The configuration file: profiles by name.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    /// Loads the config file from the standard location.
    /// Returns an empty config if the file doesn't exist.
    ///
    /// # Errors
    /// Returns an error if the file exists but is invalid.
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| CliError::Config(format!("cannot read config: {e}")))?;

        toml::from_str(&content).map_err(|e| CliError::Config(format!("invalid config: {e}")))
    }

    /// Saves the config file to the standard location.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let path = config_path()?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::Config(format!("cannot create config directory: {e}")))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::Config(format!("cannot serialize config: {e}")))?;

        std::fs::write(&path, content)
            .map_err(|e| CliError::Config(format!("cannot write config: {e}")))?;

        Ok(())
    }

    /// Gets a profile by name.
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Adds or updates a profile.
    pub fn set_profile(&mut self, name: String, profile: Profile) {
        self.profiles.insert(name, profile);
    }
}

/// The standard config file path.
/// Priority: `$LEDGERS_CONFIG_DIR/config.toml`, then `$XDG_CONFIG_HOME/ledgers/config.toml`,
/// then `~/.config/ledgers/config.toml`.
fn config_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("LEDGERS_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("config.toml"));
    }

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("ledgers").join("config.toml"));
    }

    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".config").join("ledgers").join("config.toml"));
    }

    Err(CliError::Config(
        "cannot determine config directory".to_string(),
    ))
}

/// Loads a profile: from env overrides or config file.
///
/// Priority:
/// 1. `LEDGERS_URL` and `LEDGERS_KEY` env vars (both as literals)
/// 2. Selected profile from config (by --profile, LEDGERS_PROFILE, or "default")
///
/// # Errors
/// Returns an error if the profile is not found or config is invalid.
pub fn load_profile(name: Option<&str>) -> Result<(String, String)> {
    // Check env overrides first
    if let (Ok(url), Ok(key)) = (std::env::var("LEDGERS_URL"), std::env::var("LEDGERS_KEY")) {
        return Ok((url, key));
    }

    // Load config
    let config = Config::load()?;

    // Determine profile name
    let profile_name = if let Some(n) = name {
        n.to_string()
    } else {
        std::env::var("LEDGERS_PROFILE").unwrap_or_else(|_| "default".to_string())
    };

    // Get profile
    let profile = config
        .get(&profile_name)
        .ok_or_else(|| CliError::Profile(format!("profile '{}' not found", profile_name)))?;

    // Resolve URL
    let url = profile.url.clone();

    // Resolve key
    let key = match &profile.key {
        KeyRef::Keychain => keyring::Entry::new("ledgers", &profile_name)
            .map_err(|e| CliError::Keychain(format!("cannot access keychain: {e}")))?
            .get_password()
            .map_err(|e| {
                CliError::Keychain(format!(
                    "no key in keychain for profile '{}': {e}",
                    profile_name
                ))
            })?,
        KeyRef::Env(var) => std::env::var(var)
            .map_err(|_| CliError::Profile(format!("environment variable {var} not set")))?,
    };

    Ok((url, key))
}

/// Stores a key in the OS keychain for a profile.
///
/// # Errors
/// Returns an error if the keychain cannot be accessed.
pub fn store_key_in_keychain(profile_name: &str, key: &str) -> Result<()> {
    keyring::Entry::new("ledgers", profile_name)
        .map_err(|e| CliError::Keychain(format!("cannot access keychain: {e}")))?
        .set_password(key)
        .map_err(|e| CliError::Keychain(format!("cannot store key in keychain: {e}")))?;
    Ok(())
}

/// Saves a profile: URL and key reference.
/// The key is stored in the keychain if key_ref is "keychain".
///
/// # Errors
/// Returns an error if the config cannot be saved or keychain access fails.
pub fn save_profile(name: String, url: String, key_ref: &str) -> Result<()> {
    let key = KeyRef::parse(key_ref)?;

    let mut config = Config::load()?;
    config.set_profile(name, Profile { url, key });
    config.save()?;

    Ok(())
}
