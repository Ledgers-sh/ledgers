//! The Settings commands: the company profile, the accounting years and
//! periods, invoice numbering, tax settings, approval rules and the audit
//! read. Every command here is human-only: an agent key gets 403 (and its
//! attempt is audited as denied). Writes go through the executor, so the
//! audit row lands in the same guarded batch as the change; the domain
//! functions that already own their audit (fiscal years, periods, tax) are
//! called from the person path below.

use super::settings_rules as rules;
use super::{Deps, ServerDispatch};
use crate::routes::Presented;
use async_trait::async_trait;
use cratefield_core::{Database, DbError, Statement};
use ledgers_audit::{AuditContext, AuditEvent, Outcome};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_ledger::close::{CloseCheckRegistry, DraftEntriesCloseCheck};
use ledgers_ledger::fiscal::{self, FiscalYear, FiscalYearSpec};
use ledgers_ledger::period::{self, PeriodState};
use ledgers_ledger::{AccountsError, EntryJournal};
use ledgers_tax::country::{self, CountryPack};
use ledgers_tax::rate::{DatedRate, Jurisdiction, RateCategory};
use ledgers_tax::settings::{TaxSettings, VatBasis};
use schemars::JsonSchema;
use sea_query::Value as SeaValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use time::Date;
use time::macros::format_description;

/// The `company_setting` key holding the company profile, as JSON.
pub const PROFILE_KEY: &str = "company.profile";
/// The `company_setting` key holding the low-risk limits, as a JSON object of
/// currency code to canonical amount.
pub const LOW_RISK_KEY: &str = "approvals.low_risk_limits";
/// The `company_setting` key holding the request expiry in hours.
pub const REQUEST_TTL_KEY: &str = "approvals.request_ttl_hours";
/// Audit events a page returns when the caller names no limit, and the cap.
const DEFAULT_AUDIT_LIMIT: u32 = 50;
const MAX_AUDIT_LIMIT: u32 = 200;
/// The largest sequence number a cursor may name. A D1 bind takes an integer
/// only as a JavaScript number, so anything past 2^53 - 1 fails serialization.
const MAX_CURSOR_SEQ: i64 = super::D1_SAFE_INTEGER_MAX;
/// Rows the audit read fetches per query while it scans for matches.
const AUDIT_CHUNK: usize = 200;
/// How many chunks one audit page may scan before it returns a cursor.
const AUDIT_MAX_CHUNKS: usize = 10;
/// The fiscal-year columns the domain reads (kept in step with ledgers-ledger).
const FISCAL_YEAR_COLUMNS: &str = "company_id, label, start_month, has_adjustment_period, \
     starts_on, ends_on, year_end_closed_at, closing_entry_id, year_end_report, created_at, updated_at";

/// The company profile fields the Settings screen edits. Name and base
/// currency are on the company row itself, not here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompanyProfile {
    /// The legal form, free text (for example `GmbH`).
    #[serde(default)]
    pub legal_form: String,
    /// The VAT identifier, with its country prefix. Empty when not registered.
    #[serde(default)]
    pub vat_id: String,
    /// The commercial register number or equivalent.
    #[serde(default)]
    pub registration_number: String,
    /// The street address, one line.
    #[serde(default)]
    pub address_line: String,
    /// The postal code.
    #[serde(default)]
    pub postal_code: String,
    /// The city.
    #[serde(default)]
    pub city: String,
    /// The country, as an ISO 3166-1 alpha-2 code.
    #[serde(default)]
    pub country: String,
    /// The address for invoices and notices from the company.
    #[serde(default)]
    pub contact_email: String,
    /// The company logo: the id of a stored PNG or JPEG of at most 2 MiB, in
    /// this company's documents. Empty when the company has no logo.
    #[serde(default)]
    pub logo_document_id: String,
}

/// The output of `company.get`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CompanyGetOutput {
    /// The company's name.
    pub name: String,
    /// The base currency, ISO 4217.
    pub base_currency: String,
    /// True once the company has an entry: the base currency can no longer change.
    pub base_currency_locked: bool,
    /// The profile fields.
    pub profile: CompanyProfile,
}

/// The input of `company.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CompanyUpdateInput {
    /// The company's name.
    pub name: String,
    /// The profile fields.
    pub profile: CompanyProfile,
}

/// The input of `tax_settings.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaxSettingsInput {
    /// Small-business exemption (Kleinunternehmer): no VAT is charged.
    pub small_business: bool,
    /// The VAT reporting method: `accrual` or `cash`.
    pub vat_basis: String,
    /// The company's own rates. Only for packs that allow them (generic and
    /// US sales tax); omitted leaves the stored rates as they are.
    #[serde(default)]
    pub user_rates: Option<Vec<UserRate>>,
}

/// A rate the company defines for a pack that allows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserRate {
    /// `standard`, `reduced` or `zero`.
    pub category: String,
    /// The rate as a percentage, for example `8.25`.
    pub percent: String,
    /// The first day the rate applies, `YYYY-MM-DD`.
    pub valid_from: String,
    /// What the company says the rate is based on (free text, required).
    pub source: String,
}

/// The `company_setting` key holding the company's own rates, as JSON.
pub const USER_RATES_KEY: &str = "tax.user_rates";

/// One rate the company's country has on the given day.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TaxRateView {
    /// `standard`, `reduced` or `zero`.
    pub category: String,
    /// The rate as a percentage, for example `19`.
    pub percent: String,
    /// The day the rate took effect, `YYYY-MM-DD`.
    pub valid_from: String,
    /// The statute the rate comes from, or the company's own note.
    pub source: String,
}

/// One tax code entries use, as the company's pack defines it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TaxCodeView {
    /// The stable id (`SG-GST-SR-9`).
    pub code: String,
    /// The English label.
    pub label: String,
    /// `output_vat`, `input_vat`, `reverse_charge`, `exempt`, `zero` or `import_vat`.
    pub kind: String,
    /// `standard`, `reduced`, `zero` or `exempt`.
    pub category: String,
}

/// One return the pack can draft.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ReturnView {
    /// The layout id (`generic`, `sg-gst-f5`, `ustva`).
    pub id: String,
    /// The English title.
    pub label: String,
}

/// The output of `tax_settings.get`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TaxSettingsOutput {
    /// Small-business exemption in effect.
    pub small_business: bool,
    /// The VAT reporting method in effect.
    pub vat_basis: String,
    /// The company's country (from the profile), empty when not set.
    pub country: String,
    /// True when the platform carries a rate table for the country.
    pub rate_table: bool,
    /// The rates in force today; empty when there is no table.
    pub rates: Vec<TaxRateView>,
    /// The pack id that applies (`sg`, `de`, `generic`); empty without a country.
    pub pack: String,
    /// The pack's English name.
    pub pack_name: String,
    /// `vat`, `gst`, `sales_tax` or `none`.
    pub regime: String,
    /// True when the company defines its own rates for this pack.
    pub user_rates_allowed: bool,
    /// `complete`, or `pending` while some statutory rates are still unverified.
    pub rates_status: String,
    /// The registration number's name (`UEN`, `NPWP`, `USt-IdNr.`).
    pub registration_label: String,
    /// What the registration check covers.
    pub registration_note: String,
    /// The returns the pack can draft, generic first.
    pub returns: Vec<ReturnView>,
    /// The tax codes entries and bills use.
    pub tax_codes: Vec<TaxCodeView>,
    /// The company's own rates as stored.
    pub user_rates: Vec<UserRate>,
}

