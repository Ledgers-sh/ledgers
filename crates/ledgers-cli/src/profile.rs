//! Profiles: the server URL and a *reference* to the API key.
//!
//! The config file (`~/.config/ledgers/config.toml`, or `$LEDGERS_CONFIG_DIR/config.toml`)
//! holds URLs and key references only. A literal key is rejected when the file is
//! read, so a secret cannot end up in plaintext config.
//!
//! ```toml
//! [profiles.default]
//! url = "http://localhost:3000"
//! key = "keychain"            # OS keychain, service "ledgers", account = profile name
//!
//! [profiles.production]
//! url = "https://ledgers.example.com"
//! key = "env:PROD_LEDGERS_KEY" # read from that environment variable
//! ```
//!
//! `LEDGERS_API_KEY` in the environment always wins over the profile's key reference.

use crate::Context;
use crate::app::GlobalFlags;
use crate::error::{CliError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Keychain service name under which profile keys are stored.
pub const KEYCHAIN_SERVICE: &str = "ledgers";

/// The profile used when neither `--profile` nor `LEDGERS_PROFILE` is set.
pub const DEFAULT_PROFILE: &str = "default";

/// A profile: the server URL and where the key lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The server base URL.
    pub url: String,
    /// Where the API key is read from.
    pub key: KeyRef,
}

/// A reference to an API key. Never the key itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRef {
    /// The OS keychain entry for the profile.
    Keychain,
    /// The named environment variable.
    Env(String),
}

impl KeyRef {
    /// Parses `keychain` or `env:VAR`.
    ///
    /// # Errors
    /// Rejects anything else, including a literal key.
    pub fn parse(s: &str) -> Result<Self> {
        if s == "keychain" {
            return Ok(KeyRef::Keychain);
        }
        match s.strip_prefix("env:") {
            Some(var) if !var.is_empty() => Ok(KeyRef::Env(var.to_string())),
            Some(_) => Err(CliError::Config(
                "env: prefix requires a variable name".into(),
            )),
            None => Err(CliError::Config(
                "literal keys must not be stored in config; use 'keychain' or 'env:VAR'".into(),
            )),
        }
    }
}

impl std::fmt::Display for KeyRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyRef::Keychain => write!(f, "keychain"),
            KeyRef::Env(var) => write!(f, "env:{var}"),
        }
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

impl<'de> Deserialize<'de> for KeyRef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        KeyRef::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// The config file: profiles by name.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Profiles keyed by name.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    /// Loads a config file. A missing file is an empty config.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed, or
    /// holds a literal key.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| CliError::Config(format!("cannot read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| CliError::Config(config_parse_error(path, &content, &e)))
    }

    /// Writes the config file, creating its directory.
    ///
    /// # Errors
    /// Returns an error if the directory or file cannot be written.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::Config(format!("cannot create config directory: {e}")))?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::Config(format!("cannot serialize config: {e}")))?;
        std::fs::write(path, content)
            .map_err(|e| CliError::Config(format!("cannot write config: {e}")))
    }

    /// Looks up a profile by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }
}

/// A config parse error without the offending source text. The TOML error's
/// `Display` quotes the line it failed on, which can be a secret, so only its
/// message and line number are reported.
fn config_parse_error(path: &Path, content: &str, error: &toml::de::Error) -> String {
    let line = error.span().map(|span| {
        let before = content.get(..span.start).unwrap_or(content);
        before.matches('\n').count() + 1
    });
    match line {
        Some(line) => format!(
            "invalid config {} at line {line}: {}",
            path.display(),
            error.message()
        ),
        None => format!("invalid config {}: {}", path.display(), error.message()),
    }
}

/// The config file path: `$LEDGERS_CONFIG_DIR/config.toml` if the context sets it,
/// otherwise `~/.config/ledgers/config.toml`.
///
/// # Errors
/// Returns an error if no home directory is known.
pub fn config_file(ctx: &Context) -> Result<PathBuf> {
    if let Some(path) = &ctx.config_path {
        return Ok(path.clone());
    }
    // No `dirs` crate: it pulls in option-ext (MPL-2.0), which deny.toml does not allow.
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| CliError::Config("cannot determine the home directory".into()))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("ledgers")
        .join("config.toml"))
}

/// A resolved server URL and API key.
#[derive(Clone, Debug)]
pub struct Connection {
    /// The server base URL.
    pub url: String,
    /// The API key, sent as a Bearer token.
    pub key: String,
}

/// Resolves the connection for a command.
///
/// URL precedence: `--url`, then `LEDGERS_URL`, then the profile.
/// Key precedence: `LEDGERS_API_KEY`, then the profile's key reference.
///
/// # Errors
/// Returns an error if the named profile does not exist, no URL or key can be
/// found, the keychain has no entry, or the config is invalid.
pub fn resolve(globals: &GlobalFlags, ctx: &Context) -> Result<Connection> {
    let explicit_profile = globals.profile.as_deref().or(ctx.profile.as_deref());
    let url_override = globals.url.as_deref().or(ctx.url.as_deref());
    let env_key = ctx.api_key.as_deref();

    // Only read the config when a value is missing or a profile was named.
    let needs_config = url_override.is_none() || env_key.is_none() || explicit_profile.is_some();
    let profile = if needs_config {
        let path = config_file(ctx)?;
        let config = Config::load_from(&path)?;
        let name = explicit_profile.unwrap_or(DEFAULT_PROFILE);
        match (config.get(name), explicit_profile) {
            (Some(p), _) => Some((name.to_string(), p.clone())),
            (None, Some(_)) => {
                return Err(CliError::Profile(format!(
                    "profile '{name}' not found in {}",
                    path.display()
                )));
            }
            (None, None) => None,
        }
    } else {
        None
    };

    let url = match (url_override, &profile) {
        (Some(url), _) => url.to_string(),
        (None, Some((_, p))) => p.url.clone(),
        (None, None) => {
            return Err(CliError::Profile(
                "no server URL: pass --url, set LEDGERS_URL, or add a profile".into(),
            ));
        }
    };

    let key = match (env_key, &profile) {
        (Some(key), _) => key.to_string(),
        (None, Some((name, p))) => read_key(name, &p.key)?,
        (None, None) => {
            return Err(CliError::Profile(
                "no API key: set LEDGERS_API_KEY or configure a profile key".into(),
            ));
        }
    };

    Ok(Connection { url, key })
}

/// Reads a key from its reference.
fn read_key(profile: &str, key_ref: &KeyRef) -> Result<String> {
    match key_ref {
        KeyRef::Keychain => keyring::Entry::new(KEYCHAIN_SERVICE, profile)
            .and_then(|entry| entry.get_password())
            .map_err(|e| {
                CliError::Keychain(format!("no key in keychain for profile '{profile}': {e}"))
            }),
        KeyRef::Env(var) => std::env::var(var)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CliError::Profile(format!("environment variable {var} is not set"))),
    }
}
