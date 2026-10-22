//! The bill commands: create from a document, update, read, list, confirm the
//! vendor (a person only) and post. Posting builds the journal entry through
//! `journal.post_entry`, so the policy, the approval and the audit trail are
//! the ledger's own: an agent's post into a protected account is held for a
//! person, and the bill records the entry it became.

use crate::model::{
    BillInput, BillLine, BillView, CheckView, FLAG_INSTRUCTION_TEXT, PreviewLine, Status,
    VendorView, derive_status, entry_lines, open_flags,
};
use crate::store::{self, BillRow};
use cratefield_core::{Database, Problem, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_extract::checks;
use ledgers_extract::guard;
use ledgers_extract::schema::parse_iso_date;
use ledgers_ledger::account::{AccountEntry, AccountType};
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, PostEntryOutput, SerializableAmount, SerializableEntryDraft,
    SerializableLine, SerializablePostedEntry,
};
use ledgers_ledger::units::{self, UnitScope};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::Date;

/// The most bills one list call returns.
const MAX_LIST: u32 = 200;
/// The chart tag of the input VAT account.
const TAG_VAT_INPUT: &str = "vat-input";
/// The chart tag of accounts payable.
const TAG_PAYABLE: &str = "ap";

/// A duplicate bill: the same document, or the same invoice number from the
/// same vendor. The problem names the bill already stored.
pub const DUPLICATE: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "bill-duplicate",
    status: cratefield_core::axum::http::StatusCode::CONFLICT,
    title: "This bill is already stored",
    description: "The document or the invoice number of this bill matches a bill that exists.",
};

fn duplicate(existing: &str, reason: &str) -> CommandError {
    CommandError::Domain(
        Problem::new(&DUPLICATE)
            .with_detail(format!("it matches bill {existing} ({reason})"))
            .with_extension("existing_bill_id", existing)
            .with_extension("reason", reason),
    )
}

fn not_found() -> CommandError {
    CommandError::Domain(Problem::not_found())
}

/// The day of the command's clock, as a date.
fn today(ctx: &CommandContext) -> Result<Date, CommandError> {
    parse_iso_date(ctx.now.get(..10).unwrap_or("")).ok_or_else(|| {
        CommandError::InvalidInput(format!("the command clock {} is not a timestamp", ctx.now))
    })
}

/// A chart account, as bills use it.
#[derive(Clone, Debug)]
struct AccountInfo {
    account_type: AccountType,
    archived: bool,
    postable: bool,
    tags: Vec<String>,
}

/// The chart, indexed by number, with the two accounts a bill posts to.
#[derive(Clone, Debug, Default)]
struct Chart {
    by_number: BTreeMap<String, AccountInfo>,
    vat_input: Option<String>,
    payable: Option<String>,
}

impl Chart {
    fn from_entries(entries: &[AccountEntry]) -> Self {
        let mut chart = Chart::default();
        for entry in entries {
            let account = &entry.account;
            let info = AccountInfo {
                account_type: account.account_type,
                archived: account.archived,
                postable: entry.is_postable(),
                tags: account.tags.clone(),
            };
            if !account.archived {
                if chart.vat_input.is_none() && info.tags.iter().any(|t| t == TAG_VAT_INPUT) {
                    chart.vat_input = Some(account.number.clone());
                }
                if chart.payable.is_none() && info.tags.iter().any(|t| t == TAG_PAYABLE) {
                    chart.payable = Some(account.number.clone());
                }
            }
            chart.by_number.insert(account.number.clone(), info);
        }
        chart
    }

    /// Whether a line may debit `number`: a live, postable expense account.
    fn expense_ok(&self, number: &str) -> bool {
        self.by_number.get(number).is_some_and(|info| {
            info.account_type == AccountType::Expense && !info.archived && info.postable
        })
    }
}

async fn load_chart(db: &dyn Database, company_id: &str) -> Result<Chart, CommandError> {
    let entries = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(Chart::from_entries(&entries))
}

fn check_expense_accounts(chart: &Chart, lines: &[BillLine]) -> Result<(), CommandError> {
    for line in lines {
        if let Some(number) = &line.expense_account
            && !chart.expense_ok(number)
        {
            return Err(CommandError::InvalidInput(format!(
                "account {number} is not a live, postable expense account"
            )));
        }
    }
    Ok(())
}

