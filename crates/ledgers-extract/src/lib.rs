#![forbid(unsafe_code)]
//! Extraction of vendor invoices into structured, checked fields (docs/PLAN.md
//! section 6, issue #21).
//!
//! Two extractors implement [`Extractor`]:
//!
//! - [`rules::RuleExtractor`] reads the PDF text layer with fixed rules (no
//!   model, no network). It finds the common fields and never the line items,
//!   so its drafts always need a person.
//! - [`llm::AnthropicExtractor`] sends the file (a PDF document block or an
//!   image) to the Messages API and reads the fields back through one forced
//!   tool call with the [`schema::ExtractedInvoice`] schema.
//!
//! Whatever an extractor returns goes through [`checks`] (arithmetic, dates,
//! currency, grounding in the text) and [`guard`] (instruction-like text).
//! Nothing here posts anything: a failed check or a flag makes a draft that
//! needs review.

pub mod checks;
pub mod fixture;
pub mod guard;
pub mod llm;
pub mod money;
pub mod pdf;
pub mod rules;
pub mod schema;

pub use schema::{ExtractedInvoice, ExtractedLine};

/// Which extractor produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The fixed rules over the PDF text layer.
    Rules,
    /// The language model.
    Llm,
}

impl Source {
    /// The stored and reported name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Rules => "rules",
            Source::Llm => "llm",
        }
    }
}

/// The file an extractor reads.
#[derive(Clone, Copy, Debug)]
pub struct DocumentInput<'a> {
    /// The file bytes, as stored.
    pub bytes: &'a [u8],
    /// The sniffed MIME type.
    pub mime: &'a str,
    /// The PDF text layer, when one was read.
    pub text_layer: Option<&'a str>,
}

/// An extraction: the fields and the extractor that read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extraction {
    /// The extracted fields, unchecked.
    pub invoice: ExtractedInvoice,
    /// Which extractor read them.
    pub source: Source,
}

/// Why an extraction did not produce fields.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    /// The extractor cannot read this file (for example a scan under the rules).
    #[error("{0}")]
    NotReadable(String),
    /// The extraction service failed or refused.
    #[error("the extraction service failed: {0}")]
    Provider(String),
    /// The service answered, but not with the expected fields.
    #[error("the extraction result is malformed: {0}")]
    Malformed(String),
}

/// Reads one document into fields.
#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    /// Which extractor this is.
    fn source(&self) -> Source;

    /// Extracts the fields of `document`.
    ///
    /// # Errors
    /// [`ExtractError`] when the file cannot be read or the service fails.
    async fn extract(&self, document: &DocumentInput<'_>) -> Result<Extraction, ExtractError>;
}
