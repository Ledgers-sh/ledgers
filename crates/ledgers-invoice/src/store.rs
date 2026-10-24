//! Database access for invoices and credit notes.

use cratefield_core::{Database, Statement};
use sea_query::Value;

/// An invoice row from the database.
#[derive(Clone, Debug)]
pub struct InvoiceRow {
    pub id: String,
    pub company_id: String,
    pub customer_id: String,
    pub series: String,
    pub number: i64,
    pub fiscal_year: i64,
    pub issue_date: String,
    pub supply_date: String,
    pub due_date: Option<String>,
    pub currency: String,
    pub rate_scaled: Option<String>,
    pub vies_check_id: Option<String>,
    pub posted_entry_id: String,
    pub created_at: String,
}

/// An invoice line row from the database.
#[derive(Clone, Debug)]
pub struct InvoiceLineRow {
    pub id: String,
    pub invoice_id: String,
    pub company_id: String,
    pub sequence: i64,
    pub description: String,
    pub quantity_scaled: String,
    pub unit: Option<String>,
    pub unit_price_minor: Option<i64>,
    pub net_minor: i64,
    pub tax_code: String,
    pub revenue_account: String,
    pub created_at: String,
}

/// A credit note row from the database.
#[derive(Clone, Debug)]
pub struct CreditNoteRow {
    pub id: String,
    pub company_id: String,
    pub invoice_id: String,
    pub series: String,
    pub number: i64,
    pub fiscal_year: i64,
    pub issue_date: String,
    pub reason: Option<String>,
    pub posted_entry_id: String,
    pub created_at: String,
}

/// A credit note line row from the database.
#[derive(Clone, Debug)]
pub struct CreditNoteLineRow {
    pub id: String,
    pub credit_note_id: String,
    pub invoice_line_id: String,
    pub company_id: String,
    pub net_credited_minor: i64,
    pub created_at: String,
}

/// Load an invoice by ID.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn load_invoice(
    db: &dyn Database,
    company_id: &str,
    invoice_id: &str,
) -> Result<Option<InvoiceRow>, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT id, company_id, customer_id, series, number, fiscal_year, issue_date, \
         supply_date, due_date, currency, rate_scaled, vies_check_id, posted_entry_id, created_at \
         FROM invoice WHERE company_id = ? AND id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(invoice_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows.first().map(|row| InvoiceRow {
        id: row.get::<String>("id").unwrap_or_default(),
        company_id: row.get::<String>("company_id").unwrap_or_default(),
        customer_id: row.get::<String>("customer_id").unwrap_or_default(),
        series: row.get::<String>("series").unwrap_or_default(),
        number: row.get::<i64>("number").unwrap_or(0),
        fiscal_year: row.get::<i64>("fiscal_year").unwrap_or(0),
        issue_date: row.get::<String>("issue_date").unwrap_or_default(),
        supply_date: row.get::<String>("supply_date").unwrap_or_default(),
        due_date: row.get::<Option<String>>("due_date").flatten(),
        currency: row.get::<String>("currency").unwrap_or_default(),
        rate_scaled: row.get::<Option<String>>("rate_scaled").flatten(),
        vies_check_id: row.get::<Option<String>>("vies_check_id").flatten(),
        posted_entry_id: row.get::<String>("posted_entry_id").unwrap_or_default(),
        created_at: row.get::<String>("created_at").unwrap_or_default(),
    }))
}

/// Load invoice lines for an invoice.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn load_invoice_lines(
    db: &dyn Database,
    company_id: &str,
    invoice_id: &str,
) -> Result<Vec<InvoiceLineRow>, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT id, invoice_id, company_id, sequence, description, quantity_scaled, unit, \
         unit_price_minor, net_minor, tax_code, revenue_account, created_at \
         FROM invoice_line WHERE company_id = ? AND invoice_id = ? ORDER BY sequence"
            .to_owned(),
        vec![Value::from(company_id), Value::from(invoice_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .iter()
        .map(|row| InvoiceLineRow {
            id: row.get::<String>("id").unwrap_or_default(),
            invoice_id: row.get::<String>("invoice_id").unwrap_or_default(),
            company_id: row.get::<String>("company_id").unwrap_or_default(),
            sequence: row.get::<i64>("sequence").unwrap_or(0),
            description: row.get::<String>("description").unwrap_or_default(),
            quantity_scaled: row.get::<String>("quantity_scaled").unwrap_or_default(),
            unit: row.get::<Option<String>>("unit").flatten(),
            unit_price_minor: row.get::<i64>("unit_price_minor"),
            net_minor: row.get::<i64>("net_minor").unwrap_or(0),
            tax_code: row.get::<String>("tax_code").unwrap_or_default(),
            revenue_account: row.get::<String>("revenue_account").unwrap_or_default(),
            created_at: row.get::<String>("created_at").unwrap_or_default(),
        })
        .collect())
}