/// One period of a fiscal year and its state.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PeriodView {
    /// `YYYY-MM`.
    pub period: String,
    /// `open`, `soft_closed` or `closed`.
    pub state: String,
}

/// One fiscal year.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct FiscalYearView {
    /// The year's label (its calendar year, for a January start).
    pub label: i64,
    /// The first day of the year.
    pub starts_on: String,
    /// The last day of the year.
    pub ends_on: String,
    /// True once the year-end close has run.
    pub closed: bool,
    /// When the year-end close ran.
    pub closed_at: Option<String>,
    /// The year's months, in order.
    pub periods: Vec<PeriodView>,
}

/// The output of `fiscal_years.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct FiscalYearsListOutput {
    /// Fiscal years, oldest first.
    pub years: Vec<FiscalYearView>,
}

/// The input of `fiscal_years.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FiscalYearCreateInput {
    /// The calendar year to open (January to December).
    pub label: i64,
}

/// The input of `fiscal_years.close`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FiscalYearCloseInput {
    /// The year to close.
    pub label: i64,
}

/// The output of `fiscal_years.close`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct FiscalYearCloseOutput {
    /// The closed year.
    pub year: FiscalYearView,
    /// The closing entry the year-end close posted, when there was a result to close.
    pub closing_entry_id: Option<String>,
}

/// The input of `periods.close`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeriodCloseInput {
    /// The month to lock, `YYYY-MM`.
    pub period: String,
}

/// The input of `periods.reopen`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeriodReopenInput {
    /// The month to reopen, `YYYY-MM`.
    pub period: String,
    /// Why the month is reopened. Written to the audit trail.
    pub reason: String,
}

/// One number sequence: a journal in one fiscal year.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SequenceView {
    /// The fiscal year.
    pub fiscal_year: i64,
    /// The number the next entry in this journal and year receives.
    pub next_number: i64,
    /// Entries already posted in this journal and year.
    pub entries: i64,
    /// True while no number has been issued, so the next number may be set.
    pub editable: bool,
}

/// One journal and its sequences.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct JournalNumberingView {
    /// The journal's id.
    pub id: String,
    /// The journal code, for example `GN`.
    pub code: String,
    /// The journal's name.
    pub name: String,
    /// The journal kind (`general`, `sales`, ...).
    pub kind: String,
    /// One sequence per fiscal year.
    pub sequences: Vec<SequenceView>,
}

/// The output of `numbering.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct NumberingListOutput {
    /// Every journal of the company.
    pub journals: Vec<JournalNumberingView>,
}

/// The input of `numbering.set_next`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NumberingSetNextInput {
    /// The journal's id.
    pub journal_id: String,
    /// The fiscal year.
    pub fiscal_year: i64,
    /// The number the next entry receives. Must be above the current next number.
    pub next_number: i64,
}

/// The approval rules the Settings screen edits.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ApprovalSettingsView {
    /// Per-currency amount (major units, canonical) at or below which a money
    /// request is low risk and may be approved in bulk.
    pub low_risk_limits: BTreeMap<String, String>,
    /// How long a request waits for a decision before it expires.
    pub request_ttl_hours: u32,
    /// The sign-in window high-risk approvals need. Fixed by the approvals
    /// crate, shown so the rule is visible.
    pub fresh_sign_in_minutes: u32,
}

/// One audit event as the Settings read returns it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AuditEventView {
    /// The event id.
    pub id: String,
    /// The position in the company's chain.
    pub seq: i64,
    /// When it was recorded (RFC 3339, UTC).
    pub at: String,
    /// `person` or `agent`.
    pub actor_kind: String,
    /// The person's id, or the agent key's id.
    pub actor_id: String,
    /// The command the event records.
    pub command: String,
    /// `allowed`, `needs-approval`, `denied` or `error`.
    pub outcome: String,
    /// The objects the command touched.
    pub objects: Vec<String>,
    /// The reason a denial or an override gave, when there is one.
    pub reason: Option<String>,
    /// The request that caused the event.
    pub request_id: String,
}

/// The input of `audit.list`. Every filter is optional.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AuditListInput {
    /// Return events older than this sequence number (the cursor from the previous page).
    #[serde(default)]
    pub before_seq: Option<i64>,
    /// Events per page (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<u32>,
    /// `person` or `agent`.
    #[serde(default)]
    pub actor_kind: Option<String>,
    /// The exact command name, such as `entries.post`.
    #[serde(default)]
    pub command: Option<String>,
    /// `allowed`, `needs-approval`, `denied` or `error`.
    #[serde(default)]
    pub outcome: Option<String>,
    /// The first day to include, `YYYY-MM-DD` (UTC).
    #[serde(default)]
    pub from: Option<String>,
    /// The last day to include, `YYYY-MM-DD` (UTC).
    #[serde(default)]
    pub to: Option<String>,
}

/// The output of `audit.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AuditListOutput {
    /// The matching events, newest first.
    pub events: Vec<AuditEventView>,
    /// Pass as `before_seq` to get the next page; null when there is no more.
    pub next_before_seq: Option<i64>,
}

/// The period a `YYYY-MM` string names, or the reason it is not one.
fn parse_period(text: &str) -> Result<String, CommandError> {
    let valid = text.len() == 7
        && text.as_bytes()[4] == b'-'
        && text[..4].chars().all(|c| c.is_ascii_digit())
        && text[5..]
            .parse::<u32>()
            .is_ok_and(|m| (1..=12).contains(&m));
    if valid {
        Ok(text.to_owned())
    } else {
        Err(CommandError::InvalidInput(format!(
            "period must be YYYY-MM, got {text:?}"
        )))
    }
}

/// The company row's name and base currency.
async fn company_row(
    db: &dyn Database,
    company_id: &str,
) -> Result<(String, String), CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT name, base_currency FROM company WHERE id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .map_err(CommandError::Database)?;
    let row = rows
        .first()
        .ok_or_else(|| CommandError::InvalidInput("the company does not exist".to_owned()))?;
    let name = row.get::<String>("name").unwrap_or_default();
    let currency = row.get::<String>("base_currency").unwrap_or_default();
    Ok((name, currency))
}

/// One `company_setting` value, or `None`.
pub async fn setting(
    db: &dyn Database,
    company_id: &str,
    key: &str,
) -> Result<Option<String>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT value FROM company_setting WHERE company_id = ? AND key = ?".to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(key)],
        ))
        .await?;
    Ok(rows.first().and_then(|row| row.get::<String>("value")))
}

/// The statement that writes one `company_setting` row (insert or replace).
fn setting_statement(company_id: &str, key: &str, value: &str, now: &str) -> Statement {
    Statement::with_values(
        "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (company_id, key) DO UPDATE SET value = ?, updated_at = ?"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(key),
            SeaValue::from(value),
            SeaValue::from(now),
            SeaValue::from(value),
            SeaValue::from(now),
        ],
    )
}

