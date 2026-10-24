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
use crate::party_fields::{
    self as pf, ADDRESS_INVALID, BANK_REASON_REQUIRED, BIC_INVALID, CONTACT_INVALID,
    CONTACT_PRIMARY_DUPLICATE, IBAN_INVALID, MERGED, NAME_REQUIRED, NOT_FOUND, ROLES_INVALID,
    TERMS_INVALID, TaxId, VAT_INVALID, VAT_TAKEN, refuse,
};
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
const VENDOR_COLUMNS: &str = "id, name, vat_id, iban, bic, roles, merged_into, personal_data, needs_review, tax_number, language, currency, payment_terms_days, default_account, default_tax_code, addresses, contacts, trading_name, registration_number, website, email, phone, default_unit, tags, notes, tax_ids, logo_document_id";

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
        refuse(
            &VAT_INVALID,
            "vat_id",
            "a VAT ID starts with its two-letter country code",
        )
    })?;
    check_vat_id(country, &normalised)
        .map_err(|message| refuse(&VAT_INVALID, "vat_id", message))?;
    Ok(Some(normalised))
}

/// Validates a role name the way the `vendor.roles` column does.
fn check_roles(roles: &str) -> Result<(), CommandError> {
    if ["customer", "vendor", "both"].contains(&roles) {
        Ok(())
    } else {
        Err(refuse(
            &ROLES_INVALID,
            "roles",
            "roles must be 'customer', 'vendor', or 'both'",
        ))
    }
}

