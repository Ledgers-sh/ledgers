//! Bills and documents in the dispatch: the extraction flow (`bills.extract`),
//! signed download links (`documents.download_url`), and the glue that fills
//! server-owned fields (ids, the caller's label) before a command runs.
//!
//! `bills.extract` reads the stored file, runs the model extractor when a key
//! is configured (the rule extractor over the PDF text layer otherwise), and
//! then creates the bill through `bills.create`, so every write is the
//! executor's. The model is called only after the caller passed the tier check
//! of `bills.extract` itself, so an unauthorised caller spends nothing.

use super::{Deps, ServerDispatch};
use crate::routes::{Dispatch, Presented};
use bytes::Bytes;
use cratefield_core::{HttpClient, Problem};
use ledgers_bills::commands::BillsCreate;
use ledgers_bills::model::{BillLine, BillView};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_documents::sign;
use ledgers_documents::store::{self, Document};
use ledgers_extract::llm::Transport;
use ledgers_extract::minimax::Pause;
use ledgers_extract::rules::RuleExtractor;
use ledgers_extract::schema::ExtractedInvoice;
use ledgers_extract::{DocumentInput, ExtractError, Extraction, Extractor, Reader, Source};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The warning a scanned PDF gets when no configured reader can take it.
pub const NEEDS_PHOTO_WARNING: &str = "A scanned PDF needs a photo upload or the Anthropic provider. Type the fields in by hand, or upload a photo of the invoice.";

/// `bills.extract`'s gate and reader: the caller needs the draft tier, and
/// the document must be stored.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExtractInput {
    /// The stored document to read.
    pub document_id: String,
}

/// Output of `bills.extract`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExtractOutput {
    /// The stored document.
    pub document: Document,
    /// Whether a text layer was read from it.
    pub has_text_layer: bool,
}

/// `bills.extract`'s gate: runs under the caller's credential and tier.
pub struct BillsExtractGate;

#[async_trait::async_trait]
impl Command for BillsExtractGate {
    const NAME: &'static str = "bills.extract";
    const TIER: Tier = Tier::Draft;

    type Input = ExtractInput;
    type Output = ExtractOutput;

    async fn preview(
        &self,
        _db: &dyn cratefield_core::Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn cratefield_core::Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let document = store::get(db, &ctx.company_id, &input.document_id)
            .await?
            .ok_or_else(|| CommandError::Domain(Problem::not_found()))?;
        let has_text_layer = document.has_text_layer;
        Ok(Plan::with_output(ExtractOutput {
            document,
            has_text_layer,
        }))
    }
}

/// Output of `bills.extract`: the bill and how its fields were read.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExtractReport {
    /// `model`, `text-layer` or `manual` (nothing could be read).
    pub mode: String,
    /// What the person should know about the reading.
    pub warnings: Vec<String>,
    /// True when the file is a scanned PDF that no configured reader takes:
    /// it needs a photo upload or the fields typed in by hand.
    pub needs_photo: bool,
}

/// What the readers made of one document.
enum ReadOutcome {
    /// Fields were read, by a model or by the rules.
    Read(Box<Extraction>),
    /// A scanned PDF no reader takes.
    NeedsPhoto,
    /// Nothing could be read, and the reason is in the warnings.
    Unread,
}

/// Waits on the harness clock. The retry of a model call uses it.
pub struct ClockPause(pub Arc<dyn cratefield_core::Clock>);

#[async_trait::async_trait]
impl Pause for ClockPause {
    async fn pause(&self, delay: Duration) {
        // The timeout of a never-ready future is the wait itself.
        let _ =
            cratefield_core::timeout::<()>(self.0.as_ref(), std::future::pending::<()>(), delay)
                .await;
    }
}

/// The response of `bills.extract`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExtractResponse {
    /// The draft bill.
    pub bill: BillView,
    /// The document it is for.
    pub document: Document,
    /// How the fields were read.
    pub extraction: ExtractReport,
}

/// The harness HTTP port as the extractor's transport.
pub struct PortTransport(pub Arc<dyn HttpClient>);

#[async_trait::async_trait]
impl Transport for PortTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let mut builder = http::Request::builder().method(http::Method::POST).uri(url);
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        let payload = serde_json::to_vec(body).map_err(|err| err.to_string())?;
        let request = builder
            .body(Bytes::from(payload))
            .map_err(|err| err.to_string())?;
        let response = self.0.send(request).await.map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let answer = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Ok((status, answer))
    }
}