/// The company profile as stored, or the empty profile when none is saved.
pub async fn profile(db: &dyn Database, company_id: &str) -> Result<CompanyProfile, DbError> {
    match setting(db, company_id, PROFILE_KEY).await? {
        Some(text) => serde_json::from_str(&text)
            .map_err(|err| DbError::Query(format!("company profile is not valid JSON: {err}"))),
        None => Ok(CompanyProfile::default()),
    }
}

/// The approval rules in effect for a company. A key that was never saved
/// reads as the default: the constants the approvals flow shipped with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRules {
    /// The low-risk limits, currency to canonical amount.
    pub low_risk: BTreeMap<String, String>,
    /// The request expiry in hours.
    pub ttl_hours: u32,
}

impl ApprovalRules {
    /// The defaults the approvals flow shipped with.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            low_risk: super::approvals::low_risk_limits(),
            ttl_hours: DEFAULT_TTL_HOURS,
        }
    }
}

/// The default request expiry: a day.
pub const DEFAULT_TTL_HOURS: u32 = 24;

/// The approval rules a company has saved, falling back to the defaults.
///
/// # Errors
/// A database error, or a saved value that is not valid (fails closed).
pub async fn approval_rules(db: &dyn Database, company_id: &str) -> Result<ApprovalRules, DbError> {
    let mut rules = ApprovalRules::defaults();
    if let Some(text) = setting(db, company_id, LOW_RISK_KEY).await? {
        rules.low_risk = serde_json::from_str(&text)
            .map_err(|err| DbError::Query(format!("low-risk limits are not valid JSON: {err}")))?;
    }
    if let Some(text) = setting(db, company_id, REQUEST_TTL_KEY).await? {
        rules.ttl_hours = text
            .parse()
            .map_err(|_| DbError::Query("request expiry is not a number of hours".to_owned()))?;
    }
    Ok(rules)
}

fn approval_view(rules: ApprovalRules) -> ApprovalSettingsView {
    ApprovalSettingsView {
        low_risk_limits: rules.low_risk,
        request_ttl_hours: rules.ttl_hours,
        fresh_sign_in_minutes: u32::try_from(ledgers_approvals::FRESH_SIGN_IN_SECS / 60)
            .unwrap_or(5),
    }
}

/// Maps a domain error to the command's problem.
fn domain(err: &AccountsError) -> CommandError {
    CommandError::Domain(err.to_problem())
}

/// The fiscal-year view for a year and its period states.
fn year_view(year: &FiscalYear, states: &HashMap<String, PeriodState>) -> FiscalYearView {
    let spec = year.spec();
    let periods = fiscal::period_ids(&spec)
        .into_iter()
        .map(|period| {
            let state = states.get(&period).copied().unwrap_or(PeriodState::Closed);
            PeriodView {
                period,
                state: state.as_str().to_owned(),
            }
        })
        .collect();
    FiscalYearView {
        label: year.label,
        starts_on: year.starts_on.clone(),
        ends_on: year.ends_on.clone(),
        closed: year.year_end_closed_at.is_some(),
        closed_at: year.year_end_closed_at.clone(),
        periods,
    }
}