fn check_terms(days: Option<i32>) -> Result<(), CommandError> {
    match days {
        Some(d) if d < 0 => Err(refuse(
            &TERMS_INVALID,
            "payment_terms_days",
            "payment_terms_days must be >= 0",
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
        Err(refuse(
            &BIC_INVALID,
            "bic",
            "a BIC is 8 or 11 letters and digits",
        ))
    }
}

/// The party name, trimmed; a party needs one.
fn checked_name(raw: &str) -> Result<String, CommandError> {
    let name = raw.trim();
    if name.is_empty() {
        Err(refuse(&NAME_REQUIRED, "name", "the party needs a name"))
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
    CommandError::Domain(cratefield_core::Problem::new(
        &ledgers_commands::error::DATABASE_ERROR,
    ))
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
        .ok_or_else(|| refuse(&NAME_REQUIRED, column, format!("missing {column}")))
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
        trading_name: text_column(row, "trading_name"),
        registration_number: text_column(row, "registration_number"),
        website: text_column(row, "website"),
        email: text_column(row, "email"),
        phone: text_column(row, "phone"),
        default_unit: text_column(row, "default_unit"),
        tags: read_json_column(row, "tags")?,
        notes: text_column(row, "notes"),
        tax_ids: read_json_column(row, "tax_ids")?,
        logo_document_id: text_column(row, "logo_document_id"),
    })
}

/// A message for a refusal, without the variant name.
fn error_message(error: &CommandError) -> String {
    match error {
        CommandError::InvalidInput(message) => message.clone(),
        CommandError::Domain(problem) => problem
            .detail
            .clone()
            .unwrap_or_else(|| problem.title.to_string()),
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
    /// Whether this is the party's primary contact (at most one per party).
    #[serde(default)]
    pub primary: bool,
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
                return Err(refuse(
                    &ADDRESS_INVALID,
                    "addresses",
                    format!("address {number} needs its first line"),
                ));
            }
            if city.is_empty() {
                return Err(refuse(
                    &ADDRESS_INVALID,
                    "addresses",
                    format!("address {number} needs a city"),
                ));
            }
            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic()) {
                return Err(refuse(
                    &ADDRESS_INVALID,
                    "addresses",
                    format!("address {number} needs a two-letter country code"),
                ));
            }
            if let Some(k) = &kind
                && !["billing", "shipping", "other"].contains(&k.as_str())
            {
                return Err(refuse(
                    &ADDRESS_INVALID,
                    "addresses",
                    format!("address {number} kind must be billing, shipping or other"),
                ));
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

/// Checks and tidies contacts: a name each, and a plausible email when given.
/// # Errors
/// A refusal naming the first contact that fails a check.
pub fn checked_contacts(contacts: &[Contact]) -> Result<Vec<Contact>, CommandError> {
    let checked = contacts
        .iter()
        .enumerate()
        .map(|(index, contact)| {
            let number = index + 1;
            let name = contact.name.trim();
            if name.is_empty() {
                return Err(refuse(
                    &CONTACT_INVALID,
                    "contacts",
                    format!("contact {number} needs a name"),
                ));
            }
            let email = pf::checked_email(contact.email.as_deref()).map_err(|_| {
                refuse(
                    &CONTACT_INVALID,
                    "contacts",
                    format!("contact {number} has an email address that does not look right"),
                )
            })?;
            Ok(Contact {
                name: name.to_string(),
                email,
                phone: optional_text(contact.phone.as_deref()),
                role: optional_text(contact.role.as_deref()),
                primary: contact.primary,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    if checked.iter().filter(|c| c.primary).count() > 1 {
        return Err(refuse(
            &CONTACT_PRIMARY_DUPLICATE,
            "contacts",
            "only one contact can be the primary contact",
        ));
    }
    Ok(checked)
}

// ===================================================================== list

/// The input of `parties.list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
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
    /// Keep only parties carrying this tag (case-insensitive).
    #[serde(default)]
    pub tag: Option<String>,
}

/// A party, with its latest VIES result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
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
    /// The name people use (optional); `name` is the legal name.
    #[serde(default)]
    pub trading_name: Option<String>,
    /// The company registration number (optional).
    #[serde(default)]
    pub registration_number: Option<String>,
    /// The website, `http://` or `https://` (optional).
    #[serde(default)]
    pub website: Option<String>,
    /// The main email address (optional).
    #[serde(default)]
    pub email: Option<String>,
    /// The main phone number (optional).
    #[serde(default)]
    pub phone: Option<String>,
    /// The business unit this party's documents book to (optional).
    #[serde(default)]
    pub default_unit: Option<String>,
    /// Tags, lower-cased.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Free-text notes (optional).
    #[serde(default)]
    pub notes: Option<String>,
    /// Further VAT and tax identifiers; `vat_id` stays the primary VAT ID.
    #[serde(default)]
    pub tax_ids: Vec<TaxId>,
    /// The stored logo document (a PNG or JPEG), when the party has one.
    #[serde(default)]
    pub logo_document_id: Option<String>,
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
    tag: Option<&str>,
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
    let tag_ok = tag.is_none_or(|t| party.tags.iter().any(|own| own == t));
    text_ok && roles_ok && review_ok && tag_ok
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
        let tag = optional_text(input.tag.as_deref()).map(|t| t.to_lowercase());
        let all = unmerged_parties(db, &ctx.company_id).await?;
        let matching: Vec<&PartyView> = all
            .iter()
            .filter(|party| {
                matches_filters(
                    party,
                    needle.as_deref(),
                    input.roles.as_deref(),
                    input.needs_review,
                    tag.as_deref(),
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

/// A recent bill of a party.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BillSummary {
    /// The bill id.
    pub id: String,
    /// The invoice number on the bill, when it has one.
    pub invoice_number: Option<String>,
    /// The invoice date.
    pub invoice_date: Option<String>,
    /// The due date.
    pub due_date: Option<String>,
    /// The currency code.
    pub currency: String,
    /// The gross amount, in minor units, when known.
    pub gross_minor: Option<i64>,
    /// The amount paid so far, in minor units.
    pub paid_minor: i64,
    /// Whether the bill has been posted to the books.
    pub posted: bool,
    /// Whether the bill was discarded.
    pub discarded: bool,
}

/// A payment made against a party's bill.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PaymentSummary {
    /// The bill the payment settles.
    pub bill_id: String,
    /// The invoice number of that bill, when it has one.
    pub invoice_number: Option<String>,
    /// The currency code.
    pub currency: String,
    /// The cumulative amount paid, in minor units.
    pub paid_minor: i64,
    /// When the payment was last recorded.
    pub paid_at: String,
}

/// The output of `parties.get`: the party, its bank history, recent bills and
/// payments. A party's open items are read by `parties.open_items`.
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
    /// The ten most recent bills of the party, newest first.
    pub recent_bills: Vec<BillSummary>,
    /// The ten most recent payments on the party's bills, newest first.
    pub recent_payments: Vec<PaymentSummary>,
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
            .ok_or_else(|| refuse(&NOT_FOUND, "id", "party not found"))?;
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
        let bill_rows = db
            .query(&Statement::with_values(
                "SELECT id, invoice_number, invoice_date, due_date, currency, gross_minor, paid_minor, posted_entry_id, discarded_at FROM bill WHERE company_id = ? AND vendor_id = ? ORDER BY created_at DESC, id LIMIT 10".to_string(),
                vec![Value::from(ctx.company_id.as_str()), Value::from(input.id.as_str())],
            ))
            .await
            .map_err(|_| database_error())?;
        let mut recent_bills = Vec::new();
        for row in &bill_rows.rows {
            recent_bills.push(BillSummary {
                id: text_required(row, "id")?,
                invoice_number: text_column(row, "invoice_number"),
                invoice_date: text_column(row, "invoice_date"),
                due_date: text_column(row, "due_date"),
                currency: text_required(row, "currency")?,
                gross_minor: row.get::<i64>("gross_minor"),
                paid_minor: row.get::<i64>("paid_minor").unwrap_or(0),
                posted: text_column(row, "posted_entry_id").is_some(),
                discarded: text_column(row, "discarded_at").is_some(),
            });
        }
        let payment_rows = db
            .query(&Statement::with_values(
                "SELECT id, invoice_number, currency, paid_minor, updated_at FROM bill WHERE company_id = ? AND vendor_id = ? AND paid_minor > 0 ORDER BY updated_at DESC, id LIMIT 10".to_string(),
                vec![Value::from(ctx.company_id.as_str()), Value::from(input.id.as_str())],
            ))
            .await
            .map_err(|_| database_error())?;
        let mut recent_payments = Vec::new();
        for row in &payment_rows.rows {
            recent_payments.push(PaymentSummary {
                bill_id: text_required(row, "id")?,
                invoice_number: text_column(row, "invoice_number"),
                currency: text_required(row, "currency")?,
                paid_minor: row.get::<i64>("paid_minor").unwrap_or(0),
                paid_at: text_required(row, "updated_at")?,
            });
        }
        Ok(Plan::with_output(GetOutput {
            party,
            bank_changes,
            bill_count,
            vies_enabled: self.vies_enabled,
            recent_bills,
            recent_payments,
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
    /// The name people use.
    #[serde(default)]
    pub trading_name: Option<String>,
    /// The registration number.
    #[serde(default)]
    pub registration_number: Option<String>,
    /// The website.
    #[serde(default)]
    pub website: Option<String>,
    /// The main email address.
    #[serde(default)]
    pub email: Option<String>,
    /// The main phone number.
    #[serde(default)]
    pub phone: Option<String>,
    /// The business unit.
    #[serde(default)]
    pub default_unit: Option<String>,
    /// The currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// The language.
    #[serde(default)]
    pub language: Option<String>,
    /// The tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// The notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// Further tax IDs.
    #[serde(default)]
    pub tax_ids: Option<Vec<TaxId>>,
}

/// One refused field and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FieldError {
    /// The input field name (`vat_id`, `iban`, ...).
    pub field: String,
    /// The message, in the server's words.
    pub message: String,
    /// The problem slug of the refusal (`party-vat-invalid`, ...), so the app can word it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
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
            slug: pf::slug_of(&error).map(str::to_string),
        });
    }
}

/// `parties.validate`: runs the same checks as create, update and `set_bank`
/// without writing anything, and names each field it refuses.
pub struct PartiesValidate;

/// The directory fields a form holds, each checked on its own.
fn validate_directory(input: &ValidateInput, errors: &mut Vec<FieldError>) {
    let vat_for_tax_ids = input.vat_id.as_deref().map(normalize_vat_id);
    let directory_checks: [(&str, Result<(), CommandError>); 10] = [
        (
            "registration_number",
            pf::checked_registration(input.registration_number.as_deref()).map(|_| ()),
        ),
        (
            "website",
            pf::checked_website(input.website.as_deref()).map(|_| ()),
        ),
        (
            "email",
            pf::checked_email(input.email.as_deref()).map(|_| ()),
        ),
        (
            "phone",
            pf::checked_phone(input.phone.as_deref()).map(|_| ()),
        ),
        (
            "default_unit",
            pf::checked_unit(input.default_unit.as_deref()).map(|_| ()),
        ),
        (
            "currency",
            pf::checked_currency(input.currency.as_deref()).map(|_| ()),
        ),
        (
            "language",
            pf::checked_language(input.language.as_deref()).map(|_| ()),
        ),
        (
            "notes",
            pf::checked_notes(input.notes.as_deref()).map(|_| ()),
        ),
        (
            "tags",
            input
                .tags
                .as_deref()
                .map_or(Ok(()), |tags| pf::checked_tags(tags).map(|_| ())),
        ),
        (
            "tax_ids",
            input.tax_ids.as_deref().map_or(Ok(()), |ids| {
                pf::checked_tax_ids(ids, vat_for_tax_ids.as_deref()).map(|_| ())
            }),
        ),
    ];
    for (field, result) in directory_checks {
        note(errors, field, result);
    }
}

/// Runs every check `parties.validate` makes on the fields a form holds.
async fn validate_fields(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &ValidateInput,
) -> Result<Vec<FieldError>, CommandError> {
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
                        slug: Some(VAT_TAKEN.slug.to_string()),
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
                .map_err(|err| refuse(&IBAN_INVALID, "iban", err.to_string())),
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
    validate_directory(input, &mut errors);
    Ok(errors)
}

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
        let errors = validate_fields(db, ctx, input).await?;
        Ok(Plan::with_output(ValidateOutput {
            valid: errors.is_empty(),
            errors,
        }))
    }
}

// ===================================================================== directory fields

/// One directory field a create or an update names, after checks. A field the
/// input does not name is simply absent from the [`Directory`]; a named blank
/// text field carries `None`, which clears it on an update.
#[derive(Clone, Debug, PartialEq)]
pub enum DirField {
    /// The name people use.
    TradingName(Option<String>),
    /// The registration number.
    RegistrationNumber(Option<String>),
    /// The website.
    Website(Option<String>),
    /// The main email address.
    Email(Option<String>),
    /// The main phone number.
    Phone(Option<String>),
    /// The business unit.
    DefaultUnit(Option<String>),
    /// Notes.
    Notes(Option<String>),
    /// The currency code.
    Currency(Option<String>),
    /// The language code.
    Language(Option<String>),
    /// The tags, lower-cased.
    Tags(Vec<String>),
    /// The further tax IDs.
    TaxIds(Vec<TaxId>),
}

/// The directory fields a create or an update names, after checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Directory(Vec<DirField>);

/// The raw directory input of a create or an update, before checks.
#[derive(Default)]
pub struct DirectoryRaw<'a> {
    /// The name people use.
    pub trading_name: Option<&'a str>,
    /// The registration number.
    pub registration_number: Option<&'a str>,
    /// The website.
    pub website: Option<&'a str>,
    /// The main email address.
    pub email: Option<&'a str>,
    /// The main phone number.
    pub phone: Option<&'a str>,
    /// The business unit.
    pub default_unit: Option<&'a str>,
    /// Notes.
    pub notes: Option<&'a str>,
    /// Currency.
    pub currency: Option<&'a str>,
    /// Language.
    pub language: Option<&'a str>,
    /// Tags.
    pub tags: Option<&'a [String]>,
    /// Further tax IDs.
    pub tax_ids: Option<&'a [TaxId]>,
}

/// Checks the directory fields. The party's primary VAT ID (`primary_vat`) is
/// the one further tax IDs must not repeat.
///
/// # Errors
/// The first field that fails its check, as its party problem.
pub fn checked_directory(
    raw: &DirectoryRaw<'_>,
    primary_vat: Option<&str>,
) -> Result<Directory, CommandError> {
    let mut fields = Vec::new();
    if let Some(v) = raw.trading_name {
        fields.push(DirField::TradingName(optional_text(Some(v))));
    }
    if let Some(v) = raw.registration_number {
        fields.push(DirField::RegistrationNumber(pf::checked_registration(
            Some(v),
        )?));
    }
    if let Some(v) = raw.website {
        fields.push(DirField::Website(pf::checked_website(Some(v))?));
    }
    if let Some(v) = raw.email {
        fields.push(DirField::Email(pf::checked_email(Some(v))?));
    }
    if let Some(v) = raw.phone {
        fields.push(DirField::Phone(pf::checked_phone(Some(v))?));
    }
    if let Some(v) = raw.default_unit {
        fields.push(DirField::DefaultUnit(pf::checked_unit(Some(v))?));
    }
    if let Some(v) = raw.notes {
        fields.push(DirField::Notes(pf::checked_notes(Some(v))?));
    }
    if let Some(v) = raw.currency {
        fields.push(DirField::Currency(pf::checked_currency(Some(v))?));
    }
    if let Some(v) = raw.language {
        fields.push(DirField::Language(pf::checked_language(Some(v))?));
    }
    if let Some(list) = raw.tags {
        fields.push(DirField::Tags(pf::checked_tags(list)?));
    }
    if let Some(list) = raw.tax_ids {
        fields.push(DirField::TaxIds(pf::checked_tax_ids(list, primary_vat)?));
    }
    Ok(Directory(fields))
}

impl Directory {
    /// The columns this directory names, with their values.
    fn columns(&self) -> Result<Vec<(&'static str, Value)>, CommandError> {
        self.0
            .iter()
            .map(|field| {
                Ok(match field {
                    DirField::TradingName(v) => ("trading_name", optional_string_value(v.as_ref())),
                    DirField::RegistrationNumber(v) => {
                        ("registration_number", optional_string_value(v.as_ref()))
                    }
                    DirField::Website(v) => ("website", optional_string_value(v.as_ref())),
                    DirField::Email(v) => ("email", optional_string_value(v.as_ref())),
                    DirField::Phone(v) => ("phone", optional_string_value(v.as_ref())),
                    DirField::DefaultUnit(v) => ("default_unit", optional_string_value(v.as_ref())),
                    DirField::Notes(v) => ("notes", optional_string_value(v.as_ref())),
                    DirField::Currency(v) => ("currency", optional_string_value(v.as_ref())),
                    DirField::Language(v) => ("language", optional_string_value(v.as_ref())),
                    DirField::Tags(list) => ("tags", json_column(list)?),
                    DirField::TaxIds(list) => ("tax_ids", json_column(list)?),
                })
            })
            .collect()
    }

    /// Writes the named fields onto a party view.
    fn apply_to(&self, view: &mut PartyView) {
        for field in &self.0 {
            match field {
                DirField::TradingName(v) => view.trading_name.clone_from(v),
                DirField::RegistrationNumber(v) => view.registration_number.clone_from(v),
                DirField::Website(v) => view.website.clone_from(v),
                DirField::Email(v) => view.email.clone_from(v),
                DirField::Phone(v) => view.phone.clone_from(v),
                DirField::DefaultUnit(v) => view.default_unit.clone_from(v),
                DirField::Notes(v) => view.notes.clone_from(v),
                DirField::Currency(v) => view.currency.clone_from(v),
                DirField::Language(v) => view.language.clone_from(v),
                DirField::Tags(list) => view.tags.clone_from(list),
                DirField::TaxIds(list) => view.tax_ids.clone_from(list),
            }
        }
    }
}

// ===================================================================== create

/// The input of `parties.create`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The party id (generated by server if empty).
    #[serde(default)]
    pub id: Option<String>,
    /// The legal name.
    pub name: String,
    /// The name people use (optional).
    #[serde(default)]
    pub trading_name: Option<String>,
    /// The VAT id (optional), checked against its country's pack.
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Further VAT and tax IDs (optional), each checked against its country.
    #[serde(default)]
    pub tax_ids: Vec<TaxId>,
    /// Roles: 'customer', 'vendor', or 'both'; defaults to 'vendor'.
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// The company registration number (optional).
    #[serde(default)]
    pub registration_number: Option<String>,
    /// Website, `http://` or `https://` (optional).
    #[serde(default)]
    pub website: Option<String>,
    /// Main email address (optional).
    #[serde(default)]
    pub email: Option<String>,
    /// Main phone number (optional).
    #[serde(default)]
    pub phone: Option<String>,
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
    /// Default account number, expense or revenue (optional).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Default tax code (optional).
    #[serde(default)]
    pub default_tax_code: Option<String>,
    /// Default business unit (optional).
    #[serde(default)]
    pub default_unit: Option<String>,
    /// Tags (optional).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Notes (optional).
    #[serde(default)]
    pub notes: Option<String>,
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

impl PartiesCreate {
    fn directory_raw(input: &CreateInput) -> DirectoryRaw<'_> {
        DirectoryRaw {
            trading_name: input.trading_name.as_deref(),
            registration_number: input.registration_number.as_deref(),
            website: input.website.as_deref(),
            email: input.email.as_deref(),
            phone: input.phone.as_deref(),
            default_unit: input.default_unit.as_deref(),
            notes: input.notes.as_deref(),
            currency: input.currency.as_deref(),
            language: input.language.as_deref(),
            tags: Some(input.tags.as_slice()),
            tax_ids: Some(input.tax_ids.as_slice()),
        }
    }
}