/// Fills the expense account of each line that has none: the first candidate
/// from the vendor's history that is still a live expense account. The
/// suggestion keeps the bill it came from, so the reason is shown.
async fn suggest_accounts(
    db: &dyn Database,
    company_id: &str,
    chart: &Chart,
    vendor_id: &str,
    lines: &mut [BillLine],
) -> Result<(), CommandError> {
    if lines.iter().all(|line| line.expense_account.is_some()) {
        return Ok(());
    }
    let history = store::vendor_history(db, company_id, vendor_id).await?;
    for line in lines
        .iter_mut()
        .filter(|line| line.expense_account.is_none())
    {
        if let Some((account, from)) = history
            .iter()
            .find(|(account, _)| chart.expense_ok(account))
        {
            line.expense_account = Some(account.clone());
            line.suggested_from = Some(from.clone());
        }
    }
    Ok(())
}

/// The text layer of the document the bill is for (read at upload), if any.
async fn document_text(
    db: &dyn Database,
    company_id: &str,
    document_id: &str,
) -> Result<Option<String>, CommandError> {
    let document = ledgers_documents::store::get(db, company_id, document_id).await?;
    if document.is_none() {
        return Err(CommandError::InvalidInput(format!(
            "document {document_id} is not stored"
        )));
    }
    Ok(ledgers_documents::store::text_layer(db, company_id, document_id).await?)
}

/// The checks and the instruction flags of a bill's fields and document text.
fn checks_and_flags(
    input: &BillInput,
    today: Date,
    text: Option<&str>,
) -> (Vec<CheckView>, Vec<String>) {
    let report = checks::run(&input.as_invoice(), today, text);
    let checks = report.checks.iter().map(CheckView::from).collect();
    let flags = text
        .map(guard::screen)
        .unwrap_or_default()
        .into_iter()
        .map(|phrase| format!("{FLAG_INSTRUCTION_TEXT}:{phrase}"))
        .collect();
    (checks, flags)
}

fn normalise_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalise_iban(iban: &str) -> String {
    iban.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
}

/// Whether an invoice IBAN differs from the IBAN the vendor is known by. A
/// vendor with no known IBAN, or an invoice with none, differs from nothing.
fn iban_differs(known: Option<&str>, seen: Option<&str>) -> bool {
    match (known, seen) {
        (Some(known), Some(seen)) => normalise_iban(known) != normalise_iban(seen),
        _ => false,
    }
}

/// The vendor a bill belongs to: matched by VAT id, then by normalised name,
/// otherwise a new vendor that needs a person. Returns the vendor and
/// whether its IBAN differs from the one the vendor is known by.
async fn resolve_vendor(
    db: &dyn Database,
    company_id: &str,
    input: &BillInput,
    ctx: &CommandContext,
) -> Result<(VendorView, Option<VendorView>, bool), CommandError> {
    let known = store::vendors(db, company_id).await?;
    let by_vat = input.vendor_vat_id.as_deref().and_then(|vat| {
        known
            .iter()
            .find(|v| v.vat_id.as_deref() == Some(vat))
            .cloned()
    });
    let by_name = input.vendor_name.as_deref().and_then(|name| {
        let wanted = normalise_name(name);
        (!wanted.is_empty())
            .then(|| {
                known
                    .iter()
                    .find(|v| normalise_name(&v.name) == wanted)
                    .cloned()
            })
            .flatten()
    });
    if let Some(vendor) = by_vat.or(by_name) {
        let changed = iban_differs(vendor.iban.as_deref(), input.vendor_iban.as_deref());
        return Ok((vendor, None, changed));
    }
    let name = input
        .vendor_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| CommandError::InvalidInput("the vendor needs a name".into()))?;
    let vendor = VendorView {
        id: format!("ven_{}", ctx.event_id),
        name,
        vat_id: input.vendor_vat_id.clone(),
        iban: input.vendor_iban.clone(),
        needs_review: true,
    };
    Ok((vendor.clone(), Some(vendor), false))
}

