//! The reads the connections commands and the server make. Every query is
//! scoped by `company_id` (company isolation), and no query selects the sealed
//! credential except [`sealed_credential`], which only the server calls to
//! reach the vendor.

use cratefield_core::{Database, DbError, Row, Statement};
use sea_query::Value;

use crate::allocation::AllocationRule;

/// A connection as the books and the app see it. No credential field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRow {
    /// The connection id.
    pub id: String,
    /// The connector id.
    pub connector: String,
    /// The name the company gave it.
    pub name: String,
    /// The non-secret settings, as stored.
    pub config_json: String,
    /// The unit that receives unmapped resources.
    pub default_unit: String,
    /// Whether it is demo data.
    pub is_demo: bool,
    /// The last successful sync, RFC 3339.
    pub last_sync_at: Option<String>,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// Columns a connection read selects.
pub const CONNECTION_SELECT: &str = "SELECT id, connector, name, config_json, default_unit, is_demo, \
     last_sync_at, created_at, updated_at FROM connector_connection";

fn text(row: &Row, column: &str) -> Result<String, DbError> {
    row.get::<String>(column)
        .ok_or_else(|| DbError::Query(format!("row lacks {column}")))
}

fn connection_from_row(row: &Row) -> Result<ConnectionRow, DbError> {
    Ok(ConnectionRow {
        id: text(row, "id")?,
        connector: text(row, "connector")?,
        name: text(row, "name")?,
        config_json: text(row, "config_json")?,
        default_unit: text(row, "default_unit")?,
        is_demo: row.get::<i64>("is_demo").unwrap_or(0) == 1,
        last_sync_at: row.get::<Option<String>>("last_sync_at").flatten(),
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
    })
}

/// Every live connection of `company_id`, oldest first.
///
/// # Errors
/// A database error.
pub async fn connections(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<ConnectionRow>, DbError> {
    let stmt = Statement::with_values(
        format!(
            "{CONNECTION_SELECT} WHERE company_id = ? AND deleted_at IS NULL ORDER BY created_at, id"
        ),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(connection_from_row).collect()
}

/// One live connection of `company_id`.
///
/// # Errors
/// A database error.
pub async fn connection(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<ConnectionRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{CONNECTION_SELECT} WHERE company_id = ? AND id = ? AND deleted_at IS NULL"),
        vec![Value::from(company_id), Value::from(id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.first().map(connection_from_row).transpose()
}

/// The connection ids of `company_id` that hold a credential.
///
/// # Errors
/// A database error.
pub async fn connections_with_credential(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT connection_id FROM connector_credential WHERE company_id = ?".to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| row.get::<String>("connection_id"))
        .collect())
}

/// The sealed credential of a connection. Only the server calls this, to reach
/// the vendor; the value is never returned to a caller.
///
/// # Errors
/// A database error.
pub async fn sealed_credential(
    db: &dyn Database,
    company_id: &str,
    connection_id: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT sealed FROM connector_credential WHERE company_id = ? AND connection_id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(connection_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .first()
        .and_then(|row| row.get::<String>("sealed")))
}

/// The allocation rules of a connection, in order.
///
/// # Errors
/// A database error.
pub async fn rules(
    db: &dyn Database,
    company_id: &str,
    connection_id: &str,
) -> Result<Vec<AllocationRule>, DbError> {
    let stmt = Statement::with_values(
        "SELECT pattern, unit_code FROM allocation_rule WHERE company_id = ? AND connection_id = ? ORDER BY position, pattern"
            .to_owned(),
        vec![Value::from(company_id), Value::from(connection_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(AllocationRule {
                pattern: text(row, "pattern")?,
                unit_code: text(row, "unit_code")?,
            })
        })
        .collect()
}

/// One stored usage row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageRow {
    /// `YYYY-MM`.
    pub period: String,
    /// The resource kind.
    pub resource_kind: String,
    /// The vendor's resource id.
    pub resource_id: String,
    /// The resource name.
    pub resource_name: String,
    /// The weight.
    pub quantity: i64,
}

/// The stored usage of a connection, optionally for one period.
///
/// # Errors
/// A database error.
pub async fn usage(
    db: &dyn Database,
    company_id: &str,
    connection_id: &str,
    period: Option<&str>,
) -> Result<Vec<UsageRow>, DbError> {
    let (condition, mut values) = match period {
        Some(period) => (" AND period = ?", vec![Value::from(period)]),
        None => ("", Vec::new()),
    };
    let mut all = vec![Value::from(company_id), Value::from(connection_id)];
    all.append(&mut values);
    let stmt = Statement::with_values(
        format!(
            "SELECT period, resource_kind, resource_id, resource_name, quantity FROM connector_usage \
             WHERE company_id = ? AND connection_id = ?{condition} ORDER BY period, resource_kind, resource_id"
        ),
        all,
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(UsageRow {
                period: text(row, "period")?,
                resource_kind: text(row, "resource_kind")?,
                resource_id: text(row, "resource_id")?,
                resource_name: text(row, "resource_name")?,
                quantity: row.get::<i64>("quantity").unwrap_or(0),
            })
        })
        .collect()
}

/// An imported invoice, with what re-splitting needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvoiceRow {
    /// The invoice id (ULID).
    pub id: String,
    /// The vendor's stable id for it.
    pub external_id: String,
    /// `YYYY-MM`.
    pub period: String,
    /// The currency.
    pub currency: String,
    /// The gross total, minor units.
    pub total_minor: i64,
    /// The sanitised description.
    pub description: String,
    /// The draft bill, if one is linked.
    pub bill_id: Option<String>,
}

/// Every invoice imported from a connection.
///
/// # Errors
/// A database error.
pub async fn invoices(
    db: &dyn Database,
    company_id: &str,
    connection_id: &str,
) -> Result<Vec<InvoiceRow>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, external_id, period, currency, total_minor, description, bill_id FROM connector_invoice \
         WHERE company_id = ? AND connection_id = ? ORDER BY period, external_id"
            .to_owned(),
        vec![Value::from(company_id), Value::from(connection_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(InvoiceRow {
                id: text(row, "id")?,
                external_id: text(row, "external_id")?,
                period: text(row, "period")?,
                currency: text(row, "currency")?,
                total_minor: row.get::<i64>("total_minor").unwrap_or(0),
                description: text(row, "description")?,
                bill_id: row.get::<Option<String>>("bill_id").flatten(),
            })
        })
        .collect()
}