/// The INSERT that writes a new party: its base columns, then the directory
/// columns it names.
fn insert_statement(
    ctx: &CommandContext,
    party: &PartyView,
    directory: &Directory,
) -> Result<Statement, CommandError> {
    let mut columns: Vec<&'static str> = vec![
        "id",
        "company_id",
        "name",
        "vat_id",
        "roles",
        "tax_number",
        "personal_data",
        "payment_terms_days",
        "default_account",
        "default_tax_code",
        "addresses",
        "contacts",
        "source",
        "needs_review",
        "created_at",
        "updated_at",
    ];
    let mut values: Vec<Value> = vec![
        Value::from(party.id.as_str()),
        Value::from(ctx.company_id.as_str()),
        Value::from(party.name.as_str()),
        optional_string_value(party.vat_id.as_ref()),
        Value::from(party.roles.as_str()),
        optional_string_value(party.tax_number.as_ref()),
        Value::from(i32::from(party.personal_data)),
        party
            .payment_terms_days
            .map_or(Value::Int(None), Value::from),
        optional_string_value(party.default_account.as_ref()),
        optional_string_value(party.default_tax_code.as_ref()),
        json_column(&party.addresses)?,
        json_column(&party.contacts)?,
        Value::from("manual"),
        Value::from(0_i32),
        Value::from(ctx.now.as_str()),
        Value::from(ctx.now.as_str()),
    ];
    for (column, value) in directory.columns()? {
        columns.push(column);
        values.push(value);
    }
    let placeholders = vec!["?"; columns.len()].join(", ");
    Ok(Statement::with_values(
        format!(
            "INSERT INTO vendor ({}) VALUES ({placeholders})",
            columns.join(", ")
        ),
        values,
    ))
}

