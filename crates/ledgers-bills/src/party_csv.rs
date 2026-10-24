//! The party directory's CSV round trip: `parties.export` writes every party
//! as CSV, and `parties.import_check` / `parties.import` read a file back.
//!
//! A file is the whole state of the rows it names. A row whose `id` is a party
//! of the company updates that party; any other row creates one (a blank `id`
//! is filled by the server). Blank cells clear their field on an update, and a
//! column left out of the file leaves its field alone. Re-importing an export
//! changes nothing.
//!
//! Bank details are human only. A row with an `iban` or `bic` needs a person
//! to make it (an agent key is refused with `party-bank-needs-person`) and a
//! `bank_reason`, which lands in the same history as `parties.set_bank`.
//!
//! `parties.import_check` runs the same plan without writing and reports each
//! row. `parties.import` writes only when every row is valid; a file with any
//! error is refused whole. Logos are not part of the file: they are uploaded.

use crate::iban;
use crate::parties::{
    Address, Contact, CreateInput, PartyView, SetBankInput, UpdateInput, bic_checked, create_parts,
    party_from_row, read_party_row, set_bank_parts, unmerged, update_parts,
};
use crate::party_fields::{
    self as pf, BANK_NEEDS_PERSON, BANK_REASON_REQUIRED, CSV_INVALID, IMPORT_INVALID,
    IMPORT_TOO_MANY, MAX_EXPORT_ROWS, MAX_IMPORT_ROWS, MERGED, TaxId, VALUE_INVALID, VAT_TAKEN,
    refuse,
};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_tax::normalize_vat_id;
use schemars::JsonSchema;
use sea_query::Value;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Every column a party file may have, in export order.
pub const COLUMNS: [&str; 25] = [
    "id",
    "name",
    "trading_name",
    "roles",
    "vat_id",
    "tax_ids",
    "tax_number",
    "registration_number",
    "website",
    "email",
    "phone",
    "currency",
    "language",
    "payment_terms_days",
    "default_account",
    "default_tax_code",
    "default_unit",
    "tags",
    "notes",
    "personal_data",
    "addresses",
    "contacts",
    "iban",
    "bic",
    "bank_reason",
];

// ===================================================================== CSV text

/// Splits CSV text into rows of cells (RFC 4180: quotes, doubled quotes,
/// commas and line breaks inside quotes). Blank lines are dropped.
fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, CommandError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if in_quotes {
        return Err(refuse(&CSV_INVALID, "csv", "a quoted cell is not closed"));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    Ok(rows)
}

/// Writes one CSV cell, quoted when it needs quotes.
fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// The parsed file: its header and its data rows, each the width of the header.
struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn table_of(text: &str) -> Result<Table, CommandError> {
    let mut rows = parse_csv(text)?;
    if rows.is_empty() {
        return Err(refuse(&CSV_INVALID, "csv", "the file has no header row"));
    }
    let header: Vec<String> = rows
        .remove(0)
        .into_iter()
        .map(|h| h.trim().to_string())
        .collect();
    let mut seen = HashSet::new();
    for column in &header {
        if !COLUMNS.contains(&column.as_str()) {
            return Err(refuse(
                &CSV_INVALID,
                "csv",
                format!("unknown column {column:?}"),
            ));
        }
        if !seen.insert(column.as_str()) {
            return Err(refuse(
                &CSV_INVALID,
                "csv",
                format!("column {column:?} is listed twice"),
            ));
        }
    }
    if !header.iter().any(|h| h == "name") {
        return Err(refuse(&CSV_INVALID, "csv", "the file needs a name column"));
    }
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(refuse(
            &IMPORT_TOO_MANY,
            "csv",
            format!(
                "the file has {} rows; at most {MAX_IMPORT_ROWS} are imported at once",
                rows.len()
            ),
        ));
    }
    for (index, row) in rows.iter().enumerate() {
        if row.len() != header.len() {
            return Err(refuse(
                &CSV_INVALID,
                "csv",
                format!(
                    "line {} has {} cells; the header has {}",
                    index + 2,
                    row.len(),
                    header.len()
                ),
            ));
        }
    }
    Ok(Table { header, rows })
}

// ===================================================================== reading rows

/// One row's cells by column name. A column the file does not have reads as absent.
struct RowCells<'a> {
    header: &'a [String],
    cells: &'a [String],
}