/// Every fiscal year of the company with its periods: two queries whatever
/// the number of years.
pub async fn fiscal_years(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<FiscalYearView>, AccountsError> {
    let year_rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT {FISCAL_YEAR_COLUMNS} FROM fiscal_year WHERE company_id = ? ORDER BY label"
            ),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    let years = year_rows
        .rows
        .iter()
        .map(FiscalYear::from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let period_rows = db
        .query(&Statement::with_values(
            "SELECT period, status FROM accounting_period WHERE company_id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    let mut states: HashMap<String, PeriodState> = HashMap::new();
    for row in &period_rows.rows {
        let period = row.get::<String>("period").unwrap_or_default();
        let state = row
            .get::<String>("status")
            .and_then(|status| status.parse::<PeriodState>().ok())
            .unwrap_or(PeriodState::Closed);
        states.insert(period, state);
    }
    Ok(years.iter().map(|year| year_view(year, &states)).collect())
}

/// The company profile read, with the base currency lock.
async fn company_view(
    db: &dyn Database,
    company_id: &str,
) -> Result<CompanyGetOutput, CommandError> {
    let (name, base_currency) = company_row(db, company_id).await?;
    let locked = !db
        .query(&Statement::with_values(
            "SELECT 1 AS found FROM journal_entry WHERE company_id = ? LIMIT 1".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .map_err(CommandError::Database)?
        .is_empty();
    let profile = profile(db, company_id)
        .await
        .map_err(CommandError::Database)?;
    Ok(CompanyGetOutput {
        name,
        base_currency,
        base_currency_locked: locked,
        profile,
    })
}

/// Validates the company fields and returns the cleaned values.
///
/// # Errors
/// [`CommandError::InvalidInput`] naming the first field that does not fit.
pub fn clean_company(input: &CompanyUpdateInput) -> Result<(String, CompanyProfile), CommandError> {
    let invalid = |message: String| CommandError::InvalidInput(message);
    let name = rules::text_field("Company name", &input.name, rules::MAX_NAME_CHARS, true)
        .map_err(invalid)?;
    let p = &input.profile;
    let country = rules::country_field(&p.country).map_err(invalid)?;
    let vat_id = rules::normalize_vat_id(&p.vat_id);
    rules::check_vat_id(&country, &vat_id).map_err(invalid)?;
    let profile = CompanyProfile {
        legal_form: rules::text_field(
            "Legal form",
            &p.legal_form,
            rules::MAX_LEGAL_FORM_CHARS,
            false,
        )
        .map_err(invalid)?,
        vat_id,
        registration_number: rules::text_field(
            "Registration number",
            &p.registration_number,
            rules::MAX_REGISTRATION_CHARS,
            false,
        )
        .map_err(invalid)?,
        address_line: rules::text_field(
            "Address",
            &p.address_line,
            rules::MAX_ADDRESS_CHARS,
            false,
        )
        .map_err(invalid)?,
        postal_code: rules::text_field(
            "Postal code",
            &p.postal_code,
            rules::MAX_POSTAL_CODE_CHARS,
            false,
        )
        .map_err(invalid)?,
        city: rules::text_field("City", &p.city, rules::MAX_CITY_CHARS, false).map_err(invalid)?,
        country,
        contact_email: rules::email_field(&p.contact_email).map_err(invalid)?,
        logo_document_id: clean_logo_id(&p.logo_document_id).map_err(invalid)?,
    };
    Ok((name, profile))
}

/// Refuses a country change once the company has an entry: like the base
/// currency, the country (and so its tax pack) is fixed from the first entry.
/// A logo names a stored document by id: empty, or ASCII letters and digits up
/// to 64 characters.
fn clean_logo_id(raw: &str) -> Result<String, String> {
    let id = raw.trim();
    if id.is_empty() {
        return Ok(String::new());
    }
    if id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id.to_owned())
    } else {
        Err("The logo must be an uploaded image.".to_owned())
    }
}

/// Refuses a company logo that is not an image this company stores within the
/// logo size limit. An empty logo is always allowed.
async fn ensure_company_logo(
    db: &dyn Database,
    company_id: &str,
    logo: &str,
) -> Result<(), CommandError> {
    if logo.is_empty() {
        return Ok(());
    }
    let stored = ledgers_documents::store::get(db, company_id, logo)
        .await?
        .ok_or_else(|| {
            CommandError::InvalidInput(
                "The logo must be an image this company has uploaded.".to_owned(),
            )
        })?;
    ledgers_documents::logo::check_logo(&stored.mime, stored.size_bytes)
        .map_err(|err| CommandError::InvalidInput(err.to_string()))
}

async fn ensure_country_change_allowed(
    db: &dyn Database,
    company_id: &str,
    country: &str,
) -> Result<(), CommandError> {
    let current = company_view(db, company_id).await?;
    if current.profile.country != country && current.base_currency_locked {
        return Err(CommandError::InvalidInput(
            "The company country can only change before the first entry is posted.".to_owned(),
        ));
    }
    Ok(())
}

/// `company.get`: the name, base currency (with its lock) and profile.
pub struct CompanyGet;

#[async_trait]
impl Command for CompanyGet {
    const NAME: &'static str = "company.get";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = Value;
    type Output = CompanyGetOutput;

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
        Ok(Plan::with_output(company_view(db, &ctx.company_id).await?))
    }
}

/// `company.update`: writes the name and the profile in the audited batch.
pub struct CompanyUpdate;

#[async_trait]
impl Command for CompanyUpdate {
    const NAME: &'static str = "company.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = CompanyUpdateInput;
    type Output = CompanyGetOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let (_, profile) = clean_company(input)?;
        ensure_country_change_allowed(db, &ctx.company_id, &profile.country).await?;
        ensure_company_logo(db, &ctx.company_id, &profile.logo_document_id).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (name, profile) = clean_company(input)?;
        ensure_country_change_allowed(db, &ctx.company_id, &profile.country).await?;
        ensure_company_logo(db, &ctx.company_id, &profile.logo_document_id).await?;
        let stored = serde_json::to_string(&profile)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let statements = vec![
            Statement::with_values(
                "UPDATE company SET name = ?, updated_at = ? WHERE id = ?".to_owned(),
                vec![
                    SeaValue::from(name.as_str()),
                    SeaValue::from(ctx.now.as_str()),
                    SeaValue::from(ctx.company_id.as_str()),
                ],
            ),
            setting_statement(&ctx.company_id, PROFILE_KEY, &stored, &ctx.now),
        ];
        let mut view = company_view(db, &ctx.company_id).await?;
        view.name = name;
        view.profile = profile;
        Ok(Plan {
            statements,
            output: view,
            objects: vec![ctx.company_id.clone()],
        })
    }
}

/// `tax_settings.get`: the small-business and VAT-basis settings, and the
/// rates the company's pack has for its country today.
pub struct TaxSettingsGet;

/// The company's own rates as stored, empty when there are none.
pub async fn user_rates(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<UserRate>, CommandError> {
    match setting(db, company_id, USER_RATES_KEY)
        .await
        .map_err(CommandError::Database)?
    {
        Some(text) => serde_json::from_str(&text)
            .map_err(|err| CommandError::InvalidInput(format!("stored tax rates: {err}"))),
        None => Ok(Vec::new()),
    }
}

/// The pack that applies to a country: its own pack, or the generic pack.
/// `None` when the country is empty.
pub fn pack_for(country: &str) -> Result<Option<CountryPack>, CommandError> {
    if country.is_empty() {
        return Ok(None);
    }
    let found = country::pack_for_country(country)
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    match found {
        Some(pack) => Ok(Some(pack)),
        None => country::generic_pack()
            .map(Some)
            .map_err(|err| CommandError::InvalidInput(err.to_string())),
    }
}

/// Converts the company's own rates into dated rates. Each rate runs from its
/// `valid_from` to the day before the next rate of the same category.
pub fn company_dated_rates(
    country: &str,
    rates: &[UserRate],
) -> Result<Vec<DatedRate>, CommandError> {
    let jurisdiction = Jurisdiction::new(country);
    let mut out = Vec::with_capacity(rates.len());
    for rate in rates {
        let category = RateCategory::from_code(&rate.category)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let valid_from = Date::parse(
            &rate.valid_from,
            format_description!("[year]-[month]-[day]"),
        )
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        out.push(DatedRate {
            jurisdiction: jurisdiction.clone(),
            category,
            rate: country::percent_rate(&rate.percent)
                .map_err(|err| CommandError::InvalidInput(err.to_string()))?,
            valid_from,
            valid_to: None,
            source: rate.source.clone(),
        });
    }
    for idx in 0..out.len() {
        let category = out[idx].category;
        let from = out[idx].valid_from;
        let end = out
            .iter()
            .filter(|other| other.category == category && other.valid_from > from)
            .map(|other| other.valid_from)
            .min()
            .and_then(time::Date::previous_day);
        out[idx].valid_to = end;
    }
    Ok(out)
}

/// The tax settings view for a company.
pub async fn tax_view(
    db: &dyn Database,
    company_id: &str,
    today: Date,
) -> Result<TaxSettingsOutput, CommandError> {
    let settings = ledgers_tax::settings::tax_settings(db, company_id)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    let country = profile(db, company_id)
        .await
        .map_err(CommandError::Database)?
        .country;
    let stored = user_rates(db, company_id).await?;
    let Some(pack) = pack_for(&country)? else {
        return Ok(TaxSettingsOutput {
            small_business: settings.small_business,
            vat_basis: settings.vat_basis.code().to_owned(),
            country,
            rate_table: false,
            rates: Vec::new(),
            pack: String::new(),
            pack_name: String::new(),
            regime: String::new(),
            user_rates_allowed: false,
            rates_status: String::new(),
            registration_label: String::new(),
            registration_note: String::new(),
            returns: Vec::new(),
            tax_codes: Vec::new(),
            user_rates: Vec::new(),
        });
    };
    let company_rates = if pack.user_rates {
        company_dated_rates(&country, &stored)?
    } else {
        Vec::new()
    };
    let table = pack
        .rate_table(&company_rates)
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    let jurisdiction = Jurisdiction::new(&country);
    let mut rates = Vec::new();
    for category in [RateCategory::Standard, RateCategory::Reduced] {
        if let Ok(dated) = table.rate_on(&jurisdiction, category, today) {
            rates.push(TaxRateView {
                category: category.code().to_owned(),
                percent: percent_text(&dated.rate.to_string()),
                valid_from: dated.valid_from.to_string(),
                source: dated.source.clone(),
            });
        }
    }
    let returns = pack
        .returns
        .iter()
        .map(|layout| match layout {
            country::ReturnLayout::Generic => ReturnView {
                id: "generic".to_owned(),
                label: "Generic VAT/GST return".to_owned(),
            },
            country::ReturnLayout::Ustva => ReturnView {
                id: "ustva".to_owned(),
                label: "Umsatzsteuer-Voranmeldung (UStVA)".to_owned(),
            },
            country::ReturnLayout::Boxes { id, label, .. } => ReturnView {
                id: id.clone(),
                label: label.clone(),
            },
        })
        .collect();
    let tax_codes = pack
        .tax_codes
        .iter()
        .map(|code| TaxCodeView {
            code: code.code.clone(),
            label: code.label.clone(),
            kind: code.kind.to_string(),
            category: code.category.code().to_owned(),
        })
        .collect();
    Ok(TaxSettingsOutput {
        small_business: settings.small_business,
        vat_basis: settings.vat_basis.code().to_owned(),
        country,
        rate_table: !pack.rates.is_empty() || pack.user_rates,
        rates,
        pack: pack.id.clone(),
        pack_name: pack.name.clone(),
        regime: pack.regime.code().to_owned(),
        user_rates_allowed: pack.user_rates,
        rates_status: if pack.rates_pending {
            "pending"
        } else {
            "complete"
        }
        .to_owned(),
        registration_label: pack.registration.label.clone(),
        registration_note: pack.registration.note.clone(),
        returns,
        tax_codes,
        user_rates: stored,
    })
}

/// A decimal fraction ("0.19") as a percentage text ("19"), exactly: the
/// decimal point moves two places right, with no rounding.
fn percent_text(fraction: &str) -> String {
    let (whole, frac) = fraction.split_once('.').unwrap_or((fraction, ""));
    let frac_full = format!("{frac:0<2}");
    let head_raw = format!("{whole}{}", &frac_full[..2]);
    let head = head_raw.trim_start_matches('0');
    let head = if head.is_empty() { "0" } else { head };
    let tail = frac_full[2..].trim_end_matches('0');
    if tail.is_empty() {
        head.to_owned()
    } else {
        format!("{head}.{tail}")
    }
}

#[async_trait]
impl Command for TaxSettingsGet {
    const NAME: &'static str = "tax_settings.get";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = Value;
    type Output = TaxSettingsOutput;

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
        let today = today_of(ctx)?;
        Ok(Plan::with_output(
            tax_view(db, &ctx.company_id, today).await?,
        ))
    }
}

/// The date of the command's timestamp (UTC).
fn today_of(ctx: &CommandContext) -> Result<Date, CommandError> {
    Date::parse(
        &ctx.now[..10.min(ctx.now.len())],
        format_description!("[year]-[month]-[day]"),
    )
    .map_err(|err| CommandError::InvalidInput(format!("bad command timestamp: {err}")))
}

/// Validates the tax input.
///
/// # Errors
/// [`CommandError::InvalidInput`] for an unknown VAT basis.
pub fn clean_tax(input: &TaxSettingsInput) -> Result<TaxSettings, CommandError> {
    let vat_basis = VatBasis::from_code(input.vat_basis.trim())
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(TaxSettings {
        small_business: input.small_business,
        vat_basis,
    })
}

/// `numbering.list`: every journal with its number sequence in each fiscal year.
pub struct NumberingList;

#[async_trait]
impl Command for NumberingList {
    const NAME: &'static str = "numbering.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = Value;
    type Output = NumberingListOutput;

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
        let company = ctx.company_id.as_str();
        let journal_rows = db
            .query(&Statement::with_values(
                "SELECT id, code, name, kind FROM journal WHERE company_id = ? ORDER BY code"
                    .to_owned(),
                vec![SeaValue::from(company)],
            ))
            .await
            .map_err(CommandError::Database)?;
        let year_rows = db
            .query(&Statement::with_values(
                "SELECT label FROM fiscal_year WHERE company_id = ? ORDER BY label".to_owned(),
                vec![SeaValue::from(company)],
            ))
            .await
            .map_err(CommandError::Database)?;
        let counter_rows = db
            .query(&Statement::with_values(
                "SELECT n.journal_id, n.fiscal_year, n.next_number FROM numbering_counter n \
                 JOIN journal j ON j.id = n.journal_id WHERE j.company_id = ?"
                    .to_owned(),
                vec![SeaValue::from(company)],
            ))
            .await
            .map_err(CommandError::Database)?;
        let entry_rows = db
            .query(&Statement::with_values(
                "SELECT journal_id, fiscal_year, COUNT(*) AS entries FROM journal_entry \
                 WHERE company_id = ? GROUP BY journal_id, fiscal_year"
                    .to_owned(),
                vec![SeaValue::from(company)],
            ))
            .await
            .map_err(CommandError::Database)?;
        let years: Vec<i64> = year_rows
            .rows
            .iter()
            .filter_map(|row| row.get::<i64>("label"))
            .collect();
        let counters = pair_map(&counter_rows.rows, "next_number");
        let entries = pair_map(&entry_rows.rows, "entries");
        let journals = journal_rows
            .rows
            .iter()
            .map(|row| {
                let id = row.get::<String>("id").unwrap_or_default();
                let sequences = years
                    .iter()
                    .map(|&year| {
                        let key = (id.clone(), year);
                        let issued = entries.get(&key).copied().unwrap_or(0);
                        SequenceView {
                            fiscal_year: year,
                            next_number: counters.get(&key).copied().unwrap_or(0) + 1,
                            entries: issued,
                            editable: issued == 0,
                        }
                    })
                    .collect();
                JournalNumberingView {
                    code: row.get::<String>("code").unwrap_or_default(),
                    name: row.get::<String>("name").unwrap_or_default(),
                    kind: row.get::<String>("kind").unwrap_or_default(),
                    id,
                    sequences,
                }
            })
            .collect();
        Ok(Plan::with_output(NumberingListOutput { journals }))
    }
}

