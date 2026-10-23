//! Parties master data: customers and vendors (issue #44).
//!
//! The `vendor` table is the one party record. Bills, bank matching and
//! parties all read it; a vendor row made by a bill extraction is a party
//! with `roles = 'vendor'` and `source = 'extraction'`, and migration 0004
//! keeps every existing row as it was.
//!
//! - `parties.list`: unmerged parties, with their latest VIES result and
//!   the other parties they may duplicate (same VAT ID, IBAN or name).
//! - `parties.create` / `parties.update`: master data. The VAT ID is checked
//!   by the country pack's validator (`ledgers_tax::check_vat_id`). Agents may
//!   use these.
//! - `parties.set_bank`: IBAN and BIC with an append-only history. Human only.
//! - `parties.check_vat`: a VIES lookup of the party's VAT ID, behind the
//!   [`ViesPort`]. The server supplies a port only when `LEDGERS_VIES=1`.
//! - `parties.merge`: folds one party into another and repoints its bills.
//!   Human only.

use crate::iban;
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard;
use ledgers_extract::llm::Transport;
use ledgers_tax::{check_vat_id, normalize_vat_id, pack_by_id};
use schemars::JsonSchema;
use sea_query::Value;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// The VIES REST endpoint `parties.check_vat` calls through [`HttpVies`].
pub const VIES_URL: &str = "https://ec.europa.eu/taxation_customs/vies/rest-api/check-vat-number";

/// The most parties one list call returns.
const MAX_LIST_ROWS: u32 = 200;
/// The rows one list call returns when the caller names no limit.
const DEFAULT_LIST_ROWS: u32 = 50;

/// The columns a party read selects.
const VENDOR_COLUMNS: &str = "id, name, vat_id, iban, bic, roles, merged_into, personal_data, needs_review, tax_number, language, currency, payment_terms_days, default_account, default_tax_code, addresses, contacts";

// ===================================================================== shared rules

/// Lower-cases a name and keeps letters and digits only, so `Acme GmbH.` and
/// `acme gmbh` name the same party.
#[must_use]
pub fn normalise_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A party as bill matching sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchCandidate {
    /// The party id.
    pub id: String,
    /// The party's name.
    pub name: String,
    /// The party's VAT ID, normalised, when it has one.
    pub vat_id: Option<String>,
}

/// The party a vendor on a bill is: the one with the same VAT ID, otherwise
/// the one with the same normalised name. `None` means a new vendor.
#[must_use]
pub fn match_party<'a>(
    candidates: &'a [MatchCandidate],
    vat_id: Option<&str>,
    name: Option<&str>,
) -> Option<&'a MatchCandidate> {
    let wanted_vat = vat_id.map(normalize_vat_id).filter(|v| !v.is_empty());
    if let Some(vat) = &wanted_vat
        && let Some(found) = candidates
            .iter()
            .find(|c| c.vat_id.as_deref() == Some(vat.as_str()))
    {
        return Some(found);
    }
    let wanted_name = normalise_name(name?);
    if wanted_name.is_empty() {
        return None;
    }
    candidates
        .iter()
        .find(|c| normalise_name(&c.name) == wanted_name)
}

/// The country a VAT ID starts with, when it starts with two letters.
fn vat_country(vat: &str) -> Option<&str> {
    let prefix = vat.get(..2)?;
    prefix
        .bytes()
        .all(|b| b.is_ascii_alphabetic())
        .then_some(prefix)
}

/// Normalises a party VAT ID and checks it against its country's pack.
/// `Ok(None)` for an empty input (the party is not VAT registered).
fn checked_vat(raw: &str) -> Result<Option<String>, CommandError> {
    let normalised = normalize_vat_id(raw);
    if normalised.is_empty() {
        return Ok(None);
    }
    let country = vat_country(&normalised).ok_or_else(|| {
        CommandError::InvalidInput("a VAT ID starts with its two-letter country code".to_string())
    })?;
    check_vat_id(country, &normalised).map_err(CommandError::InvalidInput)?;
    Ok(Some(normalised))
}

/// Validates a role name the way the `vendor.roles` column does.
fn check_roles(roles: &str) -> Result<(), CommandError> {
    if ["customer", "vendor", "both"].contains(&roles) {
        Ok(())
    } else {
        Err(CommandError::InvalidInput(
            "roles must be 'customer', 'vendor', or 'both'".to_string(),
        ))
    }
}

fn check_terms(days: Option<i32>) -> Result<(), CommandError> {
    match days {
        Some(d) if d < 0 => Err(CommandError::InvalidInput(
            "payment_terms_days must be >= 0".to_string(),
        )),
        _ => Ok(()),
    }
}

/// A BIC is eight or eleven letters and digits.
fn checked_bic(raw: &str) -> Result<String, CommandError> {
    let bic = raw.trim().to_ascii_uppercase();
    let shape_ok =
        (bic.len() == 8 || bic.len() == 11) && bic.bytes().all(|b| b.is_ascii_alphanumeric());
    if shape_ok {
        Ok(bic)
    } else {
        Err(CommandError::InvalidInput(
            "a BIC is 8 or 11 letters and digits".to_string(),
        ))
    }
}

/// The party name, trimmed; a party needs one.
fn checked_name(raw: &str) -> Result<String, CommandError> {
    let name = raw.trim();
    if name.is_empty() {
        Err(CommandError::InvalidInput(
            "the party needs a name".to_string(),
        ))
    } else {
        Ok(name.to_string())
    }
}

/// The party a VAT ID already belongs to, so a refusal can name it. The
/// `UNIQUE (company_id, vat_id)` constraint stays the guard; this read only
/// words the refusal.
async fn vat_owner(
    db: &dyn Database,
    company_id: &str,
    vat: &str,
    except: Option<&str>,
) -> Result<Option<String>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id FROM vendor WHERE company_id = ? AND vat_id = ?".to_string(),
            vec![Value::from(company_id), Value::from(vat)],
        ))
        .await
        .map_err(|_| database_error())?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| row.get::<String>("id"))
        .find(|id| Some(id.as_str()) != except))
}

fn database_error() -> CommandError {
    CommandError::InvalidInput("database error".to_string())
}

fn optional_string_value(s: Option<&String>) -> Value {
    s.map_or(Value::String(None), |v| Value::from(v.as_str()))
}

