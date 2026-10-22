//! The document commands: registering an upload, and reading documents.
//!
//! `documents.register` is not in the public registry: only the upload route
//! calls it, after the bytes were sniffed and hashed. Its input carries the
//! server-computed fields (the hash, the sniffed MIME type, the text layer),
//! so a caller cannot register a document whose bytes the server did not see.

use crate::anchor::Anchor;
use crate::store::{self, Document, RetentionClass};
use cratefield_core::{Database, Problem, Statement};
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
/// The most anchors one set call accepts.
const MAX_ANCHORS: u32 = 200;

/// Problem: document is linked to a posted entry or bill.
fn document_linked_problem() -> Problem {
    Problem::new(&cratefield_core::ProblemDef {
        slug: "document-linked",
        status: cratefield_core::axum::http::StatusCode::CONFLICT,
        title: "Document linked",
        description: "The document is linked to a posted entry, bill, or bank statement and cannot be deleted.",
    })
}

/// Escapes a string for safe use in a LIKE pattern with ESCAPE '\'.
/// _ is a LIKE wildcard, so we escape it (document IDs are ULIDs and never contain it, but this is defensive).
fn escape_like(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '%' => result.push_str("\\%"),
            '_' => result.push_str("\\_"),
            c => result.push(c),
        }
    }
    result
}

/// Returns a SQL condition (and its bind values) asserting that a document is NOT linked
/// to any posted journal entry, bill, or bank statement.
///
/// The condition is used both for the pre-flight read (to pick the error message) and
/// for the guard statement (which is the actual enforcement; see ADR 0004).
fn linked_condition(company_id: &str, document_id: &str) -> (&'static str, Vec<Value>) {
    let escaped_id = escape_like(document_id);
    let like_pattern = format!("%\"{escaped_id}\"%");
    let condition = "NOT EXISTS (SELECT 1 FROM journal_entry WHERE company_id = ? AND source_documents LIKE ? ESCAPE '\\') \
                     AND NOT EXISTS (SELECT 1 FROM bill WHERE company_id = ? AND document_id = ?) \
                     AND NOT EXISTS (SELECT 1 FROM bank_statement WHERE company_id = ? AND document_id = ?)";
    (
        condition,
        vec![
            Value::from(company_id),
            Value::from(like_pattern),
            Value::from(company_id),
            Value::from(document_id),
            Value::from(company_id),
            Value::from(document_id),
        ],
    )
}

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
    /// The source of the upload: 'upload', 'email', 'folder', 'api'; defaults to 'upload'.
    pub source: Option<String>,
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
        let source = input.source.as_deref().unwrap_or("upload");
        statements.push(upload_event(
            &event_id,
            ctx,
            &document.id,
            &input.uploaded_by,
            &name,
            source,
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
    source: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO document_upload_event (id, company_id, document_id, uploaded_by, original_name, uploaded_at, source) \
         VALUES (?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(event_id),
            Value::from(ctx.company_id.as_str()),
            Value::from(document_id),
            Value::from(by),
            Value::from(name),
            Value::from(ctx.now.as_str()),
            Value::from(source),
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
        "application/pdf"
            | "image/png"
            | "image/jpeg"
            | "application/xml"
            | "text/plain"
            | "text/csv"
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
    if let Some(source) = &input.source {
        #[allow(clippy::collapsible_if)]
        if !matches!(source.as_str(), "upload" | "email" | "folder" | "api") {
            return Err(CommandError::InvalidInput(
                "source must be 'upload', 'email', 'folder' or 'api'".into(),
            ));
        }
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

/// Input of `documents.set_anchors`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SetAnchorsInput {
    /// The document id.
    pub id: String,
    /// The anchors to set (replaces existing ones).
    pub anchors: Vec<Anchor>,
}

/// Output of `documents.set_anchors`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SetAnchorsOutput {
    /// The number of anchors set.
    pub count: u32,
}

/// `documents.set_anchors`: replaces all anchors for a document.
pub struct DocumentsSetAnchors;

#[async_trait::async_trait]
impl Command for DocumentsSetAnchors {
    const NAME: &'static str = "documents.set_anchors";
    const TIER: Tier = Tier::Draft;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = Some("id");

    type Input = SetAnchorsInput;
    type Output = SetAnchorsOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        if input.anchors.len() > MAX_ANCHORS as usize {
            return Err(CommandError::InvalidInput(format!(
                "anchors count exceeds limit {MAX_ANCHORS}"
            )));
        }
        for anchor in &input.anchors {
            anchor.validate().map_err(CommandError::InvalidInput)?;
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        if input.anchors.len() > MAX_ANCHORS as usize {
            return Err(CommandError::InvalidInput(format!(
                "anchors count exceeds limit {MAX_ANCHORS}"
            )));
        }
        for anchor in &input.anchors {
            anchor.validate().map_err(CommandError::InvalidInput)?;
        }
        // Check document exists in company.
        let _document = store::get(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::Domain(Problem::not_found()))?;

        let mut statements = Vec::new();
        // Delete existing anchors for this document.
        statements.push(Statement::with_values(
            "DELETE FROM document_anchor WHERE company_id = ? AND document_id = ?".to_owned(),
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));
        // Insert new anchors.
        for (idx, anchor) in input.anchors.iter().enumerate() {
            let anchor_id = format!("anc_{}_{}", ctx.event_id, idx);
            let (page, x, y, w, h, xml_path) = match &anchor.location {
                crate::Location::Page { page, bbox } => (
                    Some(*page),
                    Some(bbox.x),
                    Some(bbox.y),
                    Some(bbox.w),
                    Some(bbox.h),
                    None,
                ),
                crate::Location::Xml { path } => (None, None, None, None, None, Some(path.clone())),
            };
            statements.push(Statement::with_values(
                "INSERT INTO document_anchor (id, company_id, document_id, field, page, x, y, w, h, xml_path, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(anchor_id),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                    Value::from(anchor.field.as_str()),
                    Value::from(page.map(i64::from)),
                    Value::from(x.map(i64::from)),
                    Value::from(y.map(i64::from)),
                    Value::from(w.map(i64::from)),
                    Value::from(h.map(i64::from)),
                    Value::from(xml_path),
                    Value::from(ctx.now.as_str()),
                ],
            ));
        }
        // Safe: len() is guaranteed ≤ MAX_ANCHORS (u32) by earlier check
        #[allow(clippy::cast_possible_truncation)]
        let count = input.anchors.len() as u32;
        let mut plan = Plan::with_output(SetAnchorsOutput { count });
        plan.statements = statements;
        Ok(plan)
    }
}

/// Input of `documents.anchors`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AnchorsInput {
    /// The document id.
    pub id: String,
}

