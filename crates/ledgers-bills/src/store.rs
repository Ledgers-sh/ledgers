//! The bill and vendor rows: the reads the commands make, and the typed
//! rows they are built from.

use crate::model::{BillLine, CheckView, VendorView};
use cratefield_core::{Database, DbError, Row, Statement};
use sea_query::Value;

/// A bill row with its vendor joined in.
#[derive(Clone, Debug)]
pub struct BillRow {
    /// The bill id.
    pub id: String,
    /// The document.
    pub document_id: String,
    /// The vendor, as the bill shows it.
    pub vendor: VendorView,
    /// The vendor id (same as `vendor.id`).
    pub vendor_id: String,
    /// The invoice number.
    pub invoice_number: Option<String>,
    /// The invoice date.
    pub invoice_date: Option<String>,
    /// The due date.
    pub due_date: Option<String>,
    /// The currency.
    pub currency: String,
    /// Net total.
    pub net_minor: Option<i64>,
    /// VAT total.
    pub vat_minor: Option<i64>,
    /// Gross total.
    pub gross_minor: Option<i64>,
    /// The lines.
    pub lines: Vec<BillLine>,
    /// Confidence per extracted field.
    pub confidence: std::collections::BTreeMap<String, u8>,
    /// The stored checks.
    pub checks: Vec<CheckView>,
    /// The stored instruction flags.
    pub instruction_flags: Vec<String>,
    /// Whether the invoice IBAN differs from the vendor's.
    pub iban_changed: bool,
    /// The extractor (or manual).
    pub source: String,
    /// The reader that produced the fields, when one did.
    pub reader: Option<String>,
    /// The business unit the bill's expense lines carry, when one is named.
    pub unit: Option<String>,
    /// The posted entry, once posted.
    pub posted_entry_id: Option<String>,
    /// When a person discarded the draft.
    pub discarded_at: Option<String>,
    /// Who created it.
    pub created_by: String,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// The columns a bill read selects, with the vendor joined.
pub const BILL_SELECT: &str = "SELECT b.id, b.document_id, b.vendor_id, b.invoice_number, b.invoice_date, \
     b.due_date, b.currency, b.net_minor, b.vat_minor, b.gross_minor, b.lines_json, b.confidence_json, b.checks_json, \
     b.instruction_flags, b.iban_changed, b.source, b.reader, b.unit_code, b.posted_entry_id, b.discarded_at, b.created_by, \
     b.created_at, b.updated_at, v.name AS vendor_name, v.vat_id AS vendor_vat_id, v.iban AS vendor_iban, \
     v.needs_review AS vendor_needs_review FROM bill b JOIN vendor v ON v.id = b.vendor_id";

fn text(row: &Row, column: &str) -> Result<String, DbError> {
    row.get::<String>(column)
        .ok_or_else(|| DbError::Query(format!("bill row lacks {column}")))
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row, column: &str) -> Result<T, DbError> {
    serde_json::from_str(&text(row, column)?)
        .map_err(|err| DbError::Query(format!("{column}: {err}")))
}

fn from_row(row: &Row) -> Result<BillRow, DbError> {
    let vendor_id = text(row, "vendor_id")?;
    Ok(BillRow {
        id: text(row, "id")?,
        document_id: text(row, "document_id")?,
        vendor: VendorView {
            id: vendor_id.clone(),
            name: text(row, "vendor_name")?,
            vat_id: row.get::<Option<String>>("vendor_vat_id").flatten(),
            iban: row.get::<Option<String>>("vendor_iban").flatten(),
            needs_review: row.get::<i64>("vendor_needs_review").unwrap_or(0) == 1,
        },
        vendor_id,
        invoice_number: row.get::<Option<String>>("invoice_number").flatten(),
        invoice_date: row.get::<Option<String>>("invoice_date").flatten(),
        due_date: row.get::<Option<String>>("due_date").flatten(),
        currency: text(row, "currency")?,
        net_minor: row.get::<i64>("net_minor"),
        vat_minor: row.get::<i64>("vat_minor"),
        gross_minor: row.get::<i64>("gross_minor"),
        lines: json_column(row, "lines_json")?,
        confidence: json_column(row, "confidence_json")?,
        checks: json_column(row, "checks_json")?,
        instruction_flags: json_column(row, "instruction_flags")?,
        iban_changed: row.get::<i64>("iban_changed").unwrap_or(0) == 1,
        source: text(row, "source")?,
        reader: row.get::<Option<String>>("reader").flatten(),
        unit: row.get::<Option<String>>("unit_code").flatten(),
        posted_entry_id: row.get::<Option<String>>("posted_entry_id").flatten(),
        discarded_at: row.get::<Option<String>>("discarded_at").flatten(),
        created_by: text(row, "created_by")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
    })
}

/// One bill in `company_id`, if it exists.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn bill_by_id(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<BillRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{BILL_SELECT} WHERE b.company_id = ? AND b.id = ?"),
        vec![Value::from(company_id), Value::from(id)],
    );
    let rows = db.query(&stmt).await?;
    rows.first().map(from_row).transpose()
}