/// Maps `(journal_id, fiscal_year)` to an integer column of the rows.
fn pair_map(rows: &[cratefield_core::Row], column: &str) -> HashMap<(String, i64), i64> {
    rows.iter()
        .filter_map(|row| {
            let journal = row.get::<String>("journal_id")?;
            let year = row.get::<i64>("fiscal_year")?;
            let value = row.get::<i64>(column)?;
            Some(((journal, year), value))
        })
        .collect()
}

/// `numbering.set_next`: moves a sequence's next number forward, while no
/// number in it has been issued. Once an entry uses the sequence, its numbers
/// are fixed: moving them would leave a gap.
pub struct NumberingSetNext;

#[async_trait]
impl Command for NumberingSetNext {
    const NAME: &'static str = "numbering.set_next";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = NumberingSetNextInput;
    type Output = NumberingSetNextInput;

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
        let company = ctx.company_id.as_str();
        let journal_known = !db
            .query(&Statement::with_values(
                "SELECT id FROM journal WHERE company_id = ? AND id = ?".to_owned(),
                vec![
                    SeaValue::from(company),
                    SeaValue::from(input.journal_id.as_str()),
                ],
            ))
            .await
            .map_err(CommandError::Database)?
            .is_empty();
        if !journal_known {
            return Err(CommandError::InvalidInput(
                "no journal with that id in this company".to_owned(),
            ));
        }
        let year_known = period::fiscal_year(db, company, input.fiscal_year)
            .await
            .map_err(|err| domain(&err))?
            .is_some();
        if !year_known {
            return Err(CommandError::InvalidInput(format!(
                "fiscal year {} is not open",
                input.fiscal_year
            )));
        }
        let issued = entry_count(db, &input.journal_id, input.fiscal_year).await?;
        if issued > 0 {
            return Err(CommandError::InvalidInput(
                "numbers have already been issued in this journal and year; moving them would leave a gap".to_owned(),
            ));
        }
        let current = ledgers_db::numbering::read_counter(db, &input.journal_id, input.fiscal_year)
            .await
            .map_err(CommandError::Database)?
            .unwrap_or(0);
        if input.next_number <= current + 1 {
            return Err(CommandError::InvalidInput(format!(
                "the next number can only move forward; it is {} now",
                current + 1
            )));
        }
        let last = input.next_number - 1;
        let statements = vec![
            ledgers_db::numbering::provision_statement(&input.journal_id, input.fiscal_year),
            Statement::with_values(
                "UPDATE numbering_counter SET next_number = ? WHERE journal_id = ? AND fiscal_year = ?".to_owned(),
                vec![
                    SeaValue::BigInt(Some(last)),
                    SeaValue::from(input.journal_id.as_str()),
                    SeaValue::BigInt(Some(input.fiscal_year)),
                ],
            ),
            ledgers_db::numbering::assert_counter_advanced_statement(
                &format!("numbering.set_next.{}", ctx.event_id),
                &input.journal_id,
                input.fiscal_year,
                last,
            ),
            Statement::with_values(
                "INSERT INTO ledger_guard (guard_key, ok) SELECT ?, CASE WHEN \
                 (SELECT COUNT(*) FROM journal_entry WHERE journal_id = ? AND fiscal_year = ?) = 0 \
                 THEN 1 ELSE NULL END"
                    .to_owned(),
                vec![
                    SeaValue::from(format!("numbering.no_entries.{}", ctx.event_id)),
                    SeaValue::from(input.journal_id.as_str()),
                    SeaValue::BigInt(Some(input.fiscal_year)),
                ],
            ),
        ];
        Ok(Plan {
            statements,
            output: input.clone(),
            objects: vec![input.journal_id.clone(), input.fiscal_year.to_string()],
        })
    }
}