/// Reads one unmerged-or-merged party of the company, if it exists.
async fn read_party(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<cratefield_core::Row>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND id = ?"),
            vec![Value::from(company_id), Value::from(id)],
        ))
        .await
        .map_err(|_| database_error())?;
    Ok(rows.rows.into_iter().next())
}

/// A text column as an optional string; blank or missing is `None`.
fn text_column(row: &cratefield_core::Row, column: &str) -> Option<String> {
    row.get::<Option<String>>(column)
        .flatten()
        .filter(|v| !v.trim().is_empty())
}

/// A text column that must be present (an id, a name, a role).
fn text_required(row: &cratefield_core::Row, column: &str) -> Result<String, CommandError> {
    row.get::<String>(column)
        .ok_or_else(|| CommandError::InvalidInput(format!("missing {column}")))
}

/// Trims a text field; blank is `None`.
fn optional_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A text column value: blank clears it to NULL.
fn text_value(raw: &str) -> Value {
    optional_text(Some(raw)).map_or(Value::String(None), |v| Value::from(v.as_str()))
}

/// A list stored as JSON text; an empty list is NULL.
fn json_column<T: Serialize>(items: &[T]) -> Result<Value, CommandError> {
    if items.is_empty() {
        return Ok(Value::String(None));
    }
    let text = serde_json::to_string(items).map_err(|_| database_error())?;
    Ok(Value::from(text.as_str()))
}

/// Reads a JSON list column; NULL is an empty list.
fn read_json_column<T: DeserializeOwned>(
    row: &cratefield_core::Row,
    column: &str,
) -> Result<Vec<T>, CommandError> {
    match row.get::<Option<String>>(column).flatten() {
        None => Ok(Vec::new()),
        Some(text) => serde_json::from_str(&text).map_err(|_| database_error()),
    }
}

fn party_view_from_row(row: &cratefield_core::Row) -> Result<PartyView, CommandError> {
    Ok(PartyView {
        id: text_required(row, "id")?,
        name: text_required(row, "name")?,
        vat_id: text_column(row, "vat_id"),
        iban: text_column(row, "iban"),
        bic: text_column(row, "bic"),
        roles: text_required(row, "roles")?,
        merged_into: text_column(row, "merged_into"),
        latest_vies: None,
        personal_data: row.get::<i64>("personal_data").unwrap_or(0) != 0,
        possible_duplicates: Vec::new(),
        needs_review: row.get::<i64>("needs_review").unwrap_or(0) != 0,
        tax_number: text_column(row, "tax_number"),
        language: text_column(row, "language"),
        currency: text_column(row, "currency"),
        payment_terms_days: row
            .get::<i64>("payment_terms_days")
            .and_then(|days| i32::try_from(days).ok()),
        default_account: text_column(row, "default_account"),
        default_tax_code: text_column(row, "default_tax_code"),
        addresses: read_json_column(row, "addresses")?,
        contacts: read_json_column(row, "contacts")?,
    })
}

/// A message for a refusal, without the variant name.
fn error_message(error: &CommandError) -> String {
    match error {
        CommandError::InvalidInput(message) => message.clone(),
        other => other.to_string(),
    }
}

/// The latest VIES result of a party, if it has been checked.
async fn latest_vies(
    db: &dyn Database,
    company_id: &str,
    party_id: &str,
) -> Result<Option<ViesResult>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT valid, consultation_number, checked_at FROM vies_check WHERE company_id = ? AND vendor_id = ? ORDER BY checked_at DESC LIMIT 1".to_string(),
            vec![Value::from(company_id), Value::from(party_id)],
        ))
        .await
        .map_err(|_| database_error())?;
    Ok(rows.rows.first().and_then(|row| {
        Some(ViesResult {
            valid: row.get::<i64>("valid").unwrap_or(0) != 0,
            consultation_number: row.get::<Option<String>>("consultation_number").flatten(),
            checked_at: row.get::<String>("checked_at")?,
        })
    }))
}

/// Every unmerged party of the company, newest first.
async fn unmerged_parties(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<PartyView>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND merged_into IS NULL ORDER BY created_at DESC, id"
            ),
            vec![Value::from(company_id)],
        ))
        .await
        .map_err(|_| database_error())?;
    rows.rows.iter().map(party_view_from_row).collect()
}

// ===================================================================== addresses and contacts

/// A postal address on a party.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Address {
    /// `billing`, `shipping` or `other` (optional).
    #[serde(default)]
    pub kind: Option<String>,
    /// The first line.
    pub line1: String,
    /// The second line (optional).
    #[serde(default)]
    pub line2: Option<String>,
    /// The postal code (optional).
    #[serde(default)]
    pub postal_code: Option<String>,
    /// The city.
    pub city: String,
    /// ISO 3166-1 alpha-2 country code.
    pub country: String,
}

/// A person to contact at a party.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    /// The contact's name.
    pub name: String,
    /// The email address (optional).
    #[serde(default)]
    pub email: Option<String>,
    /// The phone number (optional).
    #[serde(default)]
    pub phone: Option<String>,
    /// The contact's role at the party (optional).
    #[serde(default)]
    pub role: Option<String>,
}

/// Checks and tidies addresses: a first line, a city and a two-letter country
/// each; `kind` is `billing`, `shipping` or `other` when given.
/// # Errors
/// A refusal naming the first address that fails a check.
pub fn checked_addresses(addresses: &[Address]) -> Result<Vec<Address>, CommandError> {
    addresses
        .iter()
        .enumerate()
        .map(|(index, address)| {
            let number = index + 1;
            let line1 = address.line1.trim();
            let city = address.city.trim();
            let country = address.country.trim().to_ascii_uppercase();
            let kind = optional_text(address.kind.as_deref()).map(|k| k.to_ascii_lowercase());
            if line1.is_empty() {
                return Err(CommandError::InvalidInput(format!(
                    "address {number} needs its first line"
                )));
            }
            if city.is_empty() {
                return Err(CommandError::InvalidInput(format!(
                    "address {number} needs a city"
                )));
            }
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic()) {
                return Err(CommandError::InvalidInput(format!(
                    "address {number} needs a two-letter country code"
                )));
            }
            if let Some(k) = &kind
                && !["billing", "shipping", "other"].contains(&k.as_str())
            {
                return Err(CommandError::InvalidInput(format!(
                    "address {number} kind must be billing, shipping or other"
                )));
            }
            Ok(Address {
                kind,
                line1: line1.to_string(),
                line2: optional_text(address.line2.as_deref()),
                postal_code: optional_text(address.postal_code.as_deref()),
                city: city.to_string(),
                country,
            })
        })
        .collect()
}

