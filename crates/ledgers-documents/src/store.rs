//! The document rows: the statements that read and write `document` and
//! `document_upload_event`, and the typed view commands return.

use cratefield_core::{Database, DbError, Statement};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// How long the original must be kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RetentionClass {
    /// Ten years (the default; the German `GoBD` period for books and vouchers).
    #[serde(rename = "ten-years")]
    TenYears,
    /// Six years.
    #[serde(rename = "six-years")]
    SixYears,
    /// Kept until someone removes it by hand.
    #[serde(rename = "indefinite")]
    Indefinite,
}

impl RetentionClass {
    /// The stored form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RetentionClass::TenYears => "ten-years",
            RetentionClass::SixYears => "six-years",
            RetentionClass::Indefinite => "indefinite",
        }
    }

    /// Parses the stored form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ten-years" => Some(Self::TenYears),
            "six-years" => Some(Self::SixYears),
            "indefinite" => Some(Self::Indefinite),
            _ => None,
        }
    }
}

/// A stored document, as commands and the app see it. The PDF text layer is
/// not part of the view: it is read by the policy screen, not returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    /// The document id.
    pub id: String,
    /// The SHA-256 of the file bytes, lowercase hex.
    pub sha256: String,
    /// The size in bytes.
    pub size_bytes: i64,
    /// The sniffed MIME type.
    pub mime: String,
    /// The file name as uploaded (display only; never trusted for type).
    pub original_name: String,
    /// Who uploaded the first copy.
    pub uploaded_by: String,
    /// The retention class.
    pub retention_class: RetentionClass,
    /// When the document was first stored (ISO 8601, UTC).
    pub created_at: String,
    /// Whether the PDF text layer was read (false for images and scans).
    pub has_text_layer: bool,
}

/// The columns a document read selects, in the order [`from_row`] expects.
pub const DOCUMENT_COLUMNS: &str = "id, sha256, size_bytes, mime, original_name, uploaded_by, \
     retention_class, created_at, CASE WHEN text_layer IS NULL THEN 0 ELSE 1 END AS has_text";

/// Reads a [`Document`] from a row selected with [`DOCUMENT_COLUMNS`].
///
/// # Errors
/// [`DbError::Query`] when a column is missing or the retention class is unknown.
pub fn from_row(row: &cratefield_core::Row) -> Result<Document, DbError> {
    let text = |column: &str| -> Result<String, DbError> {
        row.get::<String>(column)
            .ok_or_else(|| DbError::Query(format!("document row lacks {column}")))
    };
    let retention = text("retention_class")?;
    Ok(Document {
        id: text("id")?,
        sha256: text("sha256")?,
        size_bytes: row
            .get::<i64>("size_bytes")
            .ok_or_else(|| DbError::Query("document row lacks size_bytes".into()))?,
        mime: text("mime")?,
        original_name: text("original_name")?,
        uploaded_by: text("uploaded_by")?,
        retention_class: RetentionClass::parse(&retention)
            .ok_or_else(|| DbError::Query(format!("unknown retention class {retention}")))?,
        created_at: text("created_at")?,
        has_text_layer: row.get::<i64>("has_text").unwrap_or(0) == 1,
    })
}

/// Reads the document with `sha256` in `company_id`, if any.
///
/// # Errors
/// A database error.
pub async fn find_by_hash(
    db: &dyn Database,
    company_id: &str,
    sha256: &str,
) -> Result<Option<Document>, DbError> {
    let stmt = Statement::with_values(
        format!("SELECT {DOCUMENT_COLUMNS} FROM document WHERE company_id = ? AND sha256 = ?"),
        vec![Value::from(company_id), Value::from(sha256)],
    );
    let rows = db.query(&stmt).await?;
    rows.first().map(from_row).transpose()
}

/// Reads one document by id in `company_id`.
///
/// # Errors
/// A database error.
pub async fn get(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<Document>, DbError> {
    let stmt = Statement::with_values(
        format!("SELECT {DOCUMENT_COLUMNS} FROM document WHERE company_id = ? AND id = ?"),
        vec![Value::from(company_id), Value::from(id)],
    );
    let rows = db.query(&stmt).await?;
    rows.first().map(from_row).transpose()
}

/// The PDF text layer of a document, if one was read.
///
/// # Errors
/// A database error.
pub async fn text_layer(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT text_layer FROM document WHERE company_id = ? AND id = ?".to_owned(),
        vec![Value::from(company_id), Value::from(id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.first().and_then(|row| row.get::<String>("text_layer")))
}

/// Lists documents in `company_id`, newest first.
///
/// # Errors
/// A database error.
pub async fn list(
    db: &dyn Database,
    company_id: &str,
    limit: u32,
) -> Result<Vec<Document>, DbError> {
    let stmt = Statement::with_values(
        format!(
            "SELECT {DOCUMENT_COLUMNS} FROM document WHERE company_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ),
        vec![Value::from(company_id), Value::from(i64::from(limit))],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(from_row).collect()
}