impl RowCells<'_> {
    /// The trimmed cell, or `None` when the file has no such column.
    fn get(&self, column: &str) -> Option<&str> {
        self.header
            .iter()
            .position(|h| h == column)
            .map(|index| self.cells[index].trim())
    }
}

/// The row's values after parsing. `None` means the file did not name the column.
#[derive(Default)]
struct ParsedRow {
    name: Option<String>,
    trading_name: Option<String>,
    roles: Option<String>,
    vat_id: Option<String>,
    tax_ids: Option<Vec<TaxId>>,
    tax_number: Option<String>,
    registration_number: Option<String>,
    website: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    currency: Option<String>,
    language: Option<String>,
    payment_terms_days: Option<i32>,
    default_account: Option<String>,
    default_tax_code: Option<String>,
    default_unit: Option<String>,
    tags: Option<Vec<String>>,
    notes: Option<String>,
    personal_data: Option<bool>,
    addresses: Option<Vec<Address>>,
    contacts: Option<Vec<Contact>>,
    iban: Option<String>,
    bic: Option<String>,
    bank_reason: Option<String>,
}

fn value_refusal(field: &str, what: &str) -> CommandError {
    refuse(&VALUE_INVALID, field, format!("{field} must be {what}"))
}

fn json_cell<T: DeserializeOwned>(
    cell: &str,
    field: &str,
    def: &'static cratefield_core::ProblemDef,
) -> Result<Vec<T>, CommandError> {
    if cell.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<T>>(cell)
        .map_err(|_| refuse(def, field, format!("{field} is not a valid JSON list")))
}

fn parse_row(cells: &RowCells<'_>) -> Result<ParsedRow, CommandError> {
    let text = |column: &str| cells.get(column).map(str::to_string);
    let mut row = ParsedRow {
        name: text("name"),
        trading_name: text("trading_name"),
        roles: cells
            .get("roles")
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        vat_id: text("vat_id"),
        tax_number: text("tax_number"),
        registration_number: text("registration_number"),
        website: text("website"),
        email: text("email"),
        phone: text("phone"),
        currency: text("currency"),
        language: text("language"),
        default_account: text("default_account"),
        default_tax_code: text("default_tax_code"),
        default_unit: text("default_unit"),
        notes: text("notes"),
        iban: text("iban"),
        bic: text("bic"),
        bank_reason: text("bank_reason"),
        ..ParsedRow::default()
    };
    if let Some(v) = cells.get("payment_terms_days") {
        row.payment_terms_days = if v.is_empty() {
            None
        } else {
            Some(
                v.parse::<i32>()
                    .map_err(|_| value_refusal("payment_terms_days", "a whole number of days"))?,
            )
        };
    }
    if let Some(v) = cells.get("personal_data") {
        row.personal_data = match v.to_ascii_lowercase().as_str() {
            "" => None,
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => return Err(value_refusal("personal_data", "true or false")),
        };
    }
    if let Some(v) = cells.get("tags") {
        row.tags = Some(
            v.split(';')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
        );
    }
    if let Some(v) = cells.get("tax_ids") {
        row.tax_ids = Some(json_cell(v, "tax_ids", &pf::TAX_ID_INVALID)?);
    }
    if let Some(v) = cells.get("addresses") {
        row.addresses = Some(json_cell(v, "addresses", &pf::ADDRESS_INVALID)?);
    }
    if let Some(v) = cells.get("contacts") {
        row.contacts = Some(json_cell(v, "contacts", &pf::CONTACT_INVALID)?);
    }
    Ok(row)
}