/// Recent bills in `company_id`, newest first.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn list_bills(
    db: &dyn Database,
    company_id: &str,
    limit: u32,
    unit: Option<&str>,
) -> Result<Vec<BillRow>, DbError> {
    // A unit scope keeps the bills that carry that unit; consolidated keeps all.
    let (condition, mut values) = match unit {
        Some(code) => (" AND b.unit_code = ?", vec![Value::from(code)]),
        None => ("", Vec::new()),
    };
    let mut all = vec![Value::from(company_id)];
    all.append(&mut values);
    all.push(Value::from(i64::from(limit)));
    let stmt = Statement::with_values(
        format!(
            "{BILL_SELECT} WHERE b.company_id = ?{condition} ORDER BY b.created_at DESC, b.id DESC LIMIT ?"
        ),
        all,
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(from_row).collect()
}

/// The id of the bill for `document_id`, if there is one.
///
/// # Errors
/// A database error.
pub async fn bill_for_document(
    db: &dyn Database,
    company_id: &str,
    document_id: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id FROM bill WHERE company_id = ? AND document_id = ?".to_owned(),
        vec![Value::from(company_id), Value::from(document_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.first().and_then(|row| row.get::<String>("id")))
}

/// The id of the bill from `vendor_id` with `invoice_number`, if there is one.
///
/// # Errors
/// A database error.
pub async fn bill_for_invoice(
    db: &dyn Database,
    company_id: &str,
    vendor_id: &str,
    invoice_number: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id FROM bill WHERE company_id = ? AND vendor_id = ? AND invoice_number = ?"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(vendor_id),
            Value::from(invoice_number),
        ],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.first().and_then(|row| row.get::<String>("id")))
}

/// Every vendor in `company_id`.
///
/// # Errors
/// A database error.
pub async fn vendors(db: &dyn Database, company_id: &str) -> Result<Vec<VendorView>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, name, vat_id, iban, needs_review FROM vendor WHERE company_id = ? ORDER BY name, id".to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(vendor_from_row).collect()
}

fn vendor_from_row(row: &Row) -> Result<VendorView, DbError> {
    Ok(VendorView {
        id: text(row, "id")?,
        name: text(row, "name")?,
        vat_id: row.get::<Option<String>>("vat_id").flatten(),
        iban: row.get::<Option<String>>("iban").flatten(),
        needs_review: row.get::<i64>("needs_review").unwrap_or(0) == 1,
    })
}

/// The lines of the most recent bill of `vendor_id` that name an expense
/// account: the learned suggestion's candidates, newest first, with the bill
/// each came from. The caller filters them against the chart.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn vendor_history(
    db: &dyn Database,
    company_id: &str,
    vendor_id: &str,
) -> Result<Vec<(String, String)>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, lines_json FROM bill WHERE company_id = ? AND vendor_id = ? \
         ORDER BY created_at DESC, id DESC LIMIT 20"
            .to_owned(),
        vec![Value::from(company_id), Value::from(vendor_id)],
    );
    let rows = db.query(&stmt).await?;
    let mut out = Vec::new();
    for row in &rows.rows {
        let bill_id = text(row, "id")?;
        let lines: Vec<BillLine> = json_column(row, "lines_json")?;
        for line in lines {
            if let Some(account) = line.expense_account {
                out.push((account, bill_id.clone()));
            }
        }
    }
    Ok(out)
}