/// Entries posted in one journal and fiscal year.
async fn entry_count(
    db: &dyn Database,
    journal_id: &str,
    fiscal_year: i64,
) -> Result<i64, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS entries FROM journal_entry WHERE journal_id = ? AND fiscal_year = ?".to_owned(),
            vec![SeaValue::from(journal_id), SeaValue::BigInt(Some(fiscal_year))],
        ))
        .await
        .map_err(CommandError::Database)?;
    Ok(rows
        .first()
        .and_then(|row| row.get::<i64>("entries"))
        .unwrap_or(0))
}

/// `fiscal_years.list`: every fiscal year and its periods.
pub struct FiscalYearsList;

#[async_trait]
impl Command for FiscalYearsList {
    const NAME: &'static str = "fiscal_years.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = Value;
    type Output = FiscalYearsListOutput;

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
        let years = fiscal_years(db, &ctx.company_id)
            .await
            .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(FiscalYearsListOutput { years }))
    }
}

/// `approval_settings.get`: the low-risk limits, the request expiry, and the
/// fixed sign-in window.
pub struct ApprovalSettingsGet;

#[async_trait]
impl Command for ApprovalSettingsGet {
    const NAME: &'static str = "approval_settings.get";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = Value;
    type Output = ApprovalSettingsView;

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
        let rules = approval_rules(db, &ctx.company_id)
            .await
            .map_err(CommandError::Database)?;
        Ok(Plan::with_output(approval_view(rules)))
    }
}

/// The input of `approval_settings.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalSettingsInput {
    /// Per-currency low-risk limits, in canonical amounts.
    pub low_risk_limits: BTreeMap<String, String>,
    /// The request expiry in hours.
    pub request_ttl_hours: u32,
}

/// `approval_settings.update`: writes the approval rules in the audited batch.
pub struct ApprovalSettingsUpdate;

#[async_trait]
impl Command for ApprovalSettingsUpdate {
    const NAME: &'static str = "approval_settings.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = ApprovalSettingsInput;
    type Output = ApprovalSettingsView;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        clean_approval(input)?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (low, ttl) = clean_approval(input)?;
        let limits = serde_json::to_string(&low)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let statements = vec![
            setting_statement(&ctx.company_id, LOW_RISK_KEY, &limits, &ctx.now),
            setting_statement(&ctx.company_id, REQUEST_TTL_KEY, &ttl.to_string(), &ctx.now),
        ];
        Ok(Plan {
            statements,
            output: approval_view(ApprovalRules {
                low_risk: low,
                ttl_hours: ttl,
            }),
            objects: vec![ctx.company_id.clone()],
        })
    }
}

/// Validates the approval rules.
///
/// # Errors
/// [`CommandError::InvalidInput`] naming the first value that does not fit.
pub fn clean_approval(
    input: &ApprovalSettingsInput,
) -> Result<(BTreeMap<String, String>, u32), CommandError> {
    let low = rules::low_risk_limits(&input.low_risk_limits).map_err(CommandError::InvalidInput)?;
    let ttl =
        rules::request_ttl_hours(input.request_ttl_hours).map_err(CommandError::InvalidInput)?;
    Ok((low, ttl))
}

/// `audit.list`: the audit events, newest first, filtered and paged.
///
/// Filters on the date and sequence are applied in the query. The actor,
/// command and outcome live in the stored event, so they are matched after
/// each chunk is read. A page scans at most [`AUDIT_MAX_CHUNKS`] chunks; when
/// it stops early it returns the cursor to continue from.
pub struct AuditList;

#[async_trait]
impl Command for AuditList {
    const NAME: &'static str = "audit.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = AuditListInput;
    type Output = AuditListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        audit_filters(input)?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let filters = audit_filters(input)?;
        let limit = input
            .limit
            .unwrap_or(DEFAULT_AUDIT_LIMIT)
            .clamp(1, MAX_AUDIT_LIMIT) as usize;
        // The scan position: `None` until the first row is read, unless the
        // caller named a cursor. The first page has no cursor, so it binds no
        // sequence number at all (a sentinel such as `i64::MAX` cannot be bound
        // on D1).
        let mut cursor = input.before_seq;
        let mut events = Vec::new();
        let mut exhausted = false;
        let mut chunks = 0;
        while events.len() < limit && chunks < AUDIT_MAX_CHUNKS {
            chunks += 1;
            let mut sql = String::from(
                "SELECT seq, payload, created_at FROM audit_event WHERE company_id = ?",
            );
            let mut values = vec![SeaValue::from(ctx.company_id.as_str())];
            if let Some(before) = cursor {
                sql.push_str(" AND seq < ?");
                values.push(SeaValue::BigInt(Some(before)));
            }
            if let Some(from) = &filters.from {
                sql.push_str(" AND created_at >= ?");
                values.push(SeaValue::from(from.as_str()));
            }
            if let Some(before) = &filters.before {
                sql.push_str(" AND created_at < ?");
                values.push(SeaValue::from(before.as_str()));
            }
            sql.push_str(" ORDER BY seq DESC LIMIT ?");
            values.push(SeaValue::BigInt(Some(
                i64::try_from(AUDIT_CHUNK).unwrap_or(i64::MAX),
            )));
            let rows = db
                .query(&Statement::with_values(sql, values))
                .await
                .map_err(CommandError::Database)?;
            if rows.rows.is_empty() {
                exhausted = true;
                break;
            }
            let fetched = rows.rows.len();
            for row in &rows.rows {
                let seq = row.get::<i64>("seq").unwrap_or(0);
                cursor = Some(seq);
                let payload = row.get::<String>("payload").unwrap_or_default();
                let event: AuditEvent = serde_json::from_str(&payload).map_err(|err| {
                    CommandError::Database(DbError::Query(format!(
                        "audit event {seq} does not parse: {err}"
                    )))
                })?;
                if filters.matches(&event) {
                    events.push(view_of(seq, event));
                    if events.len() == limit {
                        break;
                    }
                }
            }
            if fetched < AUDIT_CHUNK && events.len() < limit {
                exhausted = true;
                break;
            }
        }
        // The cursor is the scan position, not the last match: a page that scanned
        // its whole budget without a match still tells the caller where to go on.
        let next_before_seq = if exhausted { None } else { cursor };
        Ok(Plan::with_output(AuditListOutput {
            events,
            next_before_seq,
        }))
    }
}