impl ParsedRow {
    fn create_input(&self, id: &str) -> Result<CreateInput, CommandError> {
        let name = self
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| refuse(&pf::NAME_REQUIRED, "name", "the party needs a name"))?;
        Ok(CreateInput {
            id: Some(id.to_string()),
            name,
            trading_name: self.trading_name.clone(),
            vat_id: self.vat_id.clone().filter(|v| !v.is_empty()),
            tax_ids: self.tax_ids.clone().unwrap_or_default(),
            roles: self.roles.clone(),
            tax_number: self.tax_number.clone().filter(|v| !v.is_empty()),
            registration_number: self.registration_number.clone().filter(|v| !v.is_empty()),
            website: self.website.clone().filter(|v| !v.is_empty()),
            email: self.email.clone().filter(|v| !v.is_empty()),
            phone: self.phone.clone().filter(|v| !v.is_empty()),
            language: self.language.clone().filter(|v| !v.is_empty()),
            currency: self.currency.clone().filter(|v| !v.is_empty()),
            personal_data: self.personal_data,
            payment_terms_days: self.payment_terms_days,
            default_account: self.default_account.clone().filter(|v| !v.is_empty()),
            default_tax_code: self.default_tax_code.clone().filter(|v| !v.is_empty()),
            default_unit: self.default_unit.clone().filter(|v| !v.is_empty()),
            tags: self.tags.clone().unwrap_or_default(),
            notes: self.notes.clone().filter(|v| !v.is_empty()),
            addresses: self.addresses.clone().unwrap_or_default(),
            contacts: self.contacts.clone().unwrap_or_default(),
        })
    }

    fn update_input(&self, id: &str) -> Result<UpdateInput, CommandError> {
        if self.name.as_deref() == Some("") {
            return Err(refuse(&pf::NAME_REQUIRED, "name", "the party needs a name"));
        }
        Ok(UpdateInput {
            id: id.to_string(),
            name: self.name.clone(),
            trading_name: self.trading_name.clone(),
            vat_id: self.vat_id.clone(),
            tax_ids: self.tax_ids.clone(),
            roles: self.roles.clone(),
            tax_number: self.tax_number.clone(),
            registration_number: self.registration_number.clone(),
            website: self.website.clone(),
            email: self.email.clone(),
            phone: self.phone.clone(),
            language: self.language.clone(),
            currency: self.currency.clone(),
            personal_data: self.personal_data,
            payment_terms_days: self.payment_terms_days,
            default_account: self.default_account.clone(),
            default_tax_code: self.default_tax_code.clone(),
            default_unit: self.default_unit.clone(),
            tags: self.tags.clone(),
            notes: self.notes.clone(),
            addresses: self.addresses.clone(),
            contacts: self.contacts.clone(),
        })
    }
}

// ===================================================================== the plan

/// One row's outcome: what it does, and its errors.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RowReport {
    /// The line in the file (the header is line 1).
    pub line: u32,
    /// `create`, `update` or `error`.
    pub action: String,
    /// The party id the row names (or the server fills for a create).
    pub id: String,
    /// The refusals for this row; empty when the row is valid.
    pub errors: Vec<RowError>,
}

/// A refused row: the problem's slug, the field when one is named, and the message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RowError {
    /// The problem type slug (`party-vat-invalid`, ...).
    pub slug: String,
    /// The field the refusal is about, or empty for the row as a whole.
    pub field: String,
    /// The message, in the server's words.
    pub message: String,
}

/// The plan of a whole file: every row's report, and the statements of the
/// valid rows (which are written only when no row has an error).
#[derive(Default)]
struct ImportPlan {
    rows: Vec<RowReport>,
    statements: Vec<Statement>,
    objects: Vec<String>,
    ids: Vec<String>,
    created: u32,
    updated: u32,
}

impl ImportPlan {
    fn errors(&self) -> u32 {
        u32::try_from(self.rows.iter().filter(|r| !r.errors.is_empty()).count()).unwrap_or(u32::MAX)
    }
}

fn row_error_of(error: &CommandError) -> RowError {
    match error {
        CommandError::Domain(problem) => RowError {
            slug: problem.slug.to_string(),
            field: String::new(),
            message: problem
                .detail
                .clone()
                .unwrap_or_else(|| problem.title.to_string()),
        },
        other => RowError {
            slug: "invalid-input".to_string(),
            field: String::new(),
            message: other.to_string(),
        },
    }
}

fn field_error(
    def: &'static cratefield_core::ProblemDef,
    field: &str,
    message: impl Into<String>,
) -> RowError {
    RowError {
        slug: def.slug.to_string(),
        field: field.to_string(),
        message: message.into(),
    }
}

/// The ids and VAT IDs the rows of one file have already named.
#[derive(Default)]
struct Seen {
    ids: HashSet<String>,
    vats: HashSet<String>,
}

/// What a valid row writes: its action, its VAT ID, and its statements.
struct Written {
    action: &'static str,
    vat: Option<String>,
    statements: Vec<Statement>,
    objects: Vec<String>,
}

/// One row's plan: its report, and what it writes when it is valid.
struct RowOutcome {
    report: RowReport,
    written: Option<Written>,
}