/// The sync history of a connection, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    /// The run id.
    pub id: String,
    /// The window start.
    pub window_since: String,
    /// The window end.
    pub window_until: String,
    /// `succeeded` or `failed`.
    pub status: String,
    /// Invoices the vendor returned in the window.
    pub invoices_found: i64,
    /// Draft bills created by this run.
    pub bills_created: i64,
    /// Usage rows the run stored.
    pub usage_rows: i64,
    /// The failure, when it failed.
    pub error_message: Option<String>,
    /// The notes, as stored (JSON array).
    pub notes_json: String,
    /// When the run finished.
    pub finished_at: String,
}

/// The sync history of a connection.
///
/// # Errors
/// A database error.
pub async fn runs(
    db: &dyn Database,
    company_id: &str,
    connection_id: &str,
    limit: u32,
) -> Result<Vec<RunRow>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, window_since, window_until, status, invoices_found, bills_created, usage_rows, error_message, notes_json, finished_at \
         FROM connector_run WHERE company_id = ? AND connection_id = ? ORDER BY finished_at DESC, id DESC LIMIT ?"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(connection_id),
            Value::from(i64::from(limit)),
        ],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(RunRow {
                id: text(row, "id")?,
                window_since: text(row, "window_since")?,
                window_until: text(row, "window_until")?,
                status: text(row, "status")?,
                invoices_found: row.get::<i64>("invoices_found").unwrap_or(0),
                bills_created: row.get::<i64>("bills_created").unwrap_or(0),
                usage_rows: row.get::<i64>("usage_rows").unwrap_or(0),
                error_message: row.get::<Option<String>>("error_message").flatten(),
                notes_json: text(row, "notes_json")?,
                finished_at: text(row, "finished_at")?,
            })
        })
        .collect()
}

/// The id of the live vendor named `name` (case-insensitive), if there is one.
///
/// # Errors
/// A database error.
pub async fn vendor_by_name(
    db: &dyn Database,
    company_id: &str,
    name: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id FROM vendor WHERE company_id = ? AND LOWER(name) = LOWER(?) AND merged_into IS NULL ORDER BY id LIMIT 1"
            .to_owned(),
        vec![Value::from(company_id), Value::from(name)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.rows.first().and_then(|row| row.get::<String>("id")))
}

/// Whether a bill is posted, or discarded, so its split may no longer change.
///
/// # Errors
/// A database error.
pub async fn bill_is_frozen(
    db: &dyn Database,
    company_id: &str,
    bill_id: &str,
) -> Result<bool, DbError> {
    let stmt = Statement::with_values(
        "SELECT posted_entry_id, discarded_at FROM bill WHERE company_id = ? AND id = ?".to_owned(),
        vec![Value::from(company_id), Value::from(bill_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.rows.first().is_none_or(|row| {
        row.get::<Option<String>>("posted_entry_id")
            .flatten()
            .is_some()
            || row
                .get::<Option<String>>("discarded_at")
                .flatten()
                .is_some()
    }))
}

/// The id of the document with `sha256`, if the company has one.
///
/// # Errors
/// A database error.
pub async fn document_by_sha(
    db: &dyn Database,
    company_id: &str,
    sha256: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id FROM document WHERE company_id = ? AND sha256 = ?".to_owned(),
        vec![Value::from(company_id), Value::from(sha256)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.rows.first().and_then(|row| row.get::<String>("id")))
}