/// The rows `parties.create` writes for one new party: the party as created,
/// its insert statement, and the objects it touches. Shared with the CSV import.
///
/// # Errors
/// The first field that fails its check, or a VAT ID another party holds.
pub(crate) async fn create_parts(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &CreateInput,
) -> Result<(PartyView, Statement, Vec<String>), CommandError> {
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
        return Err(refuse(
            &VAT_TAKEN,
            "vat_id",
            format!("party {owner} already has this VAT ID"),
        ));
    }
    let addresses = checked_addresses(&input.addresses)?;
    let contacts = checked_contacts(&input.contacts)?;
    let directory = checked_directory(&PartiesCreate::directory_raw(input), vat_id.as_deref())?;
    let personal_data = input.personal_data.unwrap_or(false);
    let tax_number = optional_text(input.tax_number.as_deref());
    let default_account = optional_text(input.default_account.as_deref());
    let default_tax_code = optional_text(input.default_tax_code.as_deref());

    let mut party = PartyView {
        id: id.clone(),
        name,
        vat_id,
        roles,
        personal_data,
        tax_number,
        payment_terms_days: input.payment_terms_days,
        default_account,
        default_tax_code,
        addresses,
        contacts,
        ..PartyView::default()
    };
    directory.apply_to(&mut party);
    let statement = insert_statement(ctx, &party, &directory)?;
    Ok((party, statement, vec![format!("vendor:{id}")]))
}

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
        let (party, statement, objects) = create_parts(db, ctx, input).await?;
        let mut plan = Plan::with_output(CreateOutput { party });
        plan.statements.push(statement);
        plan.objects = objects;
        Ok(plan)
    }
}

