//! Which model reads documents, chosen from configuration.
//!
//! `LEDGERS_EXTRACT_PROVIDER` is `anthropic`, `minimax` or `rules`. When it is
//! unset, the Anthropic reader is used if `LEDGERS_EXTRACT_API_KEY` is set,
//! otherwise `MiniMax` if `LEDGERS_MINIMAX_API_KEY` is set, otherwise the rules
//! only. An explicit provider whose key is missing is a configuration error,
//! never a silent fallback to another provider.
//!
//! The keys live only in [`ModelSettings`]: its `Debug` redacts them, and the
//! error messages name the variables, never their values.

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

/// The default `MiniMax` model id when `LEDGERS_MINIMAX_MODEL` is unset.
pub const DEFAULT_MINIMAX_MODEL: &str = minimax::MODEL;

/// Selects the model reader from configuration values. `None` means the rules
/// only. Blank values count as unset.
///
/// # Errors
/// A message naming the variable that is wrong (never a key value).
pub fn select(
    provider: Option<&str>,
    anthropic_key: Option<&str>,
    minimax_key: Option<&str>,
    minimax_model: Option<&str>,
) -> Result<Option<ModelSettings>, String> {
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
    match clean(provider).map(|text| text.to_ascii_lowercase()) {
        None => Ok(match (anthropic_key, minimax_key) {
            (Some(key), _) => Some(anthropic(key)),
            (None, Some(key)) => Some(minimax(key)),
            (None, None) => None,
        }),
        Some(name) if name == "rules" => Ok(None),
        Some(name) if name == "anthropic" => match anthropic_key {
            Some(key) => Ok(Some(anthropic(key))),
            None => Err(
                "LEDGERS_EXTRACT_PROVIDER is anthropic but LEDGERS_EXTRACT_API_KEY is not set"
                    .to_owned(),
            ),
        },
        Some(name) if name == "minimax" => match minimax_key {
            Some(key) => Ok(Some(minimax(key))),
            None => Err(
                "LEDGERS_EXTRACT_PROVIDER is minimax but LEDGERS_MINIMAX_API_KEY is not set"
                    .to_owned(),
            ),
        },
        Some(other) => Err(format!(
            "LEDGERS_EXTRACT_PROVIDER must be anthropic, minimax or rules (got `{other}`)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_means_rules_only() {
        assert!(select(None, None, None, None).expect("ok").is_none());
        assert!(
            select(Some("  "), Some(""), Some(" "), None)
                .expect("ok")
                .is_none()
        );
    }

    #[test]
    fn the_anthropic_key_wins_when_no_provider_is_named() {
        let settings = select(None, Some("a-key"), Some("m-key"), None)
            .expect("ok")
            .expect("some");
        assert_eq!(settings.provider, Provider::Anthropic);
    }

    #[test]
    fn minimax_is_chosen_when_only_its_key_is_set() {
        let settings = select(None, None, Some("m-key"), Some("MiniMax-M3.1-Flash"))
            .expect("ok")
            .expect("some");
        assert_eq!(settings.provider, Provider::MiniMax);
        assert_eq!(settings.model, "MiniMax-M3.1-Flash");
    }

    #[test]
    fn an_explicit_provider_is_honoured_over_the_default() {
        let settings = select(Some("MiniMax"), Some("a-key"), Some("m-key"), None)
            .expect("ok")
            .expect("some");
        assert_eq!(settings.provider, Provider::MiniMax);
        assert_eq!(settings.model, minimax::MODEL);
    }

    #[test]
    fn rules_forces_the_rules_even_with_keys() {
        assert!(
            select(Some("rules"), Some("a"), Some("m"), None)
                .expect("ok")
                .is_none()
        );
    }

    #[test]
    fn an_explicit_provider_without_its_key_is_an_error_not_a_fallback() {
        let err = select(Some("minimax"), Some("a-key"), None, None).unwrap_err();
        assert!(err.contains("LEDGERS_MINIMAX_API_KEY"));
        let err = select(Some("anthropic"), None, Some("m-key"), None).unwrap_err();
        assert!(err.contains("LEDGERS_EXTRACT_API_KEY"));
    }

    #[test]
    fn an_unknown_provider_is_refused_by_name() {
        let err = select(Some("openai"), Some("a"), None, None).unwrap_err();
        assert!(err.contains("openai"));
    }

    #[test]
    fn debug_never_shows_the_key() {
        let settings = select(Some("minimax"), None, Some("sk-very-secret"), None)
            .expect("ok")
            .expect("some");
        let text = format!("{settings:?}");
        assert!(!text.contains("sk-very-secret"));
        assert!(text.contains("redacted"));
    }
}