/// The bank change a row asks for, when it changes the bank details. Bank
/// details need a person and a reason; a row that keeps them returns `None`.
fn bank_change(
    parsed: &ParsedRow,
    existing: Option<&PartyView>,
    id: &str,
    person: Option<&str>,
) -> Result<Option<SetBankInput>, CommandError> {
    let given = |v: &Option<String>| v.as_deref().is_some_and(|v| !v.is_empty());
    if !given(&parsed.iban) && !given(&parsed.bic) {
        return Ok(None);
    }
    if person.is_none() {
        return Err(refuse(
            &BANK_NEEDS_PERSON,
            "iban",
            "bank details change only by a person, never by an agent key",
        ));
    }
    let iban_text = parsed.iban.clone().unwrap_or_default();
    if iban_text.is_empty() {
        return Err(refuse(
            &pf::IBAN_INVALID,
            "iban",
            "a BIC needs an IBAN with it",
        ));
    }
    let iban = iban::validate(&iban_text)
        .map_err(|err| refuse(&pf::IBAN_INVALID, "iban", err.to_string()))?;
    let bic = match parsed.bic.as_deref().filter(|b| !b.is_empty()) {
        Some(b) => Some(bic_checked(b)?),
        None => None,
    };
    let changed = match existing {
        Some(current) => {
            current.iban.as_deref().map(iban::normalise).as_deref() != Some(iban.as_str())
                || current.bic != bic
        }
        None => true,
    };
    if !changed {
        return Ok(None);
    }
    let reason = parsed.bank_reason.clone().unwrap_or_default();
    if reason.trim().is_empty() {
        return Err(refuse(
            &BANK_REASON_REQUIRED,
            "bank_reason",
            "a bank change needs a reason, kept in the history",
        ));
    }
    Ok(Some(SetBankInput {
        id: id.to_string(),
        iban,
        bic,
        reason,
    }))
}