// ===================================================================== update

/// The input of `parties.update`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The party id (from the path).
    pub id: String,
    /// The legal name (optional).
    #[serde(default)]
    pub name: Option<String>,
    /// The name people use (optional). Blank clears it.
    #[serde(default)]
    pub trading_name: Option<String>,
    /// The VAT id (optional), checked against its country's pack. Blank clears it.
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Further VAT and tax IDs; when given, they replace the list (optional).
    #[serde(default)]
    pub tax_ids: Option<Vec<TaxId>>,
    /// Roles (optional).
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// The company registration number (optional). Blank clears it.
    #[serde(default)]
    pub registration_number: Option<String>,
    /// Website (optional). Blank clears it.
    #[serde(default)]
    pub website: Option<String>,
    /// Main email address (optional). Blank clears it.
    #[serde(default)]
    pub email: Option<String>,
    /// Main phone number (optional). Blank clears it.
    #[serde(default)]
    pub phone: Option<String>,
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
    /// Default business unit (optional). Blank clears it.
    #[serde(default)]
    pub default_unit: Option<String>,
    /// Tags (optional); when given, they replace the list.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Notes (optional). Blank clears them.
    #[serde(default)]
    pub notes: Option<String>,
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

impl PartiesUpdate {
    fn directory_raw(input: &UpdateInput) -> DirectoryRaw<'_> {
        DirectoryRaw {
            trading_name: input.trading_name.as_deref(),
            registration_number: input.registration_number.as_deref(),
            website: input.website.as_deref(),
            email: input.email.as_deref(),
            phone: input.phone.as_deref(),
            default_unit: input.default_unit.as_deref(),
            notes: input.notes.as_deref(),
            currency: input.currency.as_deref(),
            language: input.language.as_deref(),
            tags: input.tags.as_deref(),
            tax_ids: input.tax_ids.as_deref(),
        }
    }
}