/// Builds the in-memory row a bill view is made from.
#[allow(clippy::too_many_arguments)]
fn row_of(
    id: &str,
    input: &BillInput,
    vendor: VendorView,
    lines: Vec<BillLine>,
    checks: Vec<CheckView>,
    flags: Vec<String>,
    iban_changed: bool,
    created: (&str, &str),
    posted_entry_id: Option<String>,
) -> BillRow {
    let confidence = input.confidence.clone();
    BillRow {
        id: id.to_owned(),
        document_id: input.document_id.clone(),
        vendor_id: vendor.id.clone(),
        vendor,
        invoice_number: input.invoice_number.clone(),
        invoice_date: input.invoice_date.clone(),
        due_date: input.due_date.clone(),
        currency: input.currency.clone().unwrap_or_default(),
        net_minor: input.net_minor,
        vat_minor: input.vat_minor,
        gross_minor: input.gross_minor,
        lines,
        confidence,
        checks,
        instruction_flags: flags,
        iban_changed,
        source: input.source.clone().unwrap_or_else(|| "manual".to_owned()),
        reader: input.reader.clone(),
        unit: input.unit.clone(),
        posted_entry_id,
        discarded_at: None,
        created_by: input.created_by.clone(),
        created_at: created.0.to_owned(),
        updated_at: created.1.to_owned(),
    }
}

/// The journal preview of a bill: the entry it posts as, when every line has
/// an expense account and the totals are known. Empty otherwise.
fn preview_of(row: &BillRow, chart: &Chart) -> Vec<PreviewLine> {
    let (Some(vat), Some(payable), Some(vat_minor), Some(gross)) = (
        chart.vat_input.as_deref(),
        chart.payable.as_deref(),
        row.vat_minor,
        row.gross_minor,
    ) else {
        return Vec::new();
    };
    if row.lines.is_empty()
        || row
            .lines
            .iter()
            .any(|l| l.expense_account.is_none() || l.net_minor.is_none())
    {
        return Vec::new();
    }
    entry_lines(&row.lines, vat_minor, gross, vat, payable)
}

/// The API view of a bill row.
fn view_of(row: BillRow, journal_preview: Vec<PreviewLine>) -> BillView {
    let flags = open_flags(
        &row.instruction_flags,
        row.vendor.needs_review,
        row.iban_changed,
        &row.lines,
    );
    let posted = row.posted_entry_id.is_some();
    let status = derive_status(posted, row.discarded_at.is_some(), &row.checks, &flags);
    BillView {
        id: row.id,
        status,
        document_id: row.document_id,
        vendor: row.vendor,
        invoice_number: row.invoice_number,
        invoice_date: row.invoice_date,
        due_date: row.due_date,
        currency: row.currency,
        net_minor: row.net_minor,
        vat_minor: row.vat_minor,
        gross_minor: row.gross_minor,
        lines: row.lines,
        confidence: row.confidence,
        checks: row.checks,
        flags,
        source: row.source,
        reader: row.reader,
        unit: row.unit,
        posted_entry_id: row.posted_entry_id,
        journal_preview: if posted { Vec::new() } else { journal_preview },
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// The bill as stored, read again: the view a command returns after writing.
async fn read_view(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<BillView, CommandError> {
    let row = store::bill_by_id(db, company_id, id)
        .await?
        .ok_or_else(not_found)?;
    let chart = load_chart(db, company_id).await?;
    let preview = preview_of(&row, &chart);
    Ok(view_of(row, preview))
}

/// Refuses a second bill for the same document or the same vendor invoice.
async fn check_duplicates(
    db: &dyn Database,
    company_id: &str,
    document_id: &str,
    vendor_id: Option<&str>,
    invoice_number: Option<&str>,
    own_id: Option<&str>,
) -> Result<(), CommandError> {
    if let Some(existing) = store::bill_for_document(db, company_id, document_id).await?
        && Some(existing.as_str()) != own_id
    {
        return Err(duplicate(&existing, "the same document"));
    }
    if let (Some(vendor), Some(number)) = (vendor_id, invoice_number)
        && let Some(existing) = store::bill_for_invoice(db, company_id, vendor, number).await?
        && Some(existing.as_str()) != own_id
    {
        return Err(duplicate(&existing, "the same vendor and invoice number"));
    }
    Ok(())
}

/// Input of `bills.create`.
pub type CreateInput = BillInput;

/// Output of `bills.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateOutput {
    /// The new bill.
    pub bill: BillView,
}

/// `bills.create`: a draft bill from a stored document and its fields.
pub struct BillsCreate;

#[async_trait::async_trait]
impl Command for BillsCreate {
    const NAME: &'static str = "bills.create";
    const TIER: Tier = Tier::Draft;

    type Input = CreateInput;
    type Output = CreateOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        check_duplicates(db, &ctx.company_id, &input.document_id, None, None, None).await?;
        document_text(db, &ctx.company_id, &input.document_id).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        if input.id.is_empty() {
            return Err(CommandError::InvalidInput("the bill id is missing".into()));
        }
        let text = document_text(db, &ctx.company_id, &input.document_id).await?;
        let (vendor, new_vendor, iban_changed) =
            resolve_vendor(db, &ctx.company_id, input, ctx).await?;
        check_duplicates(
            db,
            &ctx.company_id,
            &input.document_id,
            Some(&vendor.id),
            input.invoice_number.as_deref(),
            None,
        )
        .await?;
        let chart = load_chart(db, &ctx.company_id).await?;
        let mut lines = input.lines.clone();
        check_expense_accounts(&chart, &lines)?;
        suggest_accounts(db, &ctx.company_id, &chart, &vendor.id, &mut lines).await?;
        let day = today(ctx)?;
        let (checks, flags) = checks_and_flags(input, day, text.as_deref());
        let row = row_of(
            &input.id,
            input,
            vendor.clone(),
            lines.clone(),
            checks,
            flags,
            iban_changed,
            (&ctx.now, &ctx.now),
            None,
        );
        let mut statements = Vec::new();
        if let Some(new) = &new_vendor {
            statements.push(Statement::with_values(
                "INSERT INTO vendor (id, company_id, name, vat_id, iban, source, needs_review, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(new.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(new.name.as_str()),
                    Value::from(new.vat_id.clone()),
                    Value::from(new.iban.clone()),
                    Value::from(if input.source.as_deref() == Some("manual") { "manual" } else { "extraction" }),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.now.as_str()),
                ],
            ));
        }
        statements.push(insert_bill(&ctx.company_id, &row)?);
        let preview = preview_of(&row, &chart);
        let mut plan = Plan::with_output(CreateOutput {
            bill: view_of(row, preview),
        });
        plan.statements = statements;
        plan.objects = vec![input.id.clone()];
        Ok(plan)
    }
}

