//! Which model reads documents, chosen from configuration.
//!
//! `LEDGERS_EXTRACT_PROVIDER` is `anthropic`, `minimax` or `rules`. When it is
//! unset, the Anthropic reader is used if `LEDGERS_EXTRACT_API_KEY` is set,
//! otherwise `MiniMax` if `LEDGERS_MINIMAX_API_KEY` is set, otherwise the rules
//! only.
//!
//! Invoice reading is optional, so a configuration that cannot be honoured
//! never stops the server. A named provider whose key is missing, or a name
//! that is not a provider, falls back to the rules reader and says so in
//! [`ExtractionStatus`]: the server logs one warning at startup and the status
//! read shows the state to the app.
//!
//! The keys live only in [`ModelSettings`]: its `Debug` redacts them, and the
//! warnings and status name the variables, never their values.

use crate::Extractor;
use crate::llm::{self, AnthropicExtractor, Transport};
use crate::minimax::{self, MiniMaxExtractor, Pause};
use std::sync::Arc;

/// A model provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// The Anthropic Messages API.
    Anthropic,
    /// `MiniMax`'s OpenAI-compatible chat API.
    MiniMax,
}

impl Provider {
    /// The provider's name in configuration and in the review screen.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::MiniMax => "minimax",
        }
    }
}

/// The selected model reader: the provider, its key and its model id.
#[derive(Clone)]
pub struct ModelSettings {
    /// The provider.
    pub provider: Provider,
    /// The model id the provider is called with.
    pub model: String,
    api_key: String,
}

impl std::fmt::Debug for ModelSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSettings")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl ModelSettings {
    /// Builds the extractor for these settings. `transport` makes the HTTP
    /// call; `pause` waits between a failed call and its one retry.
    #[must_use]
    pub fn extractor(
        &self,
        transport: Arc<dyn Transport>,
        pause: Arc<dyn Pause>,
    ) -> Arc<dyn Extractor> {
        match self.provider {
            Provider::Anthropic => {
                Arc::new(AnthropicExtractor::new(self.api_key.clone(), transport))
            }
            Provider::MiniMax => Arc::new(MiniMaxExtractor::new(
                self.api_key.clone(),
                self.model.clone(),
                transport,
                pause,
            )),
        }
    }
}

/// Why the configured reader is not the one in effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackReason {
    /// The named provider has no API key configured.
    MissingKey,
    /// The name is not `anthropic`, `minimax` or `rules`.
    UnknownProvider,
}

impl FallbackReason {
    /// The reason's name in the status read.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FallbackReason::MissingKey => "missing_key",
            FallbackReason::UnknownProvider => "unknown_provider",
        }
    }
}

/// The reader in effect and, when the configuration asked for another one,
/// what was asked for and why it is not in effect. Never holds key material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractionStatus {
    /// The reader in effect: `anthropic`, `minimax` or `rules`.
    pub provider: &'static str,
    /// The provider the configuration named (lower-cased), when it named one.
    pub configured: Option<String>,
    /// Why `configured` is not in effect; `None` when nothing fell back.
    pub reason: Option<FallbackReason>,
}

impl ExtractionStatus {
    /// The warning the server logs at startup, when a fallback happened.
    /// Names the variable, never a value.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        let configured = self.configured.as_deref()?;
        match self.reason? {
            FallbackReason::MissingKey => Some(format!(
                "LEDGERS_EXTRACT_PROVIDER is {configured} but its API key is not set; invoice reading uses the rules reader until the key is set"
            )),
            FallbackReason::UnknownProvider => Some(format!(
                "LEDGERS_EXTRACT_PROVIDER must be anthropic, minimax or rules (got `{configured}`); invoice reading uses the rules reader"
            )),
        }
    }
}

/// The outcome of [`select`]: the reader to build, if any, and its status.
#[derive(Clone)]
pub struct Selection {
    /// The settings of the reader in effect; `None` for the rules only.
    pub settings: Option<ModelSettings>,
    /// What is in effect and any fallback.
    pub status: ExtractionStatus,
}

/// The default `MiniMax` model id when `LEDGERS_MINIMAX_MODEL` is unset.
pub const DEFAULT_MINIMAX_MODEL: &str = minimax::MODEL;