/// The audit filters after validation.
struct AuditFilters {
    actor_kind: Option<String>,
    command: Option<String>,
    outcome: Option<String>,
    from: Option<String>,
    before: Option<String>,
}

impl AuditFilters {
    fn matches(&self, event: &AuditEvent) -> bool {
        let kind_ok = self
            .actor_kind
            .as_deref()
            .is_none_or(|kind| match &event.actor {
                ledgers_audit::Actor::Person { .. } => kind == "person",
                ledgers_audit::Actor::Agent { .. } => kind == "agent",
            });
        let command_ok = self
            .command
            .as_deref()
            .is_none_or(|name| event.command == name);
        let outcome_ok = self
            .outcome
            .as_deref()
            .is_none_or(|name| outcome_text(event.outcome) == name);
        kind_ok && command_ok && outcome_ok
    }
}

fn outcome_text(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Allowed => "allowed",
        Outcome::NeedsApproval => "needs-approval",
        Outcome::Denied => "denied",
        Outcome::Error => "error",
    }
}

fn audit_filters(input: &AuditListInput) -> Result<AuditFilters, CommandError> {
    let date = |text: &str| -> Result<Date, CommandError> {
        Date::parse(text, format_description!("[year]-[month]-[day]"))
            .map_err(|_| CommandError::InvalidInput(format!("{text:?} is not a date, YYYY-MM-DD")))
    };
    let actor_kind = match input.actor_kind.as_deref() {
        None | Some("") => None,
        Some(kind @ ("person" | "agent")) => Some(kind.to_owned()),
        Some(other) => {
            return Err(CommandError::InvalidInput(format!(
                "actor_kind must be person or agent, got {other:?}"
            )));
        }
    };
    let outcome = match input.outcome.as_deref() {
        None | Some("") => None,
        Some(name @ ("allowed" | "needs-approval" | "denied" | "error")) => Some(name.to_owned()),
        Some(other) => {
            return Err(CommandError::InvalidInput(format!(
                "unknown outcome {other:?}"
            )));
        }
    };
    if let Some(before) = input.before_seq
        && !(0..=MAX_CURSOR_SEQ).contains(&before)
    {
        return Err(CommandError::InvalidInput(format!(
            "before_seq must be between 0 and {MAX_CURSOR_SEQ}, got {before}"
        )));
    }
    let from = match input.from.as_deref() {
        None | Some("") => None,
        Some(text) => Some(format!("{}T00:00:00", date(text)?)),
    };
    let before = match input.to.as_deref() {
        None | Some("") => None,
        Some(text) => {
            let day = date(text)?;
            let next = day.next_day().ok_or_else(|| {
                CommandError::InvalidInput("the to date is out of range".to_owned())
            })?;
            Some(format!("{next}T00:00:00"))
        }
    };
    if let (Some(from), Some(before)) = (&from, &before)
        && from >= before
    {
        return Err(CommandError::InvalidInput(
            "from must be on or before to".to_owned(),
        ));
    }
    Ok(AuditFilters {
        actor_kind,
        command: input.command.clone().filter(|name| !name.is_empty()),
        outcome,
        from,
        before,
    })
}

fn view_of(seq: i64, event: AuditEvent) -> AuditEventView {
    let (actor_kind, actor_id) = match event.actor {
        ledgers_audit::Actor::Person { person_id } => ("person".to_owned(), person_id),
        ledgers_audit::Actor::Agent { key_id, .. } => ("agent".to_owned(), key_id),
    };
    AuditEventView {
        id: event.id,
        seq,
        at: event.at,
        actor_kind,
        actor_id,
        command: event.command,
        outcome: outcome_text(event.outcome).to_owned(),
        objects: event.objects,
        reason: event.reason,
        request_id: event.request_id,
    }
}