fn json_text<T: Serialize>(value: &T) -> Result<String, CommandError> {
    serde_json::to_string(value).map_err(|err| CommandError::InvalidInput(err.to_string()))
}

fn insert_bill(company_id: &str, row: &BillRow) -> Result<Statement, CommandError> {
    Ok(Statement::with_values(
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, invoice_date, due_date, \
         currency, net_minor, vat_minor, gross_minor, lines_json, confidence_json, checks_json, \
         instruction_flags, iban_changed, source, reader, unit_code, posted_entry_id, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(row.id.as_str()),
            Value::from(company_id),
            Value::from(row.document_id.as_str()),
            Value::from(row.vendor_id.as_str()),
            Value::from(row.invoice_number.clone()),
            Value::from(row.invoice_date.clone()),
            Value::from(row.due_date.clone()),
            Value::from(row.currency.as_str()),
            Value::from(row.net_minor),
            Value::from(row.vat_minor),
            Value::from(row.gross_minor),
            Value::from(json_text(&row.lines)?),
            Value::from(json_text(&row.confidence)?),
            Value::from(json_text(&row.checks)?),
            Value::from(json_text(&row.instruction_flags)?),
            Value::from(i64::from(row.iban_changed)),
            Value::from(row.source.as_str()),
            Value::from(row.reader.clone()),
            Value::from(row.unit.clone()),
            Value::from(row.created_by.as_str()),
            Value::from(row.created_at.as_str()),
            Value::from(row.updated_at.as_str()),
        ],
    ))
}

/// Input of `bills.update`: the bill id and the corrected fields.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateInput {
    /// The bill id (from the path).
    pub id: String,
    /// The corrected fields, in the same shape as create.
    #[serde(flatten)]
    pub fields: BillInput,
}

/// `bills.update`: corrects a bill's fields and lines and checks them again.
/// The vendor and the document are not changed here.
pub struct BillsUpdate;