/// Whether an email address has one `@`, no spaces and a dotted domain.
fn plausible_email(email: &str) -> bool {
    if email.chars().any(char::is_whitespace) {
        return false;
    }
    match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

/// Checks and tidies contacts: a name each, and a plausible email when given.
/// # Errors
/// A refusal naming the first contact that fails a check.
pub fn checked_contacts(contacts: &[Contact]) -> Result<Vec<Contact>, CommandError> {
    contacts
        .iter()
        .enumerate()
        .map(|(index, contact)| {
            let number = index + 1;
            let name = contact.name.trim();
            if name.is_empty() {
                return Err(CommandError::InvalidInput(format!(
                    "contact {number} needs a name"
                )));
            }
            let email = optional_text(contact.email.as_deref());
            if let Some(e) = &email
                && !plausible_email(e)
            {
                return Err(CommandError::InvalidInput(format!(
                    "contact {number} has an email address that does not look right"
                )));
            }
            Ok(Contact {
                name: name.to_string(),
                email,
                phone: optional_text(contact.phone.as_deref()),
                role: optional_text(contact.role.as_deref()),
            })
        })
        .collect()
}

// ===================================================================== list

/// The input of `parties.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Maximum rows to return; defaults to 50, at most 200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Number of matching rows to skip; defaults to 0.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Keep parties whose name, VAT ID or IBAN contains this text (case-insensitive).
    #[serde(default)]
    pub q: Option<String>,
    /// Keep only parties with exactly this role: 'customer', 'vendor' or 'both'.
    #[serde(default)]
    pub roles: Option<String>,
    /// Keep only parties that still need a person's review (true) or only reviewed ones (false).
    #[serde(default)]
    pub needs_review: Option<bool>,
}

/// A party, with its latest VIES result.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PartyView {
    /// The party id.
    pub id: String,
    /// The legal name.
    pub name: String,
    /// The VAT id (optional).
    pub vat_id: Option<String>,
    /// The IBAN (optional).
    pub iban: Option<String>,
    /// The BIC (optional).
    pub bic: Option<String>,
    /// Roles: 'customer', 'vendor', or 'both'.
    pub roles: String,
    /// The party this one was merged into, when it was.
    pub merged_into: Option<String>,
    /// Latest VIES check result (if any).
    pub latest_vies: Option<ViesResult>,
    /// Whether this party's name/address/contacts is personal data.
    pub personal_data: bool,
    /// Other unmerged parties with the same VAT ID, IBAN or name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub possible_duplicates: Vec<String>,
    /// Whether a person still has to confirm this party (created from a bill or a bank line).
    pub needs_review: bool,
    /// The tax number (optional).
    pub tax_number: Option<String>,
    /// The language code (optional).
    pub language: Option<String>,
    /// The currency code (optional).
    pub currency: Option<String>,
    /// Payment terms in days (optional).
    pub payment_terms_days: Option<i32>,
    /// The default expense or revenue account (optional).
    pub default_account: Option<String>,
    /// The default tax code (optional).
    pub default_tax_code: Option<String>,
    /// Postal addresses.
    #[serde(default)]
    pub addresses: Vec<Address>,
    /// Contacts.
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

/// The latest VIES check result for a party.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ViesResult {
    /// Whether the VAT number is valid.
    pub valid: bool,
    /// The VIES request identifier (consultation number).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consultation_number: Option<String>,
    /// When we checked it.
    pub checked_at: String,
}

/// The output of `parties.list`: one page, and the count of every match.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListOutput {
    /// The parties on this page.
    pub parties: Vec<PartyView>,
    /// Total number of unmerged parties that match the filters.
    pub total: u32,
}

/// Whether a party passes the search, role and review filters.
fn matches_filters(
    party: &PartyView,
    needle: Option<&str>,
    roles: Option<&str>,
    needs_review: Option<bool>,
) -> bool {
    let text_ok = needle.is_none_or(|n| {
        let compact: String = n.chars().filter(|c| !c.is_whitespace()).collect();
        party.name.to_lowercase().contains(n)
            || party
                .vat_id
                .as_deref()
                .is_some_and(|v| v.to_lowercase().contains(n))
            || party
                .iban
                .as_deref()
                .is_some_and(|i| iban::normalise(i).to_lowercase().contains(&compact))
    });
    let roles_ok = roles.is_none_or(|r| party.roles == r);
    let review_ok = needs_review.is_none_or(|flag| party.needs_review == flag);
    text_ok && roles_ok && review_ok
}

/// The other unmerged parties that may be this one (same VAT ID, IBAN or name).
fn duplicates_among(party: &PartyView, all: &[PartyView]) -> Vec<String> {
    if party.merged_into.is_some() {
        return Vec::new();
    }
    all.iter()
        .filter(|other| other.id != party.id && is_possible_duplicate(party, other))
        .map(|other| other.id.clone())
        .collect()
}

/// `parties.list`: unmerged parties, filtered and paged, with their latest VIES results.
pub struct PartiesList;

#[async_trait::async_trait]
impl Command for PartiesList {
    const NAME: &'static str = "parties.list";
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
        let limit = input
            .limit
            .unwrap_or(DEFAULT_LIST_ROWS)
            .clamp(1, MAX_LIST_ROWS) as usize;
        let offset = input.offset.unwrap_or(0) as usize;
        if let Some(roles) = &input.roles {
            check_roles(roles)?;
        }
        let needle = optional_text(input.q.as_deref()).map(|q| q.to_lowercase());
        let all = unmerged_parties(db, &ctx.company_id).await?;
        let matching: Vec<&PartyView> = all
            .iter()
            .filter(|party| {
                matches_filters(
                    party,
                    needle.as_deref(),
                    input.roles.as_deref(),
                    input.needs_review,
                )
            })
            .collect();
        let total = u32::try_from(matching.len()).unwrap_or(u32::MAX);
        let mut parties = Vec::new();
        for party in matching.into_iter().skip(offset).take(limit) {
            parties.push(PartyView {
                latest_vies: latest_vies(db, &ctx.company_id, &party.id).await?,
                possible_duplicates: duplicates_among(party, &all),
                ..party.clone()
            });
        }
        Ok(Plan::with_output(ListOutput { parties, total }))
    }
}