/// The party as an update leaves it, for the output: the fields the input
/// named take their new values, the rest stay as they were.
fn party_after_update(
    current: PartyView,
    input: &UpdateInput,
    name: String,
    new_vat: Option<String>,
    addresses: Option<Vec<Address>>,
    contacts: Option<Vec<Contact>>,
    directory: &Directory,
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
    directory.apply_to(&mut party);
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
    directory: &Directory,
    now: &str,
) -> Result<(Vec<String>, Vec<Value>), CommandError> {
    let mut updates = vec!["updated_at = ?".to_string()];
    let mut values = vec![Value::from(now)];
    let mut set = |column: &str, value: Value| {
        updates.push(format!("{column} = ?"));
        values.push(value);
    };
    if input.name.is_some() {
        set("name", Value::from(name));
    }
    if input.vat_id.is_some() {
        set("vat_id", optional_string_value(new_vat));
    }
    if let Some(r) = &input.roles {
        set("roles", Value::from(r.as_str()));
    }
    if let Some(t) = &input.tax_number {
        set("tax_number", text_value(t));
    }
    if let Some(p) = input.personal_data {
        set("personal_data", Value::from(i32::from(p)));
    }
    if let Some(d) = input.payment_terms_days {
        set("payment_terms_days", Value::from(d));
    }
    if let Some(a) = &input.default_account {
        set("default_account", text_value(a));
    }
    if let Some(t) = &input.default_tax_code {
        set("default_tax_code", text_value(t));
    }
    if let Some(list) = addresses {
        set("addresses", json_column(list)?);
    }
    if let Some(list) = contacts {
        set("contacts", json_column(list)?);
    }
    for (column, value) in directory.columns()? {
        set(column, value);
    }
    Ok((updates, values))
}