/// The placeholder vendor of a draft nothing could read. A bill needs a
/// vendor, so the draft carries this name until a person replaces it; the
/// review screen says why.
pub const UNREAD_VENDOR: &str = "Unread document";

/// The fields of a draft nothing could read: no amounts, the placeholder vendor.
fn unread_draft() -> ExtractedInvoice {
    ExtractedInvoice {
        vendor_name: Some(UNREAD_VENDOR.to_owned()),
        ..ExtractedInvoice::default()
    }
}

/// The draft fields a read outcome makes: the invoice, the source and reader
/// it came from, the mode the report names, and whether a photo is needed.
fn draft_of(
    outcome: ReadOutcome,
) -> (
    ExtractedInvoice,
    Option<Source>,
    Option<Reader>,
    &'static str,
    bool,
) {
    match outcome {
        ReadOutcome::Read(extraction) => {
            let Extraction {
                invoice,
                source,
                reader,
            } = *extraction;
            let mode = match source {
                Source::Llm => "model",
                Source::Rules => "text-layer",
            };
            (invoice, Some(source), Some(reader), mode, false)
        }
        ReadOutcome::NeedsPhoto => (
            unread_draft(),
            None,
            Some(Reader::UnreadScan),
            "manual",
            true,
        ),
        ReadOutcome::Unread => (unread_draft(), None, None, "manual", false),
    }
}

/// The bill input an extraction becomes, before the command checks it.
fn bill_input_of(
    invoice: &ExtractedInvoice,
    source: &str,
    reader: Option<&str>,
    document_id: &str,
    created_by: &str,
) -> Value {
    let lines: Vec<BillLine> = invoice.lines.iter().map(BillLine::from).collect();
    json!({
        "id": "",
        "document_id": document_id,
        "vendor_name": invoice.vendor_name,
        "vendor_vat_id": invoice.vendor_vat_id,
        "vendor_iban": invoice.vendor_iban,
        "invoice_number": invoice.invoice_number,
        "invoice_date": invoice.invoice_date,
        "due_date": invoice.due_date,
        "currency": invoice.currency,
        "net_minor": invoice.net_minor,
        "vat_minor": invoice.vat_minor,
        "gross_minor": invoice.gross_minor,
        "lines": lines,
        "source": source,
        "reader": reader,
        "created_by": created_by,
    })
}

impl ServerDispatch {
    /// The label a caller's writes are recorded under: `agent:<key id>` or
    /// `person:<person id>`. Taken from the credential, never from the body.
    pub(super) async fn caller_label(
        &self,
        deps: &Deps,
        presented: Option<&Presented>,
        request_id: &str,
    ) -> Result<String, Problem> {
        match presented {
            Some(Presented::Bearer(token)) => {
                let principal = self.principal(deps, token).await?;
                Ok(principal_label(&principal))
            }
            Some(Presented::Principal(principal)) => Ok(principal_label(principal)),
            Some(Presented::Session(_)) => {
                let person = self
                    .person_id(deps, presented.cloned(), request_id, None)
                    .await?;
                Ok(format!("person:{person}"))
            }
            None => Err(CommandError::Unauthenticated
                .to_problem()
                .instance(request_id)),
        }
    }