/// Two parties may be the same when their VAT IDs, IBANs or names agree.
fn is_possible_duplicate(a: &PartyView, b: &PartyView) -> bool {
    let same_vat = matches!((&a.vat_id, &b.vat_id), (Some(x), Some(y)) if normalize_vat_id(x) == normalize_vat_id(y));
    let same_iban = matches!((&a.iban, &b.iban), (Some(x), Some(y)) if iban::normalise(x) == iban::normalise(y));
    let name_a = normalise_name(&a.name);
    let same_name = !name_a.is_empty() && name_a == normalise_name(&b.name);
    same_vat || same_iban || same_name
}

// ===================================================================== get

/// The input of `parties.get`, by id from the path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetInput {
    /// The party id.
    pub id: String,
}

/// One change of a party's bank details, from the append-only history.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BankChangeView {
    /// The change id.
    pub id: String,
    /// The IBAN before the change (none when the party had none).
    pub before_iban: Option<String>,
    /// The BIC before the change.
    pub before_bic: Option<String>,
    /// The IBAN after the change.
    pub after_iban: Option<String>,
    /// The BIC after the change.
    pub after_bic: Option<String>,
    /// The person who made the change.
    pub changed_by: String,
    /// When the change was made.
    pub changed_at: String,
    /// The reason the person gave.
    pub reason: Option<String>,
}

/// The output of `parties.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetOutput {
    /// The party, with its latest VIES result and possible duplicates.
    pub party: PartyView,
    /// Bank details changes, newest first.
    pub bank_changes: Vec<BankChangeView>,
    /// How many bills name this party (a merge repoints them to the survivor).
    pub bill_count: u32,
    /// Whether `parties.check_vat` can call VIES on this server (`LEDGERS_VIES=1`).
    pub vies_enabled: bool,
}

/// `parties.get`: one party with its bank history and bill count.
pub struct PartiesGet {
    /// Whether VAT checks are on for this server; the output says so.
    pub vies_enabled: bool,
}

impl PartiesGet {
    /// A get that reports whether VAT checks are on.
    #[must_use]
    pub fn new(vies_enabled: bool) -> Self {
        Self { vies_enabled }
    }
}

#[async_trait::async_trait]
impl Command for PartiesGet {
    const NAME: &'static str = "parties.get";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

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
        let row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("party not found".to_string()))?;
        let mut party = party_view_from_row(&row)?;
        party.latest_vies = latest_vies(db, &ctx.company_id, &input.id).await?;
        let all = unmerged_parties(db, &ctx.company_id).await?;
        party.possible_duplicates = duplicates_among(&party, &all);
        let changes = db
            .query(&Statement::with_values(
                "SELECT id, before_iban, before_bic, after_iban, after_bic, changed_by, changed_at, reason FROM vendor_bank_change WHERE company_id = ? AND vendor_id = ? ORDER BY changed_at DESC, id DESC".to_string(),
                vec![Value::from(ctx.company_id.as_str()), Value::from(input.id.as_str())],
            ))
            .await
            .map_err(|_| database_error())?;
        let bank_changes = changes
            .rows
            .iter()
            .map(|row| {
                Ok(BankChangeView {
                    id: text_required(row, "id")?,
                    before_iban: text_column(row, "before_iban"),
                    before_bic: text_column(row, "before_bic"),
                    after_iban: text_column(row, "after_iban"),
                    after_bic: text_column(row, "after_bic"),
                    changed_by: text_required(row, "changed_by")?,
                    changed_at: text_required(row, "changed_at")?,
                    reason: text_column(row, "reason"),
                })
            })
            .collect::<Result<Vec<_>, CommandError>>()?;
        let counted = db
            .query(&Statement::with_values(
                "SELECT COUNT(*) AS n FROM bill WHERE company_id = ? AND vendor_id = ?".to_string(),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ))
            .await
            .map_err(|_| database_error())?;
        let bill_count = counted
            .rows
            .first()
            .and_then(|row| row.get::<i64>("n"))
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
        Ok(Plan::with_output(GetOutput {
            party,
            bank_changes,
            bill_count,
            vies_enabled: self.vies_enabled,
        }))
    }
}

// ===================================================================== validate

/// The input of `parties.validate`: the fields a form holds. A field left out
/// is not checked; a blank name or VAT ID that is sent is checked.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateInput {
    /// The party being edited, so its own VAT ID is not a conflict (optional).
    #[serde(default)]
    pub id: Option<String>,
    /// The legal name.
    #[serde(default)]
    pub name: Option<String>,
    /// The VAT ID, checked against its country's pack and the other parties.
    #[serde(default)]
    pub vat_id: Option<String>,
    /// The IBAN.
    #[serde(default)]
    pub iban: Option<String>,
    /// The BIC.
    #[serde(default)]
    pub bic: Option<String>,
    /// The roles.
    #[serde(default)]
    pub roles: Option<String>,
    /// The payment terms in days.
    #[serde(default)]
    pub payment_terms_days: Option<i32>,
    /// The addresses.
    #[serde(default)]
    pub addresses: Vec<Address>,
    /// The contacts.
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

/// One refused field and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FieldError {
    /// The input field name (`vat_id`, `iban`, ...).
    pub field: String,
    /// The message, in the server's words.
    pub message: String,
}

/// The output of `parties.validate`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ValidateOutput {
    /// True when no field is refused.
    pub valid: bool,
    /// Every refused field, once each.
    pub errors: Vec<FieldError>,
}

fn note(errors: &mut Vec<FieldError>, field: &str, result: Result<(), CommandError>) {
    if let Err(error) = result {
        errors.push(FieldError {
            field: field.to_string(),
            message: error_message(&error),
        });
    }
}

/// `parties.validate`: runs the same checks as create, update and `set_bank`
/// without writing anything, and names each field it refuses.
pub struct PartiesValidate;