#[async_trait::async_trait]
impl Command for BillsUpdate {
    const NAME: &'static str = "bills.update";
    const TIER: Tier = Tier::Draft;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = UpdateInput;
    type Output = CreateOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let existing = store::bill_by_id(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if existing.posted_entry_id.is_some() {
            return Err(CommandError::InvalidInput(
                "a posted bill is not edited; post a reversal and a new bill".into(),
            ));
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
        let existing = store::bill_by_id(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if existing.posted_entry_id.is_some() {
            return Err(CommandError::InvalidInput(
                "a posted bill is not edited; post a reversal and a new bill".into(),
            ));
        }
        let mut fields = input.fields.clone();
        fields.document_id = existing.document_id.clone();
        fields.source = Some(existing.source.clone());
        fields.reader = existing.reader.clone();
        fields.created_by = existing.created_by.clone();
        let text = document_text(db, &ctx.company_id, &existing.document_id).await?;
        check_duplicates(
            db,
            &ctx.company_id,
            &existing.document_id,
            Some(&existing.vendor_id),
            fields.invoice_number.as_deref(),
            Some(&existing.id),
        )
        .await?;
        let chart = load_chart(db, &ctx.company_id).await?;
        check_expense_accounts(&chart, &fields.lines)?;
        let mut lines = fields.lines.clone();
        suggest_accounts(db, &ctx.company_id, &chart, &existing.vendor_id, &mut lines).await?;
        let (checks, flags) = checks_and_flags(&fields, today(ctx)?, text.as_deref());
        let iban_changed = iban_differs(
            existing.vendor.iban.as_deref(),
            fields.vendor_iban.as_deref(),
        );
        let row = row_of(
            &existing.id,
            &fields,
            existing.vendor.clone(),
            lines,
            checks,
            flags,
            iban_changed,
            (&existing.created_at, &ctx.now),
            None,
        );
        let statement = Statement::with_values(
            "UPDATE bill SET invoice_number = ?, invoice_date = ?, due_date = ?, currency = ?, net_minor = ?, \
             vat_minor = ?, gross_minor = ?, lines_json = ?, checks_json = ?, instruction_flags = ?, \
             iban_changed = ?, unit_code = ?, updated_at = ? WHERE company_id = ? AND id = ? AND posted_entry_id IS NULL"
                .to_owned(),
            vec![
                Value::from(row.invoice_number.clone()),
                Value::from(row.invoice_date.clone()),
                Value::from(row.due_date.clone()),
                Value::from(row.currency.as_str()),
                Value::from(row.net_minor),
                Value::from(row.vat_minor),
                Value::from(row.gross_minor),
                Value::from(json_text(&row.lines)?),
                Value::from(json_text(&row.checks)?),
                Value::from(json_text(&row.instruction_flags)?),
                Value::from(i64::from(row.iban_changed)),
                Value::from(row.unit.clone()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(existing.id.as_str()),
            ],
        );
        let preview = preview_of(&row, &chart);
        let mut plan = Plan::with_output(CreateOutput {
            bill: view_of(row, preview),
        });
        plan.statements = vec![statement];
        plan.objects = vec![existing.id];
        Ok(plan)
    }
}

/// Input of `bills.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetInput {
    /// The bill id (from the path).
    pub id: String,
}

/// Output of `bills.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetOutput {
    /// The bill.
    pub bill: BillView,
}

/// Input of `bills.discard`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DiscardInput {
    /// The bill id (from the path).
    pub id: String,
}

/// `bills.discard`: a person drops a draft bill. A posted bill is not
/// discarded (post a reversal instead); a discarded bill cannot post.
pub struct BillsDiscard;

#[async_trait::async_trait]
impl Command for BillsDiscard {
    const NAME: &'static str = "bills.discard";
    const TIER: Tier = Tier::Draft;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = DiscardInput;
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
        let mut row = store::bill_by_id(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if row.posted_entry_id.is_some() {
            return Err(CommandError::InvalidInput(
                "a posted bill is not discarded; post a reversal instead".into(),
            ));
        }
        row.discarded_at = Some(ctx.now.clone());
        let statement = Statement::with_values(
            "UPDATE bill SET discarded_at = ?, updated_at = ? WHERE company_id = ? AND id = ? \
             AND posted_entry_id IS NULL"
                .to_owned(),
            vec![
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(row.id.as_str()),
            ],
        );
        let chart = load_chart(db, &ctx.company_id).await?;
        let preview = preview_of(&row, &chart);
        let mut plan = Plan::with_output(GetOutput {
            bill: view_of(row, preview),
        });
        plan.statements = vec![statement];
        Ok(plan)
    }
}