/// The person-path commands: they call domain functions that own their audit
/// row, so they authenticate first and then hand over the person's context.
impl ServerDispatch {
    /// `fiscal_years.create`: opens a calendar year (January to December).
    pub(super) async fn fiscal_years_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, cratefield_core::Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("fiscal_years.create"))
            .await?;
        let body: FiscalYearCreateInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let audit = person_audit(deps, &person, request_id);
        let now = super::rfc3339_utc(deps.clock.now());
        let spec = FiscalYearSpec {
            label: body.label,
            start_month: 1,
            has_adjustment_period: false,
        };
        period::open_fiscal_year(deps.db.as_ref(), &self.company_id, spec, now, &audit)
            .await
            .map_err(|err| fail(domain(&err)))?;
        let years = fiscal_years(deps.db.as_ref(), &self.company_id)
            .await
            .map_err(|err| fail(domain(&err)))?;
        let year = years
            .into_iter()
            .find(|year| year.label == body.label)
            .ok_or_else(|| {
                fail(CommandError::InvalidInput(
                    "the year was not stored".to_owned(),
                ))
            })?;
        serde_json::to_value(year).map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    /// `fiscal_years.close`: runs the year-end close and reports the view after it.
    pub(super) async fn fiscal_years_close(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, cratefield_core::Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("fiscal_years.close"))
            .await?;
        let body: FiscalYearCloseInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let audit = person_audit(deps, &person, request_id);
        let now = super::rfc3339_utc(deps.clock.now());
        let checks = close_checks();
        let year_before = period::fiscal_year(deps.db.as_ref(), &self.company_id, body.label)
            .await
            .map_err(|err| fail(domain(&err)))?
            .ok_or_else(|| {
                fail(CommandError::InvalidInput(format!(
                    "fiscal year {} does not exist",
                    body.label
                )))
            })?;
        if year_before.year_end_closed_at.is_some() {
            return Err(fail(CommandError::InvalidInput(format!(
                "fiscal year {} is already closed",
                body.label
            ))));
        }
        // The year-end close needs every month but the last closed; the last
        // (December) stays open or soft-closed to take the closing entry. Each
        // month is locked as its own audited step, soft-closed then closed, so
        // a blocker on one month stops the year before it closes.
        let months = period::fiscal_year_periods(deps.db.as_ref(), &self.company_id, body.label)
            .await
            .map_err(|err| fail(domain(&err)))?;
        let (_, earlier) = months.split_last().ok_or_else(|| {
            fail(CommandError::InvalidInput(format!(
                "fiscal year {} has no months",
                body.label
            )))
        })?;
        for (month, state) in earlier {
            if *state == PeriodState::Open {
                period::soft_close_period(
                    deps.db.as_ref(),
                    &self.company_id,
                    month,
                    now.clone(),
                    &audit,
                )
                .await
                .map_err(|err| fail(domain(&err)))?;
            }
            if *state != PeriodState::Closed {
                period::close_period(
                    deps.db.as_ref(),
                    &self.company_id,
                    month,
                    &EntryJournal,
                    &checks,
                    None,
                    now.clone(),
                    &audit,
                )
                .await
                .map_err(|err| fail(domain(&err)))?;
            }
        }
        let report = period::close_fiscal_year(
            deps.db.as_ref(),
            &self.company_id,
            body.label,
            &EntryJournal,
            &checks,
            None,
            now,
            &audit,
        )
        .await
        .map_err(|err| fail(domain(&err)))?;
        let year = fiscal_years(deps.db.as_ref(), &self.company_id)
            .await
            .map_err(|err| fail(domain(&err)))?
            .into_iter()
            .find(|year| year.label == body.label)
            .ok_or_else(|| {
                fail(CommandError::InvalidInput(
                    "the year was not found".to_owned(),
                ))
            })?;
        let output = FiscalYearCloseOutput {
            year,
            closing_entry_id: report.closing_entry.map(|entry| entry.entry_id),
        };
        serde_json::to_value(output)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    /// `periods.close`: locks one month against postings.
    pub(super) async fn periods_close(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, cratefield_core::Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("periods.close"))
            .await?;
        let body: PeriodCloseInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let period_id = parse_period(&body.period).map_err(fail)?;
        let audit = person_audit(deps, &person, request_id);
        let now = super::rfc3339_utc(deps.clock.now());
        let checks = close_checks();
        let state = period::close_period(
            deps.db.as_ref(),
            &self.company_id,
            &period_id,
            &EntryJournal,
            &checks,
            None,
            now,
            &audit,
        )
        .await
        .map_err(|err| fail(domain(&err)))?;
        serde_json::to_value(PeriodView {
            period: period_id,
            state: state.as_str().to_owned(),
        })
        .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    /// `periods.reopen`: reopens a locked month, with the reason in the audit trail.
    pub(super) async fn periods_reopen(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, cratefield_core::Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("periods.reopen"))
            .await?;
        let body: PeriodReopenInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let period_id = parse_period(&body.period).map_err(fail)?;
        let audit = person_audit(deps, &person, request_id);
        let now = super::rfc3339_utc(deps.clock.now());
        let state = period::reopen_period(
            deps.db.as_ref(),
            &self.company_id,
            &period_id,
            &body.reason,
            now,
            &audit,
        )
        .await
        .map_err(|err| fail(domain(&err)))?;
        serde_json::to_value(PeriodView {
            period: period_id,
            state: state.as_str().to_owned(),
        })
        .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    /// `tax_settings.update`: writes the tax settings with their audit row.
    pub(super) async fn tax_settings_update(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, cratefield_core::Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("tax_settings.update"))
            .await?;
        let body: TaxSettingsInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let settings = clean_tax(&body).map_err(fail)?;
        let audit = person_audit(deps, &person, request_id);
        let at = super::rfc3339_utc(deps.clock.now());
        let audit_config = ledgers_audit::AuditConfig::default();
        ledgers_tax::settings::set_tax_settings(
            deps.db.as_ref(),
            &self.company_id,
            &settings,
            &at,
            &audit,
            &audit_config,
        )
        .await
        .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        if let Some(rates) = &body.user_rates {
            let stored = clean_user_rates(&self.company_id, deps.db.as_ref(), rates)
                .await
                .map_err(fail)?;
            let json = serde_json::to_string(&stored)
                .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
            ledgers_tax::settings::set_user_rates(
                deps.db.as_ref(),
                &self.company_id,
                &json,
                &at,
                &audit,
                &audit_config,
            )
            .await
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        }
        let today = Date::parse(&at[..10], format_description!("[year]-[month]-[day]"))
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let view = tax_view(deps.db.as_ref(), &self.company_id, today)
            .await
            .map_err(fail)?;
        serde_json::to_value(view).map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }
}

/// Validates the company's own rates for its pack and returns them cleaned.
/// Only a pack that allows company rates accepts them; the rates must form a
/// sequence with no gaps or overlaps once dated.
pub async fn clean_user_rates(
    company_id: &str,
    db: &dyn Database,
    rates: &[UserRate],
) -> Result<Vec<UserRate>, CommandError> {
    let country = profile(db, company_id)
        .await
        .map_err(CommandError::Database)?
        .country;
    let pack = pack_for(&country)?.ok_or_else(|| {
        CommandError::InvalidInput("Choose the company country before entering rates.".to_owned())
    })?;
    if !pack.user_rates {
        return Err(CommandError::InvalidInput(format!(
            "{} uses statutory rates; company rates are only for packs that allow them.",
            pack.name
        )));
    }
    let mut cleaned = Vec::with_capacity(rates.len());
    for rate in rates {
        let category = RateCategory::from_code(rate.category.trim())
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let source = rate.source.trim();
        if source.is_empty() {
            return Err(CommandError::InvalidInput(
                "Each rate needs a source note.".to_owned(),
            ));
        }
        let percent = rate.percent.trim().to_owned();
        country::percent_rate(&percent)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let valid_from = rate.valid_from.trim().to_owned();
        Date::parse(&valid_from, format_description!("[year]-[month]-[day]")).map_err(|_| {
            CommandError::InvalidInput(format!("{valid_from:?} is not a YYYY-MM-DD date"))
        })?;
        cleaned.push(UserRate {
            category: category.code().to_owned(),
            percent,
            valid_from,
            source: source.to_owned(),
        });
    }
    let dated = company_dated_rates(&country, &cleaned)?;
    pack.rate_table(&dated)
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(cleaned)
}

/// The audit context a person's domain write records under.
fn person_audit(deps: &Deps, person: &str, request_id: &str) -> AuditContext {
    AuditContext::new(
        ledgers_audit::Actor::Person {
            person_id: person.to_owned(),
        },
        request_id.to_owned(),
        deps.id_gen.ulid(),
    )
}

/// The year-end and period close checks the server runs: the same registry
/// `LedgersConfig::open` builds.
#[must_use]
pub fn close_checks() -> CloseCheckRegistry {
    let mut checks = CloseCheckRegistry::new();
    checks.register(Box::new(DraftEntriesCloseCheck));
    checks
}

/// Reads the company's approval rules for a write, or the defaults for a read.
pub(super) async fn approval_rules_for<C: Command>(
    db: &dyn Database,
    company_id: &str,
) -> Result<ApprovalRules, DbError> {
    if C::TIER == Tier::Read {
        Ok(ApprovalRules::defaults())
    } else {
        approval_rules(db, company_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_text_shifts_the_decimal_point() {
        assert_eq!(percent_text("0.19"), "19");
        assert_eq!(percent_text("0.07"), "7");
        assert_eq!(percent_text("0.055"), "5.5");
        assert_eq!(percent_text("1"), "100");
        assert_eq!(percent_text("0"), "0");
    }

    #[test]
    fn periods_are_year_and_month() {
        assert_eq!(parse_period("2026-03").unwrap(), "2026-03");
        assert!(parse_period("2026-3").is_err());
        assert!(parse_period("2026-13").is_err());
        assert!(parse_period("26-03-1").is_err());
    }

    #[test]
    fn tax_basis_codes_are_checked() {
        let ok = clean_tax(&TaxSettingsInput {
            user_rates: None,
            small_business: false,
            vat_basis: "cash".to_owned(),
        });
        assert!(ok.is_ok());
        let bad = clean_tax(&TaxSettingsInput {
            user_rates: None,
            small_business: false,
            vat_basis: "monthly".to_owned(),
        });
        assert!(matches!(bad, Err(CommandError::InvalidInput(_))));
    }
}