/// Output of `documents.anchors`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AnchorsOutput {
    /// The anchors.
    pub anchors: Vec<Anchor>,
}

/// `documents.anchors`: reads anchors for a document.
pub struct DocumentsAnchors;

#[async_trait::async_trait]
impl Command for DocumentsAnchors {
    const NAME: &'static str = "documents.anchors";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = AnchorsInput;
    type Output = AnchorsOutput;

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
        // Verify document exists.
        store::get(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::Domain(Problem::not_found()))?;

        let stmt = Statement::with_values(
            "SELECT field, page, x, y, w, h, xml_path FROM document_anchor \
             WHERE company_id = ? AND document_id = ? ORDER BY field"
                .to_owned(),
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        );
        let rows = db.query(&stmt).await?;
        let mut anchors = Vec::new();
        for row in rows.rows {
            let field: Option<String> = row.get("field");
            let page: Option<i64> = row.get("page");
            let x: Option<i64> = row.get("x");
            let y: Option<i64> = row.get("y");
            let w: Option<i64> = row.get("w");
            let h: Option<i64> = row.get("h");
            let xml_path: Option<String> = row.get("xml_path");

            if let Some(field) = field {
                let location =
                    if let (Some(p), Some(x), Some(y), Some(w), Some(h)) = (page, x, y, w, h) {
                        crate::Location::Page {
                            page: u32::try_from(p).unwrap_or(0),
                            bbox: crate::BoundingBox {
                                x: u32::try_from(x).unwrap_or(0),
                                y: u32::try_from(y).unwrap_or(0),
                                w: u32::try_from(w).unwrap_or(0),
                                h: u32::try_from(h).unwrap_or(0),
                            },
                        }
                    } else if let Some(path) = xml_path {
                        crate::Location::Xml { path }
                    } else {
                        continue; // Skip malformed rows
                    };
                anchors.push(Anchor { field, location });
            }
        }
        Ok(Plan::with_output(AnchorsOutput { anchors }))
    }
}

/// Output of `documents.delete`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeleteOutput {
    /// The SHA-256 of the deleted document's blob (for garbage collection).
    pub sha256: String,
    /// The MIME type of the deleted document (for re-storing the blob if concurrent upload sees it missing).
    pub mime: String,
}

/// `documents.delete`: human-only deletion, preventing deletion of linked documents.
pub struct DocumentsDelete;

#[async_trait::async_trait]
impl Command for DocumentsDelete {
    const NAME: &'static str = "documents.delete";
    const TIER: Tier = Tier::Draft;
    const IDEMPOTENT: bool = false;
    const TARGET: Option<&'static str> = Some("id");
    const HUMAN_ONLY: bool = true;

    type Input = GetInput; // Reuse id input.
    type Output = DeleteOutput;

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
            .ok_or_else(|| CommandError::Domain(Problem::not_found()))?;
        let document_sha = document.sha256.clone();
        let document_mime = document.mime.clone();

        // Pre-flight check: if document is linked, fail early with the specific problem.
        // The guard statement below is the actual enforcement (ADR 0004).
        let (condition, values) = linked_condition(&ctx.company_id, &input.id);
        let is_unlinked = db
            .query(&Statement::with_values(
                format!("SELECT CASE WHEN {condition} THEN 1 ELSE 0 END AS ok"),
                values,
            ))
            .await?
            .first()
            .and_then(|row| row.get::<i64>("ok"))
            .unwrap_or(0)
            > 0;

        if !is_unlinked {
            return Err(CommandError::Domain(document_linked_problem()));
        }

        let sha256 = document_sha;
        let guard_key = ledgers_db::guard::guard_key(&["document-unlinked", &input.id]);
        let mut statements = Vec::new();

        // Guard: fail if document is now linked (concurrent attachment).
        let (condition, values) = linked_condition(&ctx.company_id, &input.id);
        statements.push(ledgers_db::guard::assert_statement(
            &guard_key, condition, values,
        ));

        // Delete anchors, upload events, and the document.
        statements.push(Statement::with_values(
            "DELETE FROM document_anchor WHERE company_id = ? AND document_id = ?".to_owned(),
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));
        statements.push(Statement::with_values(
            "DELETE FROM document_upload_event WHERE company_id = ? AND document_id = ?".to_owned(),
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));
        statements.push(Statement::with_values(
            "DELETE FROM document WHERE company_id = ? AND id = ?".to_owned(),
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));

        // Release guard.
        statements.push(ledgers_db::guard::release_statement(&guard_key));

        let mut plan = Plan::with_output(DeleteOutput {
            sha256,
            mime: document_mime,
        });
        plan.statements = statements;
        Ok(plan)
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