/// `bills.get`.
pub struct BillsGet;

#[async_trait::async_trait]
impl Command for BillsGet {
    const NAME: &'static str = "bills.get";
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
        let bill = read_view(db, &ctx.company_id, &input.id).await?;
        Ok(Plan::with_output(GetOutput { bill }))
    }
}

/// Input of `bills.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListInput {
    /// The most rows to return (default 50, cap 200).
    pub limit: Option<u32>,
    /// Keep only bills with this status.
    pub status: Option<Status>,
    /// The business-unit scope: a unit code, or `all` (the default).
    #[serde(default)]
    pub unit: Option<String>,
}

/// Output of `bills.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListOutput {
    /// Bills, newest first.
    pub bills: Vec<BillView>,
}

/// `bills.list`.
pub struct BillsList;

#[async_trait::async_trait]
impl Command for BillsList {
    const NAME: &'static str = "bills.list";
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
        let chart = load_chart(db, &ctx.company_id).await?;
        let unit_code = match UnitScope::parse(input.unit.as_deref())
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?
        {
            UnitScope::Consolidated => None,
            UnitScope::Unit(code) => {
                if units::find_unit(db, &ctx.company_id, &code)
                    .await
                    .map_err(|err| CommandError::InvalidInput(err.to_string()))?
                    .is_none()
                {
                    return Err(CommandError::InvalidInput(format!(
                        "business unit {code} does not exist"
                    )));
                }
                Some(code)
            }
        };
        let rows = store::list_bills(db, &ctx.company_id, limit, unit_code.as_deref()).await?;
        let bills: Vec<BillView> = rows
            .into_iter()
            .map(|row| {
                let preview = preview_of(&row, &chart);
                view_of(row, preview)
            })
            .filter(|bill| input.status.is_none_or(|wanted| bill.status == wanted))
            .collect();
        Ok(Plan::with_output(ListOutput { bills }))
    }
}

/// Input of `bills.confirm`: a person confirms the vendor and the IBAN of a bill.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmInput {
    /// The bill id (from the path).
    pub id: String,
}

/// `bills.confirm`: clears the new-vendor and changed-IBAN flags. Human only:
/// an agent that creates a vendor cannot confirm it.
pub struct BillsConfirm;

#[async_trait::async_trait]
impl Command for BillsConfirm {
    const NAME: &'static str = "bills.confirm";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = ConfirmInput;
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
        let mut row = store::bill_by_id(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(not_found)?;
        if row.posted_entry_id.is_some() {
            return Err(CommandError::InvalidInput(
                "the bill is already posted".into(),
            ));
        }
        row.vendor.needs_review = false;
        row.iban_changed = false;
        let statements = vec![
            Statement::with_values(
                "UPDATE vendor SET needs_review = 0, updated_at = ? WHERE company_id = ? AND id = ?".to_owned(),
                vec![
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(row.vendor_id.as_str()),
                ],
            ),
            Statement::with_values(
                "UPDATE bill SET iban_changed = 0, updated_at = ? WHERE company_id = ? AND id = ?".to_owned(),
                vec![
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(row.id.as_str()),
                ],
            ),
        ];
        let chart = load_chart(db, &ctx.company_id).await?;
        let preview = preview_of(&row, &chart);
        let mut plan = Plan::with_output(GetOutput {
            bill: view_of(row, preview),
        });
        plan.statements = statements;
        Ok(plan)
    }
}

/// Input of `bills.post`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostInput {
    /// The bill id (from the path).
    pub id: String,
    /// The journal to post into; defaults to the general journal `GN`.
    pub journal_id: Option<String>,
    /// The fiscal year; defaults to the year of the posting date.
    pub fiscal_year: Option<i64>,
}

/// The journal entry a bill became.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostedEntryView {
    /// The entry id.
    pub entry_id: String,
    /// The journal it posted into.
    pub journal_id: String,
    /// The fiscal year the number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number.
    pub number: i64,
    /// The recorded-at timestamp.
    pub recorded_at: String,
}