/// The history row and the bank columns of a party a file creates with bank
/// details: the same record `parties.set_bank` writes, with no earlier values.
fn create_bank_statements(
    ctx: &CommandContext,
    index: usize,
    id: &str,
    bank: &SetBankInput,
    changed_by: &str,
) -> Vec<Statement> {
    let bic = bank.bic.as_deref().map_or(Value::String(None), Value::from);
    vec![
        Statement::with_values(
            "INSERT INTO vendor_bank_change (id, company_id, vendor_id, before_iban, before_bic, after_iban, after_bic, changed_by, changed_at, reason, created_at) VALUES (?, ?, ?, NULL, NULL, ?, ?, ?, ?, ?, ?)".to_string(),
            vec![
                Value::from(format!("change_{}_{}", ctx.event_id, index + 1).as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(id),
                Value::from(bank.iban.as_str()),
                bic.clone(),
                Value::from(changed_by),
                Value::from(ctx.now.as_str()),
                Value::from(bank.reason.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        ),
        Statement::with_values(
            "UPDATE vendor SET iban = ?, bic = ?, updated_at = ? WHERE company_id = ? AND id = ?"
                .to_string(),
            vec![
                Value::from(bank.iban.as_str()),
                bic,
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(id),
            ],
        ),
    ]
}

/// Writes one valid row's plan: an update or a create, plus its bank change.
async fn write_row(
    db: &dyn Database,
    ctx: &CommandContext,
    index: usize,
    id: &str,
    parsed: &ParsedRow,
    existing: Option<&PartyView>,
    person: Option<&str>,
) -> Result<Written, CommandError> {
    let bank = bank_change(parsed, existing, id, person)?;
    let changed_by = person.unwrap_or_default();
    if existing.is_some() {
        let input = parsed.update_input(id)?;
        let (party, mut statements, mut objects) = update_parts(db, ctx, &input).await?;
        if let Some(bank_input) = &bank {
            let (_, bank_statements, bank_objects) =
                set_bank_parts(db, ctx, changed_by, bank_input).await?;
            statements.extend(bank_statements);
            objects.extend(bank_objects);
        }
        return Ok(Written {
            action: "update",
            vat: party.vat_id,
            statements,
            objects,
        });
    }
    let input = parsed.create_input(id)?;
    let (party, create_statement, objects) = create_parts(db, ctx, &input).await?;
    let mut statements = vec![create_statement];
    if let Some(bank_input) = &bank {
        statements.extend(create_bank_statements(
            ctx, index, id, bank_input, changed_by,
        ));
    }
    Ok(Written {
        action: "create",
        vat: party.vat_id,
        statements,
        objects,
    })
}

/// Plans one row: its id, its parsed fields, the bank rule, and the write.
async fn plan_row(
    db: &dyn Database,
    ctx: &CommandContext,
    index: usize,
    cells: &RowCells<'_>,
    person: Option<&str>,
    seen: &mut Seen,
) -> Result<RowOutcome, CommandError> {
    let line = u32::try_from(index + 2).unwrap_or(u32::MAX);
    let given_id = cells
        .get("id")
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let id = given_id
        .clone()
        .unwrap_or_else(|| format!("party_csv_{}_{}", ctx.event_id, index + 1));
    let mut errors: Vec<RowError> = Vec::new();
    if given_id.is_some() && !seen.ids.insert(id.clone()) {
        errors.push(field_error(
            &pf::NAME_REQUIRED,
            "id",
            format!("party {id} is listed twice"),
        ));
    }
    let existing = match &given_id {
        Some(known) => match read_party_row(db, &ctx.company_id, known).await? {
            Some(row) => Some(party_from_row(&row)?),
            None => None,
        },
        None => None,
    };
    if existing.as_ref().is_some_and(|p| p.merged_into.is_some()) {
        errors.push(field_error(
            &MERGED,
            "id",
            "party is merged and cannot be changed",
        ));
    }
    let parsed = match parse_row(cells) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            errors.push(row_error_of(&error));
            None
        }
    };
    let mut written = None;
    if let Some(parsed) = parsed.as_ref().filter(|_| errors.is_empty()) {
        match write_row(db, ctx, index, &id, parsed, existing.as_ref(), person).await {
            Ok(done) => written = Some(done),
            Err(error) => errors.push(row_error_of(&error)),
        }
    }
    if let Some(done) = &written
        && errors.is_empty()
        && let Some(vat) = done.vat.as_deref().map(normalize_vat_id)
        && !vat.is_empty()
        && !seen.vats.insert(vat.clone())
    {
        errors.push(field_error(
            &VAT_TAKEN,
            "vat_id",
            format!("another row of this file already has VAT ID {vat}"),
        ));
    }
    if !errors.is_empty() {
        written = None;
    }
    let action = written.as_ref().map_or("error", |done| done.action);
    Ok(RowOutcome {
        report: RowReport {
            line,
            action: action.to_string(),
            id,
            errors,
        },
        written,
    })
}

/// Plans a file: each row becomes a create or an update, or a row error.
/// `person` is the acting person, required for any bank detail.
async fn plan_import(
    db: &dyn Database,
    ctx: &CommandContext,
    table: &Table,
    person: Option<&str>,
) -> Result<ImportPlan, CommandError> {
    let mut plan = ImportPlan::default();
    let mut seen = Seen::default();
    for (index, cells) in table.rows.iter().enumerate() {
        let row = RowCells {
            header: &table.header,
            cells,
        };
        let outcome = plan_row(db, ctx, index, &row, person, &mut seen).await?;
        if let Some(done) = outcome.written {
            match done.action {
                "create" => plan.created += 1,
                _ => plan.updated += 1,
            }
            plan.statements.extend(done.statements);
            plan.objects.extend(done.objects);
            plan.ids.push(outcome.report.id.clone());
        }
        plan.rows.push(outcome.report);
    }
    Ok(plan)
}

// ===================================================================== export

/// The CSV text of the company's parties, in name order, with the columns of
/// [`COLUMNS`]. Importing it back changes nothing.
#[must_use]
pub fn party_csv(parties: &[PartyView]) -> String {
    let mut out = String::new();
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for party in parties {
        let json_or_empty = |empty: bool, text: Result<String, serde_json::Error>| {
            if empty {
                String::new()
            } else {
                text.unwrap_or_default()
            }
        };
        let cells: Vec<String> = vec![
            party.id.clone(),
            party.name.clone(),
            party.trading_name.clone().unwrap_or_default(),
            party.roles.clone(),
            party.vat_id.clone().unwrap_or_default(),
            json_or_empty(
                party.tax_ids.is_empty(),
                serde_json::to_string(&party.tax_ids),
            ),
            party.tax_number.clone().unwrap_or_default(),
            party.registration_number.clone().unwrap_or_default(),
            party.website.clone().unwrap_or_default(),
            party.email.clone().unwrap_or_default(),
            party.phone.clone().unwrap_or_default(),
            party.currency.clone().unwrap_or_default(),
            party.language.clone().unwrap_or_default(),
            party
                .payment_terms_days
                .map(|d| d.to_string())
                .unwrap_or_default(),
            party.default_account.clone().unwrap_or_default(),
            party.default_tax_code.clone().unwrap_or_default(),
            party.default_unit.clone().unwrap_or_default(),
            party.tags.join(";"),
            party.notes.clone().unwrap_or_default(),
            if party.personal_data { "true" } else { "false" }.to_string(),
            json_or_empty(
                party.addresses.is_empty(),
                serde_json::to_string(&party.addresses),
            ),
            json_or_empty(
                party.contacts.is_empty(),
                serde_json::to_string(&party.contacts),
            ),
            party.iban.clone().unwrap_or_default(),
            party.bic.clone().unwrap_or_default(),
            String::new(),
        ];
        let line: Vec<String> = cells.iter().map(|c| csv_cell(c)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

// ===================================================================== commands

/// The output of `parties.import_check` and the report of `parties.import`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ImportCheckOutput {
    /// True when every row is valid, so the file would import.
    pub valid: bool,
    /// Rows that would create a party.
    pub creates: u32,
    /// Rows that would update a party.
    pub updates: u32,
    /// Rows with at least one refusal.
    pub errors: u32,
    /// Every row's report, in file order.
    pub rows: Vec<RowReport>,
}

/// The input of `parties.import_check` and `parties.import`: the CSV text.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportInput {
    /// The CSV file's text, with a header row.
    pub csv: String,
}

/// The output of `parties.import`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ImportOutput {
    /// Parties created.
    pub created: u32,
    /// Parties updated.
    pub updated: u32,
    /// The party ids the file wrote, in file order.
    pub ids: Vec<String>,
}

/// `parties.import_check`: reads a file and reports each row, writing nothing.
pub struct PartiesImportCheck {
    /// The acting person, when the caller is one (bank rows need one).
    pub person_id: Option<String>,
}

impl PartiesImportCheck {
    /// The check for `person_id`; `None` for an agent key.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesImportCheck {
    const NAME: &'static str = "parties.import_check";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = ImportInput;
    type Output = ImportCheckOutput;

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
        let table = table_of(&input.csv)?;
        let plan = plan_import(db, ctx, &table, self.person_id.as_deref()).await?;
        let errors = plan.errors();
        let creates = plan.created;
        let updates = plan.updated;
        Ok(Plan::with_output(ImportCheckOutput {
            valid: errors == 0,
            creates,
            updates,
            errors,
            rows: plan.rows,
        }))
    }
}