/// The party and statements `parties.update` writes for one update, shared with the CSV import.
pub(crate) async fn update_parts(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &UpdateInput,
) -> Result<(PartyView, Vec<Statement>, Vec<String>), CommandError> {
    let row = read_party(db, &ctx.company_id, &input.id)
        .await?
        .ok_or_else(|| refuse(&NOT_FOUND, "id", "party not found"))?;
    let current = party_view_from_row(&row)?;
    if current.merged_into.is_some() {
        return Err(refuse(
            &MERGED,
            "id",
            "party is merged and cannot be updated",
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
        return Err(refuse(
            &VAT_TAKEN,
            "vat_id",
            format!("party {owner} already has this VAT ID"),
        ));
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
    let directory = checked_directory(&PartiesUpdate::directory_raw(input), new_vat.as_deref())?;
    let (updates, mut values) = update_columns(
        input,
        &name,
        new_vat.as_ref(),
        addresses.as_deref(),
        contacts.as_deref(),
        &directory,
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
    let party = party_after_update(
        current, input, name, new_vat, addresses, contacts, &directory,
    );
    Ok((party, statements, vec![format!("vendor:{}", input.id)]))
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
        let (party, statements, objects) = update_parts(db, ctx, input).await?;
        let mut plan = Plan::with_output(UpdateOutput { party });
        plan.statements = statements;
        plan.objects = objects;
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

/// The party and statements `parties.set_bank` writes: the new bank details,
/// the history row, and the open bills that now need a person to confirm the
/// payee. Shared with the CSV import, which runs it for human imports only.
pub(crate) async fn set_bank_parts(
    db: &dyn Database,
    ctx: &CommandContext,
    changed_by: &str,
    input: &SetBankInput,
) -> Result<(PartyView, Vec<Statement>, Vec<String>), CommandError> {
    let new_iban = iban::validate(&input.iban)
        .map_err(|err| refuse(&IBAN_INVALID, "iban", err.to_string()))?;
    let new_bic = input.bic.as_deref().map(checked_bic).transpose()?;
    let reason = input.reason.trim();
    if reason.is_empty() {
        return Err(refuse(
            &BANK_REASON_REQUIRED,
            "reason",
            "a bank change needs a reason",
        ));
    }

    let row = read_party(db, &ctx.company_id, &input.id)
        .await?
        .ok_or_else(|| refuse(&NOT_FOUND, "id", "party not found"))?;
    let current = party_view_from_row(&row)?;
    if current.merged_into.is_some() {
        return Err(refuse(
            &MERGED,
            "id",
            "party is merged and cannot be changed",
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
                Value::from(changed_by),
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
            "UPDATE bill SET iban_changed = 1, iban_confirmed = 0, updated_at = ? WHERE company_id = ? AND vendor_id = ? AND posted_entry_id IS NULL AND discarded_at IS NULL".to_string(),
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
    Ok((
        party,
        statements,
        vec![
            format!("vendor:{}", input.id),
            format!("vendor_bank_change:{change_id}"),
        ],
    ))
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
        let changed_by = self
            .person_id
            .clone()
            .ok_or(CommandError::Unauthenticated)?;
        let (party, statements, objects) = set_bank_parts(db, ctx, &changed_by, input).await?;
        let mut plan = Plan::with_output(SetBankOutput { party });
        plan.statements = statements;
        plan.objects = objects;
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
            return Err(refuse(
                &VAT_INVALID,
                "country_code",
                "VAT checking is only available for EU countries",
            ));
        }
        let full = normalize_vat_id(&format!("{country}{}", input.vat_number));
        // The same validator the company's own VAT ID passes.
        check_vat_id(&country, &full)
            .map_err(|message| refuse(&VAT_INVALID, "vat_number", message))?;

        let row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| refuse(&NOT_FOUND, "id", "party not found"))?;
        let party = party_view_from_row(&row)?;
        if party.merged_into.is_some() {
            return Err(refuse(
                &MERGED,
                "id",
                "party is merged and cannot be checked",
            ));
        }
        if party.vat_id.as_deref() != Some(full.as_str()) {
            return Err(refuse(
                &VAT_INVALID,
                "vat_number",
                "the VAT number is not this party's VAT ID; update the party first",
            ));
        }

        let port = self.vies.clone().ok_or_else(|| {
            refuse(
                &pf::VIES_UNAVAILABLE,
                "id",
                "VAT checks are off on this server (LEDGERS_VIES is not 1)",
            )
        })?;
        let answer = port
            .check(&country, &input.vat_number)
            .await
            .map_err(|err| {
                refuse(
                    &pf::VIES_UNAVAILABLE,
                    "id",
                    format!("VIES did not answer: {err}"),
                )
            })?;

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
            return Err(refuse(
                &pf::MERGE_SELF,
                "from",
                "cannot merge a party into itself",
            ));
        }
        let survivor_row = read_party(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| refuse(&NOT_FOUND, "survivor", "survivor party not found"))?;
        let source_row = read_party(db, &ctx.company_id, &input.from)
            .await?
            .ok_or_else(|| refuse(&NOT_FOUND, "from", "source party not found"))?;
        let survivor = party_view_from_row(&survivor_row)?;
        if survivor.merged_into.is_some() {
            return Err(refuse(&MERGED, "id", "survivor party is already merged"));
        }
        if party_view_from_row(&source_row)?.merged_into.is_some() {
            return Err(refuse(&MERGED, "from", "source party is already merged"));
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

// ===================================================================== helpers shared with the CSV import

/// The party row of a company, for the import and the logo command.
pub(crate) async fn read_party_row(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<cratefield_core::Row>, CommandError> {
    read_party(db, company_id, id).await
}

/// A party view read from its row.
pub(crate) fn party_from_row(row: &cratefield_core::Row) -> Result<PartyView, CommandError> {
    party_view_from_row(row)
}

/// Every unmerged party of the company.
pub(crate) async fn unmerged(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<PartyView>, CommandError> {
    unmerged_parties(db, company_id).await
}

/// A BIC, checked and upper-cased.
pub(crate) fn bic_checked(raw: &str) -> Result<String, CommandError> {
    checked_bic(raw)
}