impl From<SerializablePostedEntry> for PostedEntryView {
    fn from(entry: SerializablePostedEntry) -> Self {
        PostedEntryView {
            entry_id: entry.entry_id,
            journal_id: entry.journal_id,
            fiscal_year: entry.fiscal_year,
            number: entry.number,
            recorded_at: entry.recorded_at,
        }
    }
}

/// Output of `bills.post`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostOutput {
    /// The bill, now posted.
    pub bill: BillView,
    /// The journal entry it became.
    pub entry: PostedEntryView,
}

/// `bills.post`: posts a ready bill as its journal entry.
pub struct BillsPost;

/// Builds the entry a bill posts as, with the ledger's own input shape.
#[allow(clippy::too_many_lines)]
async fn entry_for(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &PostInput,
) -> Result<(BillRow, PostEntryInput), CommandError> {
    let row = store::bill_by_id(db, &ctx.company_id, &input.id)
        .await?
        .ok_or_else(not_found)?;
    if row.posted_entry_id.is_some() {
        return Err(CommandError::InvalidInput(
            "the bill is already posted".into(),
        ));
    }
    if row.discarded_at.is_some() {
        return Err(CommandError::InvalidInput(
            "the bill was discarded and cannot post".into(),
        ));
    }
    let flags = open_flags(
        &row.instruction_flags,
        row.vendor.needs_review,
        row.iban_changed,
        &row.lines,
    );
    if !row.checks.iter().all(|c| c.ok) || !flags.is_empty() {
        let failed: Vec<&str> = row
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.id.as_str())
            .collect();
        return Err(CommandError::InvalidInput(format!(
            "the bill needs review before it posts: failed checks {failed:?}, open flags {flags:?}"
        )));
    }
    let base: String = db
        .query(&Statement::with_values(
            "SELECT base_currency FROM company WHERE id = ?".to_owned(),
            vec![Value::from(ctx.company_id.as_str())],
        ))
        .await?
        .first()
        .and_then(|r| r.get::<String>("base_currency"))
        .ok_or_else(|| CommandError::InvalidInput("the company has no base currency".into()))?;
    if row.currency != base {
        return Err(CommandError::InvalidInput(format!(
            "a {} bill cannot post into the {base} books: foreign-currency bills need an exchange rate, which is not in this slice",
            row.currency
        )));
    }
    let chart = load_chart(db, &ctx.company_id).await?;
    let vat = chart.vat_input.clone().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no input VAT account (tag vat-input)".into())
    })?;
    let payable = chart.payable.clone().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no payable account (tag ap)".into())
    })?;
    let (Some(vat_minor), Some(gross)) = (row.vat_minor, row.gross_minor) else {
        return Err(CommandError::InvalidInput(
            "the bill has no VAT or gross total".into(),
        ));
    };
    let lines = entry_lines(&row.lines, vat_minor, gross, &vat, &payable);
    let unit = bill_unit(db, &ctx.company_id, row.unit.as_deref()).await?;
    let journal_id = match &input.journal_id {
        Some(id) => id.clone(),
        None => ledgers_ledger::journal::find_journal(db, &ctx.company_id, "GN")
            .await
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?
            .map(|journal| journal.id)
            .ok_or_else(|| CommandError::InvalidInput("no general journal GN exists".into()))?,
    };
    let posting_date = ctx.now.get(..10).unwrap_or("").to_owned();
    let fiscal_year = match input.fiscal_year {
        Some(year) => year,
        None => posting_date
            .get(..4)
            .and_then(|y| y.parse().ok())
            .ok_or_else(|| CommandError::InvalidInput("the command clock has no year".into()))?,
    };
    let description = format!(
        "Bill {} {}",
        row.vendor.name,
        row.invoice_number.clone().unwrap_or_default()
    )
    .trim()
    .to_owned();
    let entry = SerializableEntryDraft {
        journal_id: journal_id.clone(),
        document_date: row
            .invoice_date
            .clone()
            .unwrap_or_else(|| posting_date.clone()),
        posting_date,
        tax_point_date: None,
        base_currency: row.currency.clone(),
        description,
        source_documents: vec![row.document_id.clone()],
        external_id: Some(format!("bill-{}", row.id)),
        source_app: Some("ledgers-bills".to_owned()),
        reverses: None,
        lines: lines
            .into_iter()
            .map(|line| {
                // Expense lines carry the bill's unit; VAT and the payable are
                // balance-sheet lines and follow their account's unit rule.
                let on_unit = line.account != vat && line.account != payable;
                SerializableLine {
                    account_number: line.account,
                    side: line.side,
                    base: SerializableAmount {
                        minor: line.minor.to_string(),
                        currency: row.currency.clone(),
                    },
                    fx: None,
                    tax_code: None,
                    dimensions: match (&unit, on_unit) {
                        (Some(code), true) => BTreeMap::from([("unit".to_owned(), code.clone())]),
                        _ => BTreeMap::new(),
                    },
                    memo: None,
                }
            })
            .collect(),
    };
    Ok((
        row,
        PostEntryInput {
            journal_id,
            fiscal_year,
            entry,
        },
    ))
}