#[async_trait::async_trait]
impl Command for PartiesValidate {
    const NAME: &'static str = "parties.validate";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = ValidateInput;
    type Output = ValidateOutput;

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
        let mut errors = Vec::new();
        if let Some(name) = &input.name {
            note(&mut errors, "name", checked_name(name).map(|_| ()));
        }
        if let Some(vat) = &input.vat_id {
            match checked_vat(vat) {
                Err(error) => note(&mut errors, "vat_id", Err(error)),
                Ok(Some(normalised)) => {
                    if let Some(owner) =
                        vat_owner(db, &ctx.company_id, &normalised, input.id.as_deref()).await?
                    {
                        errors.push(FieldError {
                            field: "vat_id".to_string(),
                            message: format!("party {owner} already has this VAT ID"),
                        });
                    }
                }
                Ok(None) => {}
            }
        }
        if let Some(iban) = &input.iban {
            note(
                &mut errors,
                "iban",
                iban::validate(iban)
                    .map(|_| ())
                    .map_err(|err| CommandError::InvalidInput(err.to_string())),
            );
        }
        if let Some(bic) = &input.bic {
            note(&mut errors, "bic", checked_bic(bic).map(|_| ()));
        }
        if let Some(roles) = &input.roles {
            note(&mut errors, "roles", check_roles(roles));
        }
        note(
            &mut errors,
            "payment_terms_days",
            check_terms(input.payment_terms_days),
        );
        if !input.addresses.is_empty() {
            note(
                &mut errors,
                "addresses",
                checked_addresses(&input.addresses).map(|_| ()),
            );
        }
        if !input.contacts.is_empty() {
            note(
                &mut errors,
                "contacts",
                checked_contacts(&input.contacts).map(|_| ()),
            );
        }
        Ok(Plan::with_output(ValidateOutput {
            valid: errors.is_empty(),
            errors,
        }))
    }
}

// ===================================================================== create

/// The input of `parties.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The party id (generated by server if empty).
    #[serde(default)]
    pub id: Option<String>,
    /// The legal name.
    pub name: String,
    /// The VAT id (optional), checked against its country's pack.
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Roles: 'customer', 'vendor', or 'both'; defaults to 'vendor'.
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// Language code (optional).
    #[serde(default)]
    pub language: Option<String>,
    /// Currency code (optional).
    #[serde(default)]
    pub currency: Option<String>,
    /// Whether this party's name/addresses/contacts is personal data (default false).
    #[serde(default)]
    pub personal_data: Option<bool>,
    /// Payment terms in days (optional, >= 0).
    #[serde(default)]
    pub payment_terms_days: Option<i32>,
    /// Default account number (optional).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Default tax code (optional).
    #[serde(default)]
    pub default_tax_code: Option<String>,
    /// Postal addresses (optional).
    #[serde(default)]
    pub addresses: Vec<Address>,
    /// Contacts (optional).
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

/// The output of `parties.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateOutput {
    /// The created party.
    pub party: PartyView,
}

/// `parties.create`: create a party (master data only, agent allowed).
pub struct PartiesCreate;

#[async_trait::async_trait]
impl Command for PartiesCreate {
    const NAME: &'static str = "parties.create";
    const TIER: Tier = Tier::Draft;

    type Input = CreateInput;
    type Output = CreateOutput;

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
        let id = input.id.clone().ok_or_else(|| {
            CommandError::InvalidInput("id must be provided or filled by server".to_string())
        })?;
        let name = checked_name(&input.name)?;
        let roles = input.roles.clone().unwrap_or_else(|| "vendor".to_string());
        check_roles(&roles)?;
        check_terms(input.payment_terms_days)?;
        let vat_id = match input.vat_id.as_deref().map(checked_vat) {
            Some(result) => result?,
            None => None,
        };
        if let Some(vat) = &vat_id
            && let Some(owner) = vat_owner(db, &ctx.company_id, vat, None).await?
        {
            return Err(CommandError::InvalidInput(format!(
                "party {owner} already has this VAT ID"
            )));
        }
        let addresses = checked_addresses(&input.addresses)?;
        let contacts = checked_contacts(&input.contacts)?;
        let personal_data = input.personal_data.unwrap_or(false);
        let tax_number = optional_text(input.tax_number.as_deref());
        let language = optional_text(input.language.as_deref());
        let currency = optional_text(input.currency.as_deref());
        let default_account = optional_text(input.default_account.as_deref());
        let default_tax_code = optional_text(input.default_tax_code.as_deref());
        let statement = Statement::with_values(
            "INSERT INTO vendor (id, company_id, name, vat_id, roles, tax_number, language, currency, personal_data, payment_terms_days, default_account, default_tax_code, addresses, contacts, source, needs_review, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'manual', 0, ?, ?)".to_string(),
            vec![
                Value::from(id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(name.as_str()),
                optional_string_value(vat_id.as_ref()),
                Value::from(roles.as_str()),
                optional_string_value(tax_number.as_ref()),
                optional_string_value(language.as_ref()),
                optional_string_value(currency.as_ref()),
                Value::from(i32::from(personal_data)),
                input.payment_terms_days.map_or(Value::Int(None), Value::from),
                optional_string_value(default_account.as_ref()),
                optional_string_value(default_tax_code.as_ref()),
                json_column(&addresses)?,
                json_column(&contacts)?,
                Value::from(ctx.now.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        );
        let party = PartyView {
            id: id.clone(),
            name,
            vat_id,
            iban: None,
            bic: None,
            roles,
            merged_into: None,
            latest_vies: None,
            personal_data,
            possible_duplicates: Vec::new(),
            needs_review: false,
            tax_number,
            language,
            currency,
            payment_terms_days: input.payment_terms_days,
            default_account,
            default_tax_code,
            addresses,
            contacts,
        };
        let mut plan = Plan::with_output(CreateOutput { party });
        plan.statements.push(statement);
        plan.objects.push(format!("vendor:{id}"));
        Ok(plan)
    }
}

// ===================================================================== update

/// The input of `parties.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The party id (from the path).
    pub id: String,
    /// The legal name (optional).
    #[serde(default)]
    pub name: Option<String>,
    /// The VAT id (optional), checked against its country's pack. Blank clears it.
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Roles (optional).
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// Language code (optional).
    #[serde(default)]
    pub language: Option<String>,
    /// Currency code (optional).
    #[serde(default)]
    pub currency: Option<String>,
    /// Whether personal data (optional).
    #[serde(default)]
    pub personal_data: Option<bool>,
    /// Payment terms in days (optional).
    #[serde(default)]
    pub payment_terms_days: Option<i32>,
    /// Default account number (optional).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Default tax code (optional).
    #[serde(default)]
    pub default_tax_code: Option<String>,
    /// Postal addresses; when given, they replace the list (optional).
    #[serde(default)]
    pub addresses: Option<Vec<Address>>,
    /// Contacts; when given, they replace the list (optional).
    #[serde(default)]
    pub contacts: Option<Vec<Contact>>,
}

/// The output of `parties.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateOutput {
    /// The updated party.
    pub party: PartyView,
}