/// Selects the model reader from configuration values. Blank values count as
/// unset. Never fails: a configuration that cannot be honoured selects the
/// rules reader and records why in the status.
#[must_use]
pub fn select(
    provider: Option<&str>,
    anthropic_key: Option<&str>,
    minimax_key: Option<&str>,
    minimax_model: Option<&str>,
) -> Selection {
    let clean = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let anthropic_key = clean(anthropic_key);
    let minimax_key = clean(minimax_key);
    let minimax_model = clean(minimax_model).unwrap_or_else(|| DEFAULT_MINIMAX_MODEL.to_owned());
    let anthropic = |key: String| ModelSettings {
        provider: Provider::Anthropic,
        model: llm::MODEL.to_owned(),
        api_key: key,
    };
    let minimax = |key: String| ModelSettings {
        provider: Provider::MiniMax,
        model: minimax_model.clone(),
        api_key: key,
    };
    let rules = |configured: Option<String>, reason: Option<FallbackReason>| Selection {
        settings: None,
        status: ExtractionStatus {
            provider: "rules",
            configured,
            reason,
        },
    };
    let in_effect = |settings: ModelSettings, configured: Option<String>| {
        let name = settings.provider.as_str();
        Selection {
            settings: Some(settings),
            status: ExtractionStatus {
                provider: name,
                configured,
                reason: None,
            },
        }
    };
    match clean(provider).map(|text| text.to_ascii_lowercase()) {
        None => match (anthropic_key, minimax_key) {
            (Some(key), _) => in_effect(anthropic(key), None),
            (None, Some(key)) => in_effect(minimax(key), None),
            (None, None) => rules(None, None),
        },
        Some(name) if name == "rules" => rules(Some(name), None),
        Some(name) if name == "anthropic" => match anthropic_key {
            Some(key) => in_effect(anthropic(key), Some(name)),
            None => rules(Some(name), Some(FallbackReason::MissingKey)),
        },
        Some(name) if name == "minimax" => match minimax_key {
            Some(key) => in_effect(minimax(key), Some(name)),
            None => rules(Some(name), Some(FallbackReason::MissingKey)),
        },
        Some(other) => rules(Some(other), Some(FallbackReason::UnknownProvider)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_of(selection: &Selection) -> &ModelSettings {
        selection
            .settings
            .as_ref()
            .expect("a model reader is selected")
    }

    #[test]
    fn nothing_configured_means_rules_only() {
        let selection = select(None, None, None, None);
        assert!(selection.settings.is_none());
        assert_eq!(selection.status.provider, "rules");
        assert_eq!(selection.status.configured, None);
        assert_eq!(selection.status.reason, None);
        assert!(selection.status.warning().is_none());
        assert!(
            select(Some("  "), Some(""), Some(" "), None)
                .settings
                .is_none()
        );
    }

    #[test]
    fn the_anthropic_key_wins_when_no_provider_is_named() {
        let selection = select(None, Some("a-key"), Some("m-key"), None);
        assert_eq!(settings_of(&selection).provider, Provider::Anthropic);
        assert_eq!(selection.status.provider, "anthropic");
    }

    #[test]
    fn minimax_is_chosen_when_only_its_key_is_set() {
        let selection = select(None, None, Some("m-key"), Some("MiniMax-M3.1-Flash"));
        assert_eq!(settings_of(&selection).provider, Provider::MiniMax);
        assert_eq!(settings_of(&selection).model, "MiniMax-M3.1-Flash");
        assert_eq!(selection.status.provider, "minimax");
    }

    #[test]
    fn an_explicit_provider_is_honoured_over_the_default() {
        let selection = select(Some("MiniMax"), Some("a-key"), Some("m-key"), None);
        assert_eq!(settings_of(&selection).provider, Provider::MiniMax);
        assert_eq!(settings_of(&selection).model, minimax::MODEL);
        assert_eq!(selection.status.configured.as_deref(), Some("minimax"));
        assert_eq!(selection.status.reason, None);
    }

    #[test]
    fn rules_forces_the_rules_even_with_keys() {
        let selection = select(Some("rules"), Some("a"), Some("m"), None);
        assert!(selection.settings.is_none());
        assert_eq!(selection.status.provider, "rules");
        assert_eq!(selection.status.reason, None);
    }

    #[test]
    fn a_named_provider_without_its_key_falls_back_to_the_rules_with_a_reason() {
        let selection = select(Some("minimax"), Some("a-key"), None, None);
        assert!(selection.settings.is_none(), "no reader without its key");
        assert_eq!(selection.status.provider, "rules");
        assert_eq!(selection.status.configured.as_deref(), Some("minimax"));
        assert_eq!(selection.status.reason, Some(FallbackReason::MissingKey));
        let warning = selection.status.warning().expect("a warning");
        assert!(warning.contains("LEDGERS_EXTRACT_PROVIDER"));
        assert!(!warning.contains("a-key"));

        let selection = select(Some("anthropic"), None, Some("m-key"), None);
        assert!(selection.settings.is_none());
        assert_eq!(selection.status.configured.as_deref(), Some("anthropic"));
        assert_eq!(selection.status.reason, Some(FallbackReason::MissingKey));
    }

    #[test]
    fn an_unknown_provider_falls_back_to_the_rules_by_name() {
        let selection = select(Some("openai"), Some("a"), None, None);
        assert!(selection.settings.is_none());
        assert_eq!(selection.status.provider, "rules");
        assert_eq!(selection.status.configured.as_deref(), Some("openai"));
        assert_eq!(
            selection.status.reason,
            Some(FallbackReason::UnknownProvider)
        );
        assert!(
            selection
                .status
                .warning()
                .expect("a warning")
                .contains("openai")
        );
    }

    #[test]
    fn the_status_reason_names_match_the_status_read() {
        assert_eq!(FallbackReason::MissingKey.as_str(), "missing_key");
        assert_eq!(FallbackReason::UnknownProvider.as_str(), "unknown_provider");
    }

    #[test]
    fn debug_never_shows_the_key() {
        let selection = select(Some("minimax"), None, Some("sk-very-secret"), None);
        let text = format!("{:?}", settings_of(&selection));
        assert!(!text.contains("sk-very-secret"));
        assert!(text.contains("redacted"));
        let status = format!("{:?}", selection.status);
        assert!(!status.contains("sk-very-secret"));
    }
}