/// `parties.import`: writes a file whose rows are all valid. A file with any
/// error is refused whole; run `parties.import_check` for the rows.
pub struct PartiesImport {
    /// The acting person, when the caller is one (bank rows need one).
    pub person_id: Option<String>,
}

impl PartiesImport {
    /// The import for `person_id`; `None` for an agent key.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesImport {
    const NAME: &'static str = "parties.import";
    const TIER: Tier = Tier::Draft;

    type Input = ImportInput;
    type Output = ImportOutput;

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
        let table = table_of(&input.csv)?;
        let plan = plan_import(db, ctx, &table, self.person_id.as_deref()).await?;
        let errors = plan.errors();
        if errors > 0 {
            return Err(refuse(
                &IMPORT_INVALID,
                "csv",
                format!("{errors} row(s) have errors; run parties.import_check for the rows"),
            ));
        }
        let output = ImportOutput {
            created: plan.created,
            updated: plan.updated,
            ids: plan.ids.clone(),
        };
        let mut out = Plan::with_output(output);
        out.statements = plan.statements;
        out.objects = plan.objects;
        Ok(out)
    }
}

/// The input of `parties.export`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {}

/// The output of `parties.export`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExportOutput {
    /// The CSV text, header first.
    pub csv: String,
    /// The number of parties in it.
    pub rows: u32,
}

/// `parties.export`: every unmerged party as CSV, in name order.
pub struct PartiesExport;

#[async_trait::async_trait]
impl Command for PartiesExport {
    const NAME: &'static str = "parties.export";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = ExportInput;
    type Output = ExportOutput;

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
        _input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let mut parties = unmerged(db, &ctx.company_id).await?;
        if parties.len() > MAX_EXPORT_ROWS {
            return Err(refuse(
                &IMPORT_TOO_MANY,
                "csv",
                format!("the company has more than {MAX_EXPORT_ROWS} parties; export in parts"),
            ));
        }
        parties.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        let rows = u32::try_from(parties.len()).unwrap_or(u32::MAX);
        Ok(Plan::with_output(ExportOutput {
            csv: party_csv(&parties),
            rows,
        }))
    }
}