/// `parties.update`: update a party's master data (agent allowed). Bank
/// details change only through `parties.set_bank`.
pub struct PartiesUpdate;

/// The party as an update leaves it, for the output: the fields the input
/// named take their new values, the rest stay as they were.
fn party_after_update(
    current: PartyView,
    input: &UpdateInput,
    name: String,
    new_vat: Option<String>,
    addresses: Option<Vec<Address>>,
    contacts: Option<Vec<Contact>>,
) -> PartyView {
    let mut party = PartyView {
        name,
        vat_id: new_vat,
        roles: input.roles.clone().unwrap_or_else(|| current.roles.clone()),
        personal_data: input.personal_data.unwrap_or(current.personal_data),
        possible_duplicates: Vec::new(),
        ..current
    };
    if let Some(t) = &input.tax_number {
        party.tax_number = optional_text(Some(t));
    }
    if let Some(l) = &input.language {
        party.language = optional_text(Some(l));
    }
    if let Some(c) = &input.currency {
        party.currency = optional_text(Some(c));
    }
    if input.payment_terms_days.is_some() {
        party.payment_terms_days = input.payment_terms_days;
    }
    if let Some(a) = &input.default_account {
        party.default_account = optional_text(Some(a));
    }
    if let Some(t) = &input.default_tax_code {
        party.default_tax_code = optional_text(Some(t));
    }
    if let Some(list) = addresses {
        party.addresses = list;
    }
    if let Some(list) = contacts {
        party.contacts = list;
    }
    party
}

/// The SET clauses of `parties.update` and their values: only the fields the
/// input named, with the `updated_at` stamp first.
fn update_columns(
    input: &UpdateInput,
    name: &str,
    new_vat: Option<&String>,
    addresses: Option<&[Address]>,
    contacts: Option<&[Contact]>,
    now: &str,
) -> Result<(Vec<&'static str>, Vec<Value>), CommandError> {
    let mut updates = vec!["updated_at = ?"];
    let mut values = vec![Value::from(now)];
    let mut set = |column: &'static str, value: Value| {
        updates.push(column);
        values.push(value);
    };
    if input.name.is_some() {
        set("name = ?", Value::from(name));
    }
    if input.vat_id.is_some() {
        set("vat_id = ?", optional_string_value(new_vat));
    }
    if let Some(r) = &input.roles {
        set("roles = ?", Value::from(r.as_str()));
    }
    if let Some(t) = &input.tax_number {
        set("tax_number = ?", text_value(t));
    }
    if let Some(l) = &input.language {
        set("language = ?", text_value(l));
    }
    if let Some(c) = &input.currency {
        set("currency = ?", text_value(c));
    }
    if let Some(p) = input.personal_data {
        set("personal_data = ?", Value::from(i32::from(p)));
    }
    if let Some(d) = input.payment_terms_days {
        set("payment_terms_days = ?", Value::from(d));
    }
    if let Some(a) = &input.default_account {
        set("default_account = ?", text_value(a));
    }
    if let Some(t) = &input.default_tax_code {
        set("default_tax_code = ?", text_value(t));
    }
    if let Some(list) = addresses {
        set("addresses = ?", json_column(list)?);
    }
    if let Some(list) = contacts {
        set("contacts = ?", json_column(list)?);
    }
    Ok((updates, values))
}

#[async_trait::async_trait]
impl Command for PartiesUpdate {
    const NAME: &'static str = "parties.update";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");

    type Input = UpdateInput;
    type Output = UpdateOutput;

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
        let row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("party not found".to_string()))?;
        let current = party_view_from_row(&row)?;
        if current.merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "party is merged and cannot be updated".to_string(),
            ));
        }
        check_terms(input.payment_terms_days)?;
        if let Some(roles) = &input.roles {
            check_roles(roles)?;
        }
        let name = match &input.name {
            Some(n) => checked_name(n)?,
            None => current.name.clone(),
        };
        let new_vat = match input.vat_id.as_deref().map(checked_vat) {
            Some(result) => result?,
            None => current.vat_id.clone(),
        };
        if let Some(vat) = &new_vat
            && current.vat_id.as_deref() != Some(vat.as_str())
            && let Some(owner) = vat_owner(db, &ctx.company_id, vat, Some(&input.id)).await?
        {
            return Err(CommandError::InvalidInput(format!(
                "party {owner} already has this VAT ID"
            )));
        }
        let addresses = input
            .addresses
            .as_deref()
            .map(checked_addresses)
            .transpose()?;
        let contacts = input
            .contacts
            .as_deref()
            .map(checked_contacts)
            .transpose()?;
        let (updates, mut values) = update_columns(
            input,
            &name,
            new_vat.as_ref(),
            addresses.as_deref(),
            contacts.as_deref(),
            &ctx.now,
        )?;
        values.push(Value::from(ctx.company_id.as_str()));
        values.push(Value::from(input.id.as_str()));
        let guard_key = guard::guard_key(&["party_not_merged", &input.id]);
        let statements = vec![
            guard::assert_statement(
                &guard_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
            Statement::with_values(
                format!(
                    "UPDATE vendor SET {} WHERE company_id = ? AND id = ?",
                    updates.join(", ")
                ),
                values,
            ),
            guard::release_statement(&guard_key),
        ];
        let party = party_after_update(current, input, name, new_vat, addresses, contacts);
        let mut plan = Plan::with_output(UpdateOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        Ok(plan)
    }
}

// ===================================================================== set_bank

/// The input of `parties.set_bank`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetBankInput {
    /// The party id (from the path).
    pub id: String,
    /// The new IBAN; checked for country length and mod-97.
    pub iban: String,
    /// The new BIC (optional).
    #[serde(default)]
    pub bic: Option<String>,
    /// Why the details changed (kept in the history).
    pub reason: String,
}

/// The output of `parties.set_bank`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SetBankOutput {
    /// The party with its new bank details.
    pub party: PartyView,
}

/// `parties.set_bank`: change IBAN/BIC with history tracking. Human only:
/// an agent is refused by the executor, and the server requires a person.
pub struct PartiesSetBank {
    /// The person's id (who made the change), set by dispatch before execution.
    pub person_id: Option<String>,
}