#[async_trait::async_trait]
impl Command for BillsPost {
    const NAME: &'static str = "bills.post";
    const TIER: Tier = Tier::PostWithinLimits;
    const SUPPORTS_DRY_RUN: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = PostInput;
    type Output = PostOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let (_, entry) = entry_for(db, ctx, input).await?;
        PostEntry.preview(db, ctx, &entry).await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (row, entry) = entry_for(db, ctx, input).await?;
        let posted: Plan<PostEntryOutput> = PostEntry.plan(token, db, ctx, &entry).await?;
        let mut statements = posted.statements;
        statements.push(Statement::with_values(
            "UPDATE bill SET posted_entry_id = ?, updated_at = ? \
             WHERE company_id = ? AND id = ? AND posted_entry_id IS NULL"
                .to_owned(),
            vec![
                Value::from(posted.output.entry.entry_id.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(row.id.as_str()),
            ],
        ));
        // The view of the bill as the batch leaves it: posted, with the entry.
        let bill = view_of(
            BillRow {
                posted_entry_id: Some(posted.output.entry.entry_id.clone()),
                ..row.clone()
            },
            Vec::new(),
        );
        let mut out = Plan::with_output(PostOutput {
            bill,
            entry: PostedEntryView::from(posted.output.entry),
        });
        out.statements = statements;
        out.objects = vec![row.id];
        Ok(out)
    }
}

/// The business unit a bill posts under. While the company has units, a
/// bill must name one (every expense line carries it); a named unit must
/// exist and be active.
///
/// # Errors
/// [`CommandError::InvalidInput`] when units are enabled and none is named,
/// or the named unit is unknown or archived; [`CommandError::Database`] when
/// a read fails.
async fn bill_unit(
    db: &dyn Database,
    company_id: &str,
    named: Option<&str>,
) -> Result<Option<String>, CommandError> {
    let enabled = units::units_enabled(db, company_id)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    match named {
        None if enabled => Err(CommandError::InvalidInput(
            "the bill has no business unit; the company has units, so name one before it posts"
                .into(),
        )),
        None => Ok(None),
        Some(code) => {
            let unit = units::find_unit(db, company_id, code)
                .await
                .map_err(|err| CommandError::InvalidInput(err.to_string()))?
                .ok_or_else(|| {
                    CommandError::InvalidInput(format!("business unit {code} does not exist"))
                })?;
            if unit.archived {
                return Err(CommandError::InvalidInput(format!(
                    "business unit {code} is archived"
                )));
            }
            Ok(Some(unit.code))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_names_match_regardless_of_case_and_punctuation() {
        assert_eq!(
            normalise_name("Halden Supplies GmbH."),
            normalise_name("halden supplies gmbh")
        );
        assert_eq!(
            normalise_iban("de89 3704 0044"),
            "DE89370400 44".replace(' ', "")
        );
    }

    #[test]
    fn a_bill_for_a_known_document_is_a_duplicate_with_its_link() {
        let err = duplicate("01BILL", "the same document");
        match err {
            CommandError::Domain(problem) => {
                assert_eq!(problem.slug, "bill-duplicate");
                assert_eq!(
                    problem.status,
                    cratefield_core::axum::http::StatusCode::CONFLICT
                );
            }
            other => panic!("expected a domain problem, got {other:?}"),
        }
    }
}
