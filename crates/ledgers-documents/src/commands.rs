//! The document commands: registering an upload, and reading documents.
//!
//! `documents.register` is not in the public registry: only the upload route
//! calls it, after the bytes were sniffed and hashed. Its input carries the
//! server-computed fields (the hash, the sniffed MIME type, the text layer),
//! so a caller cannot register a document whose bytes the server did not see.

use crate::store::{self, Document, RetentionClass};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// The longest text layer stored with a document, in bytes. Longer text is
/// cut at a character boundary; the policy screen then sees less text.
pub const MAX_TEXT_LAYER_BYTES: usize = 512 * 1024;
/// The longest file name stored.
const MAX_NAME_CHARS: usize = 255;
/// The most documents one list call returns.
pub const MAX_LIST: u32 = 200;

/// Input of `documents.register` (internal; see the module docs).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterInput {
    /// The new document's id, when the first copy is stored.
    pub id: String,
    /// The SHA-256 of the bytes, lowercase hex.
    pub sha256: String,
    /// The size in bytes.
    pub size_bytes: i64,
    /// The sniffed MIME type.
    pub mime: String,
    /// The file name as uploaded.
    pub original_name: String,
    /// Who uploaded this copy.
    pub uploaded_by: String,
    /// The PDF text layer, if one was read.
    pub text_layer: Option<String>,
    /// The retention class; defaults to ten years.
    pub retention_class: Option<RetentionClass>,
}

/// Output of `documents.register`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterOutput {
    /// The stored document (the existing one when `duplicate` is true).
    pub document: Document,
    /// True when these bytes were already stored: one document, one more upload event.
    pub duplicate: bool,
}

/// `documents.register`: stores a document, or records another upload of an
/// identical file.
pub struct DocumentsRegister;

#[async_trait::async_trait]
impl Command for DocumentsRegister {
    const NAME: &'static str = "documents.register";
    const TIER: Tier = Tier::Draft;
    const IDEMPOTENT: bool = false;

    type Input = RegisterInput;
    type Output = RegisterOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        validate(input)?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        validate(input)?;
        let event_id = format!("upl_{}", ctx.event_id);
        let name = clean_name(&input.original_name);
        let existing = store::find_by_hash(db, &ctx.company_id, &input.sha256).await?;
        let mut statements = Vec::new();
        let (document, duplicate) = if let Some(document) = existing {
            (document, true)
        } else {
            {
                let retention = input.retention_class.unwrap_or(RetentionClass::TenYears);
                let text = input
                    .text_layer
                    .as_deref()
                    .map(|text| truncate_utf8(text, MAX_TEXT_LAYER_BYTES).to_owned());
                statements.push(Statement::with_values(
                    "INSERT INTO document (id, company_id, sha256, size_bytes, mime, original_name, \
                     uploaded_by, retention_class, text_layer, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                        .to_owned(),
                    vec![
                        Value::from(input.id.as_str()),
                        Value::from(ctx.company_id.as_str()),
                        Value::from(input.sha256.as_str()),
                        Value::from(input.size_bytes),
                        Value::from(input.mime.as_str()),
                        Value::from(name.as_str()),
                        Value::from(input.uploaded_by.as_str()),
                        Value::from(retention.as_str()),
                        Value::from(text),
                        Value::from(ctx.now.as_str()),
                    ],
                ));
                let document = Document {
                    id: input.id.clone(),
                    sha256: input.sha256.clone(),
                    size_bytes: input.size_bytes,
                    mime: input.mime.clone(),
                    original_name: name.clone(),
                    uploaded_by: input.uploaded_by.clone(),
                    retention_class: retention,
                    created_at: ctx.now.clone(),
                    has_text_layer: input.text_layer.is_some(),
                };
                (document, false)
            }
        };
        statements.push(upload_event(
            &event_id,
            ctx,
            &document.id,
            &input.uploaded_by,
            &name,
        ));
        let mut plan = Plan::with_output(RegisterOutput {
            document: document.clone(),
            duplicate,
        });
        plan.statements = statements;
        plan.objects = vec![document.id];
        Ok(plan)
    }
}

fn upload_event(
    event_id: &str,
    ctx: &CommandContext,
    document_id: &str,
    by: &str,
    name: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO document_upload_event (id, company_id, document_id, uploaded_by, original_name, uploaded_at) \
         VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(event_id),
            Value::from(ctx.company_id.as_str()),
            Value::from(document_id),
            Value::from(by),
            Value::from(name),
            Value::from(ctx.now.as_str()),
        ],
    )
}

fn validate(input: &RegisterInput) -> Result<(), CommandError> {
    let hash_ok = input.sha256.len() == 64
        && input
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !hash_ok {
        return Err(CommandError::InvalidInput(
            "sha256 must be 64 lowercase hex digits".into(),
        ));
    }
    if !matches!(
        input.mime.as_str(),
        "application/pdf" | "image/png" | "image/jpeg"
    ) {
        return Err(CommandError::InvalidInput(format!(
            "mime {} is not stored",
            input.mime
        )));
    }
    let limit = i64::try_from(crate::sniff::MAX_BYTES).unwrap_or(i64::MAX);
    if input.size_bytes <= 0 || input.size_bytes > limit {
        return Err(CommandError::InvalidInput(
            "size_bytes is outside the upload limit".into(),
        ));
    }
    Ok(())
}

/// Keeps the base name only, without control characters, and caps its length.
#[must_use]
pub fn clean_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    if cleaned.trim().is_empty() {
        "document".to_owned()
    } else {
        cleaned
    }
}

fn truncate_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Input of `documents.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListInput {
    /// The most rows to return (default 50, cap 200).
    pub limit: Option<u32>,
}

/// Output of `documents.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListOutput {
    /// Documents, newest first.
    pub documents: Vec<Document>,
}

/// `documents.list`.
pub struct DocumentsList;

#[async_trait::async_trait]
impl Command for DocumentsList {
    const NAME: &'static str = "documents.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = ListInput;
    type Output = ListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let limit = input.limit.unwrap_or(50).clamp(1, MAX_LIST);
        let documents = store::list(db, &ctx.company_id, limit).await?;
        Ok(Plan::with_output(ListOutput { documents }))
    }
}

/// Input of `documents.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetInput {
    /// The document id (from the path).
    pub id: String,
}

/// Output of `documents.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetOutput {
    /// The document.
    pub document: Document,
}

/// `documents.get`: one document. A missing id is a 404 problem.
pub struct DocumentsGet;

#[async_trait::async_trait]
impl Command for DocumentsGet {
    const NAME: &'static str = "documents.get";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = GetInput;
    type Output = GetOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let document = store::get(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        Ok(Plan::with_output(GetOutput { document }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_keeps_the_base_name_only() {
        assert_eq!(clean_name("../../etc/Rechnung 12.pdf"), "Rechnung 12.pdf");
        assert_eq!(clean_name("C:\\Users\\x\\scan.jpg"), "scan.jpg");
        assert_eq!(clean_name("bad\u{0007}name.pdf"), "badname.pdf");
        assert_eq!(clean_name("   "), "document");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        let text = "ab€cd";
        assert_eq!(truncate_utf8(text, 4), "ab");
        assert_eq!(truncate_utf8(text, 5), "ab€");
        assert_eq!(truncate_utf8(text, 100), text);
    }
}