impl PartiesSetBank {
    /// Creates the command for `person_id`; `None` is refused at plan time.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesSetBank {
    const NAME: &'static str = "parties.set_bank";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = SetBankInput;
    type Output = SetBankOutput;

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
        // A bank change is never written without the acting person.
        let changed_by = self
            .person_id
            .clone()
            .ok_or(CommandError::Unauthenticated)?;
        let new_iban = iban::validate(&input.iban)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        let new_bic = input.bic.as_deref().map(checked_bic).transpose()?;
        let reason = input.reason.trim();
        if reason.is_empty() {
            return Err(CommandError::InvalidInput(
                "a bank change needs a reason".to_string(),
            ));
        }

        let row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("party not found".to_string()))?;
        let current = party_view_from_row(&row)?;
        if current.merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "party is merged and cannot be changed".to_string(),
            ));
        }
        let iban_differs = current
            .iban
            .as_deref()
            .is_none_or(|known| iban::normalise(known) != new_iban);

        let guard_key = guard::guard_key(&["party_not_merged", &input.id]);
        let change_id = format!("change_{}", ctx.event_id);
        let mut statements = vec![
            guard::assert_statement(
                &guard_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
            // The history row captures the before values in the same batch.
            Statement::with_values(
                "INSERT INTO vendor_bank_change (id, company_id, vendor_id, before_iban, before_bic, after_iban, after_bic, changed_by, changed_at, reason, created_at) SELECT ?, company_id, id, iban, bic, ?, ?, ?, ?, ?, ? FROM vendor WHERE company_id = ? AND id = ?".to_string(),
                vec![
                    Value::from(change_id.as_str()),
                    Value::from(new_iban.as_str()),
                    optional_string_value(new_bic.as_ref()),
                    Value::from(changed_by.as_str()),
                    Value::from(ctx.now.as_str()),
                    Value::from(reason),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
            Statement::with_values(
                "UPDATE vendor SET iban = ?, bic = ?, updated_at = ? WHERE company_id = ? AND id = ?"
                    .to_string(),
                vec![
                    Value::from(new_iban.as_str()),
                    optional_string_value(new_bic.as_ref()),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
        ];
        if iban_differs {
            // Open bills from this party now carry a changed-IBAN flag: a
            // person must confirm the payee before anything is paid.
            statements.push(Statement::with_values(
                "UPDATE bill SET iban_changed = 1, updated_at = ? WHERE company_id = ? AND vendor_id = ? AND posted_entry_id IS NULL AND discarded_at IS NULL".to_string(),
                vec![
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ));
        }
        statements.push(guard::release_statement(&guard_key));

        let party = PartyView {
            iban: Some(new_iban),
            bic: new_bic,
            ..current
        };
        let mut plan = Plan::with_output(SetBankOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        plan.objects.push(format!("vendor_bank_change:{change_id}"));
        Ok(plan)
    }
}

// ===================================================================== check_vat

/// The input of `parties.check_vat`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckVatInput {
    /// The party id (from the path).
    pub id: String,
    /// The country code (e.g. `DE`, `FR`); must be a VIES member state.
    pub country_code: String,
    /// The VAT number without its country prefix. Together they must be the
    /// party's VAT ID.
    pub vat_number: String,
}

/// The output of `parties.check_vat`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CheckVatOutput {
    /// The VIES check result.
    pub vies_result: ViesCheckResult,
}

/// The VIES check result returned to the caller.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ViesCheckResult {
    /// Whether the VAT number is valid.
    pub valid: bool,
    /// The returned business name (if valid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The returned address (if valid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The VIES request identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consultation_number: Option<String>,
    /// The VIES request date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_date: Option<String>,
}

/// The answer of a VIES lookup, as the port returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViesResponse {
    /// Whether VIES says the number is valid.
    pub valid: bool,
    /// The business name VIES returned.
    pub name: Option<String>,
    /// The address VIES returned.
    pub address: Option<String>,
    /// The request identifier (consultation number).
    pub consultation_number: Option<String>,
    /// The request date.
    pub request_date: Option<String>,
}

/// The VIES lookup, as a port: the one place a VAT number leaves the server.
/// The server wires [`HttpVies`] only when `LEDGERS_VIES=1`; tests inject a
/// fake, so no test reaches the network.
#[async_trait::async_trait]
pub trait ViesPort: Send + Sync {
    /// Looks up `vat_number` (without prefix) for `country_code`.
    ///
    /// # Errors
    /// A message when the lookup did not complete. Nothing is recorded then.
    async fn check(&self, country_code: &str, vat_number: &str) -> Result<ViesResponse, String>;
}

/// The VIES REST API, reached through the harness HTTP transport.
pub struct HttpVies {
    transport: Arc<dyn Transport>,
}

impl HttpVies {
    /// Wraps a transport that can POST JSON.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }
}

#[async_trait::async_trait]
impl ViesPort for HttpVies {
    async fn check(&self, country_code: &str, vat_number: &str) -> Result<ViesResponse, String> {
        let body = json!({
            "countryCode": country_code,
            "vatNumber": vat_number,
        });
        let (status, response) = self.transport.post_json(VIES_URL, &[], &body).await?;
        if status != 200 {
            return Err(format!("VIES answered with status {status}"));
        }
        let valid = response
            .get("valid")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "VIES answer had no valid flag".to_string())?;
        let text = |key: &str| {
            response
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        Ok(ViesResponse {
            valid,
            name: text("name"),
            address: text("address"),
            consultation_number: text("requestIdentifier"),
            request_date: text("requestDate"),
        })
    }
}

/// The member states VIES checks: the EU pack's coverage, and Northern
/// Ireland (`XI`), which VIES checks under its own code.
fn vies_covers(country_code: &str) -> Result<bool, CommandError> {
    if country_code == "XI" {
        return Ok(true);
    }
    let pack = pack_by_id("eu").map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(pack.covers_country(country_code))
}

/// `parties.check_vat`: looks the party's VAT ID up in VIES and records the
/// answer (agent allowed). Refused when VIES is off on this server.
pub struct PartiesCheckVat {
    /// The VIES port, when the server enables VAT checks.
    pub vies: Option<Arc<dyn ViesPort>>,
}

impl PartiesCheckVat {
    /// Creates the command; `None` refuses every check with a clear message.
    #[must_use]
    pub fn new(vies: Option<Arc<dyn ViesPort>>) -> Self {
        Self { vies }
    }
}

#[async_trait::async_trait]
impl Command for PartiesCheckVat {
    const NAME: &'static str = "parties.check_vat";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");

    type Input = CheckVatInput;
    type Output = CheckVatOutput;

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
        let country = input.country_code.trim().to_ascii_uppercase();
        if !vies_covers(&country)? {
            return Err(CommandError::InvalidInput(
                "VAT checking is only available for EU countries".to_string(),
            ));
        }
        let full = normalize_vat_id(&format!("{country}{}", input.vat_number));
        // The same validator the company's own VAT ID passes.
        check_vat_id(&country, &full).map_err(CommandError::InvalidInput)?;

        let row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("party not found".to_string()))?;
        let party = party_view_from_row(&row)?;
        if party.merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "party is merged and cannot be checked".to_string(),
            ));
        }
        if party.vat_id.as_deref() != Some(full.as_str()) {
            return Err(CommandError::InvalidInput(
                "the VAT number is not this party's VAT ID; update the party first".to_string(),
            ));
        }

        let port = self.vies.clone().ok_or_else(|| {
            CommandError::InvalidInput(
                "VAT checks are off on this server (LEDGERS_VIES is not 1)".to_string(),
            )
        })?;
        let answer = port
            .check(&country, &input.vat_number)
            .await
            .map_err(|err| CommandError::InvalidInput(format!("VIES did not answer: {err}")))?;

        let check_id = format!("vies_{}", ctx.event_id);
        let statement = Statement::with_values(
            "INSERT INTO vies_check (id, company_id, vendor_id, country_code, vat_number, valid, name, address, consultation_number, request_date, checked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".to_string(),
            vec![
                Value::from(check_id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
                Value::from(country.as_str()),
                Value::from(full.as_str()),
                Value::from(i32::from(answer.valid)),
                optional_string_value(answer.name.as_ref()),
                optional_string_value(answer.address.as_ref()),
                optional_string_value(answer.consultation_number.as_ref()),
                optional_string_value(answer.request_date.as_ref()),
                Value::from(ctx.now.as_str()),
            ],
        );

        let mut plan = Plan::with_output(CheckVatOutput {
            vies_result: ViesCheckResult {
                valid: answer.valid,
                name: answer.name,
                address: answer.address,
                consultation_number: answer.consultation_number,
                request_date: answer.request_date,
            },
        });
        plan.statements.push(statement);
        plan.objects.push(format!("vies_check:{check_id}"));
        Ok(plan)
    }
}