    /// `bills.extract`: reads the stored document and creates a draft bill.
    pub(super) async fn bills_extract(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        let gate = self
            .execute(
                deps,
                &BillsExtractGate,
                presented.clone(),
                request_id,
                input,
            )
            .await?;
        let gated: ExtractOutput = serde_json::from_value(gate).map_err(|err| {
            Problem::internal()
                .with_detail(format!("extract gate: {err}"))
                .instance(request_id)
        })?;
        let document = gated.document;
        let text = store::text_layer(deps.db.as_ref(), self.company_id(), &document.id)
            .await
            .map_err(|err| {
                CommandError::Database(err)
                    .to_problem()
                    .instance(request_id)
            })?;
        let bytes = match &deps.blob {
            Some(blob) => blob
                .get(&crate::documents::blob_key(&document.sha256))
                .await
                .map_err(|_| {
                    Problem::internal()
                        .with_detail("document storage failed")
                        .instance(request_id)
                })?
                .map(|object| object.bytes),
            None => None,
        };
        let mut warnings = Vec::new();
        let doc_input = DocumentInput {
            bytes: bytes.as_deref().unwrap_or(&[]),
            mime: &document.mime,
            text_layer: text.as_deref(),
        };
        let outcome = self.read_document(deps, &doc_input, &mut warnings).await;
        let (invoice, source, reader, mode, needs_photo) = draft_of(outcome);
        let source_name = source.map_or("manual", Source::as_str);
        let mut bill_input = bill_input_of(
            &invoice,
            source_name,
            reader.map(Reader::as_str),
            &document.id,
            &caller,
        );
        bill_input["id"] = json!(deps.id_gen.ulid());
        let created = self
            .execute(deps, &BillsCreate, presented, request_id, bill_input)
            .await?;
        let bill = created.get("bill").cloned().ok_or_else(|| {
            Problem::internal()
                .with_detail("bill missing from create")
                .instance(request_id)
        })?;
        let response = ExtractResponse {
            bill: serde_json::from_value(bill).map_err(|err| {
                Problem::internal()
                    .with_detail(format!("bill view: {err}"))
                    .instance(request_id)
            })?,
            document,
            extraction: ExtractReport {
                mode: mode.to_owned(),
                warnings,
                needs_photo,
            },
        };
        serde_json::to_value(response).map_err(|err| {
            Problem::internal()
                .with_detail(err.to_string())
                .instance(request_id)
        })
    }

    /// Runs the configured model reader, then the rules over the text layer
    /// when the model fails. A scanned PDF that no reader takes is reported as
    /// such, with the photo flag, and never as a silent empty draft.
    async fn read_document(
        &self,
        deps: &Deps,
        doc: &DocumentInput<'_>,
        warnings: &mut Vec<String>,
    ) -> ReadOutcome {
        if let (Some(settings), Some(http)) = (&self.model, &deps.http) {
            let reader = settings.extractor(
                Arc::new(PortTransport(http.clone())),
                Arc::new(ClockPause(deps.clock.clone())),
            );
            match reader.extract(doc).await {
                Ok(extraction) => return ReadOutcome::Read(Box::new(extraction)),
                Err(ExtractError::NeedsPhoto) => {
                    warnings.push(NEEDS_PHOTO_WARNING.to_owned());
                    return ReadOutcome::NeedsPhoto;
                }
                Err(err) => warnings.push(format!(
                    "The model could not read this document ({err}). The text layer was used instead."
                )),
            }
        } else if self.model.is_none() {
            warnings.push(
                "No model key is configured, so only the PDF text layer is read. Scans and images need the fields typed in by hand."
                    .to_owned(),
            );
        }
        match RuleExtractor.extract(doc).await {
            Ok(extraction) => ReadOutcome::Read(Box::new(extraction)),
            Err(_) if doc.mime == "application/pdf" && doc.text_layer.is_none() => {
                warnings.push(NEEDS_PHOTO_WARNING.to_owned());
                ReadOutcome::NeedsPhoto
            }
            Err(err) => {
                warnings.push(err.to_string());
                ReadOutcome::Unread
            }
        }
    }

    /// `documents.download_url`: a link that serves the document until it
    /// expires (five minutes).
    pub(super) async fn documents_download_url(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Problem::validation_failed("the document id is missing").instance(request_id)
            })?
            .to_owned();
        let read = self
            .execute(
                deps,
                &ledgers_documents::commands::DocumentsGet,
                presented,
                request_id,
                json!({ "id": id }),
            )
            .await?;
        let document: Document =
            serde_json::from_value(read["document"].clone()).map_err(|err| {
                Problem::internal()
                    .with_detail(err.to_string())
                    .instance(request_id)
            })?;
        if self.document_key.is_empty() {
            return Err(
                Problem::not_ready("document download links are not configured")
                    .instance(request_id),
            );
        }
        let expires_at = deps.clock.now().unix_timestamp() + sign::LINK_TTL_SECONDS;
        let signature = sign::sign(
            &self.document_key,
            &document.id,
            &document.sha256,
            expires_at,
        );
        Ok(json!({
            "url": format!("/v1/documents/{}/file?expires={expires_at}&signature={signature}", document.id),
            "expires_at": expires_at,
        }))
    }
}

/// The label of a principal in the audit trail and on the records it wrote.
fn principal_label(principal: &ledgers_commands::auth::Principal) -> String {
    match principal {
        ledgers_commands::auth::Principal::Person { person_id, .. } => {
            format!("person:{person_id}")
        }
        ledgers_commands::auth::Principal::Agent { key_id, .. } => format!("agent:{key_id}"),
    }
}
