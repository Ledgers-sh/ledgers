//! The status read: which optional features are off, and why.
//!
//! `GET /v1/status` answers without credentials and carries no secret. It
//! names the reader in effect for invoice photos (and what was configured
//! when that differs), and each optional feature that is off with its reason.
//! Reasons name configuration variables, never their values. The app shows
//! them, for example "Invoice reading for photos is not set up: add the
//! `MiniMax` key".

use cratefield_core::axum::{self, Json, extract::State, routing::get};
use cratefield_core::{Config, ConfigError, Harness, Migrations, Module, ModuleContext, Port};
use ledgers_extract::provider::{ExtractionStatus, FallbackReason};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// An optional feature that is off because its configuration is missing or
/// invalid. The server still runs: the feature's routes answer 503 with
/// `reason`, and the rest of the API is unaffected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unavailable {
    /// The feature's name in the status read (`sign_in`, `document_links`).
    pub feature: &'static str,
    /// What is missing or wrong, naming variables and never values.
    pub reason: String,
}

/// What the composition is running with: the invoice reader in effect and
/// the optional features that are off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The invoice reader in effect, and any fallback from the configuration.
    pub extraction: ExtractionStatus,
    /// The optional features that are off.
    pub unavailable: Vec<Unavailable>,
}

impl Status {
    /// The startup warnings: one per fallback or degraded feature. They name
    /// variables only.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self.extraction.warning().into_iter().collect();
        warnings.extend(
            self.unavailable
                .iter()
                .map(|item| format!("{} is off: {}", item.feature, item.reason)),
        );
        warnings
    }

    /// The JSON the status read returns.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "extraction": {
                "provider": self.extraction.provider,
                "configured": self.extraction.configured,
                "reason": self.extraction.reason.map(FallbackReason::as_str),
            },
            "unavailable": self.unavailable,
            "limits": {
                "document_max_bytes": ledgers_documents::sniff::MAX_BYTES,
                "pdf_max_pages": ledgers_documents::sniff::MAX_PDF_PAGES,
            },
        })
    }
}

/// A composed harness and what it is running with.
pub struct Composition {
    /// The harness: every module, mounted.
    pub harness: Harness,
    /// What is degraded, as the status read reports it.
    pub status: Status,
}

impl Composition {
    /// The startup warnings for the operator's log.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        self.status.warnings()
    }
}

/// The module behind `GET /v1/status`.
pub struct StatusModule {
    status: Arc<Status>,
}

impl StatusModule {
    /// The module serving `status`.
    #[must_use]
    pub fn new(status: Status) -> Self {
        Self {
            status: Arc::new(status),
        }
    }
}

impl Module for StatusModule {
    fn name(&self) -> &'static str {
        "status"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        axum::Router::new()
            .route("/", get(read_status))
            .with_state(self.status.clone())
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

async fn read_status(State(status): State<Arc<Status>>) -> Json<Value> {
    Json(status.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_names_the_document_limits_and_the_reader() {
        let status = Status {
            extraction: ExtractionStatus {
                provider: "rules",
                configured: Some("minimax".to_owned()),
                reason: Some(FallbackReason::MissingKey),
            },
            unavailable: Vec::new(),
        };
        let body = status.to_json();
        assert_eq!(body["extraction"]["provider"], "rules");
        assert_eq!(body["extraction"]["configured"], "minimax");
        assert_eq!(body["extraction"]["reason"], "missing_key");
        assert_eq!(body["limits"]["document_max_bytes"], 10 * 1024 * 1024);
        assert_eq!(body["limits"]["pdf_max_pages"], 50);
    }
}