// ===================================================================== merge

/// The input of `parties.merge`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeInput {
    /// The surviving party id (from the path).
    pub id: String,
    /// The party to merge away.
    pub from: String,
}

/// The output of `parties.merge`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MergeOutput {
    /// The surviving party.
    pub party: PartyView,
}

/// `parties.merge`: folds `from` into `id` (human only). The source stays as
/// a merged record, and its bills move to the survivor.
pub struct PartiesMerge {
    /// The person's id (who performed the merge), set by dispatch before execution.
    pub person_id: Option<String>,
}

impl PartiesMerge {
    /// Creates the command for `person_id`; `None` is refused at plan time.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesMerge {
    const NAME: &'static str = "parties.merge";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = MergeInput;
    type Output = MergeOutput;

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
        if self.person_id.is_none() {
            return Err(CommandError::Unauthenticated);
        }
        if input.id == input.from {
            return Err(CommandError::InvalidInput(
                "cannot merge a party into itself".to_string(),
            ));
        }
        let survivor_row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("survivor party not found".to_string()))?;
        let source_row = read_party(db, &ctx.company_id, &input.from)
            .await?
            .ok_or_else(|| CommandError::InvalidInput("source party not found".to_string()))?;
        let survivor = party_view_from_row(&survivor_row)?;
        if survivor.merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "survivor party is already merged".to_string(),
            ));
        }
        if party_view_from_row(&source_row)?.merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "source party is already merged".to_string(),
            ));
        }

        let survivor_key = guard::guard_key(&["merge_survivor", &input.id]);
        let source_key = guard::guard_key(&["merge_source", &input.from]);
        let statements = vec![
            guard::assert_statement(
                &survivor_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
            guard::assert_statement(
                &source_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.from.as_str()),
                ],
            ),
            Statement::with_values(
                "UPDATE bill SET vendor_id = ?, updated_at = ? WHERE company_id = ? AND vendor_id = ?"
                    .to_string(),
                vec![
                    Value::from(input.id.as_str()),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.from.as_str()),
                ],
            ),
            Statement::with_values(
                "UPDATE vendor SET merged_into = ?, updated_at = ? WHERE company_id = ? AND id = ? AND merged_into IS NULL"
                    .to_string(),
                vec![
                    Value::from(input.id.as_str()),
                    Value::from(ctx.now.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.from.as_str()),
                ],
            ),
            guard::release_statement(&survivor_key),
            guard::release_statement(&source_key),
        ];

        let mut plan = Plan::with_output(MergeOutput { party: survivor });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        plan.objects.push(format!("vendor:{}", input.from));
        Ok(plan)
    }
}

// ===================================================================== zero-rating gate

/// Guard statement for the zero-rating gate: fails unless the party's most
/// recent VIES check is valid and was for the party's current VAT ID. An
/// intra-EU supply is zero-rated only with that check on file.
#[must_use]
pub fn vies_valid_guard(company_id: &str, party_id: &str) -> Statement {
    Statement::with_values(
        "INSERT INTO ledger_guard (guard_key, ok) SELECT ?, CASE WHEN EXISTS(\
            SELECT 1 FROM vendor p \
            JOIN vies_check v ON v.vendor_id = p.id AND v.company_id = p.company_id \
            WHERE p.company_id = ? AND p.id = ? AND v.valid = 1 \
            AND v.vat_number = p.vat_id \
            AND v.checked_at = (\
                SELECT MAX(checked_at) FROM vies_check \
                WHERE company_id = ? AND vendor_id = ?\
            )\
        ) THEN 1 ELSE NULL END"
            .to_string(),
        vec![
            Value::from(format!("vies_valid:{party_id}").as_str()),
            Value::from(company_id),
            Value::from(party_id),
            Value::from(company_id),
            Value::from(party_id),
        ],
    )
}