/// Load a credit note by ID.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn load_credit_note(
    db: &dyn Database,
    company_id: &str,
    credit_note_id: &str,
) -> Result<Option<CreditNoteRow>, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT id, company_id, invoice_id, series, number, fiscal_year, issue_date, reason, \
         posted_entry_id, created_at FROM credit_note WHERE company_id = ? AND id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(credit_note_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows.first().map(|row| CreditNoteRow {
        id: row.get::<String>("id").unwrap_or_default(),
        company_id: row.get::<String>("company_id").unwrap_or_default(),
        invoice_id: row.get::<String>("invoice_id").unwrap_or_default(),
        series: row.get::<String>("series").unwrap_or_default(),
        number: row.get::<i64>("number").unwrap_or(0),
        fiscal_year: row.get::<i64>("fiscal_year").unwrap_or(0),
        issue_date: row.get::<String>("issue_date").unwrap_or_default(),
        reason: row.get::<Option<String>>("reason").flatten(),
        posted_entry_id: row.get::<String>("posted_entry_id").unwrap_or_default(),
        created_at: row.get::<String>("created_at").unwrap_or_default(),
    }))
}

/// Load credit note lines for a credit note.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn load_credit_note_lines(
    db: &dyn Database,
    company_id: &str,
    credit_note_id: &str,
) -> Result<Vec<CreditNoteLineRow>, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT id, credit_note_id, invoice_line_id, company_id, net_credited_minor, created_at \
         FROM credit_note_line WHERE company_id = ? AND credit_note_id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(credit_note_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .iter()
        .map(|row| CreditNoteLineRow {
            id: row.get::<String>("id").unwrap_or_default(),
            credit_note_id: row.get::<String>("credit_note_id").unwrap_or_default(),
            invoice_line_id: row.get::<String>("invoice_line_id").unwrap_or_default(),
            company_id: row.get::<String>("company_id").unwrap_or_default(),
            net_credited_minor: row.get::<i64>("net_credited_minor").unwrap_or(0),
            created_at: row.get::<String>("created_at").unwrap_or_default(),
        })
        .collect())
}

/// Get the next invoice number for a series/fiscal year, provisioning the counter if needed.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn read_invoice_counter(
    db: &dyn Database,
    company_id: &str,
    series: &str,
    fiscal_year: i64,
) -> Result<i64, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT next_number FROM invoice_number_counter WHERE company_id = ? AND series = ? AND fiscal_year = ?"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(series),
            Value::from(fiscal_year),
        ],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .first()
        .and_then(|row| row.get::<i64>("next_number"))
        .unwrap_or(1))
}

/// Get the next credit note number for a series/fiscal year.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn read_credit_note_counter(
    db: &dyn Database,
    company_id: &str,
    series: &str,
    fiscal_year: i64,
) -> Result<i64, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT next_number FROM credit_note_number_counter WHERE company_id = ? AND series = ? AND fiscal_year = ?"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(series),
            Value::from(fiscal_year),
        ],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .first()
        .and_then(|row| row.get::<i64>("next_number"))
        .unwrap_or(1))
}

/// Get total net amount already credited for an invoice line.
///
/// # Errors
/// Returns [`cratefield_core::Problem`] on database errors.
pub async fn total_credited_for_line(
    db: &dyn Database,
    company_id: &str,
    invoice_line_id: &str,
) -> Result<i64, cratefield_core::Problem> {
    let stmt = Statement::with_values(
        "SELECT COALESCE(SUM(net_credited_minor), 0) as total FROM credit_note_line WHERE company_id = ? AND invoice_line_id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(invoice_line_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0))
}
