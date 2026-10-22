//! Business units inside one company, and the books between them.
//!
//! A business unit (a venture, a department, a shared-cost centre) is a
//! `dimension_value` of kind `unit` that a company keeps as its own books
//! view. Units are not companies: they share one chart, one set of
//! journals and one gapless numbering. What a unit adds is a code, a name,
//! a colour, an optional logo, and three guarantees this module and the
//! posting engine enforce together:
//!
//! 1. **Each unit's lines balance.** A unit with `own_books` gets clearing
//!    lines on the chart's `inter-unit-clearing` account from
//!    [`crate::dimension::clearing_lines_for_units`] (the posting engine
//!    does this; see [`crate::posting`]).
//! 2. **Profit and loss is attributed.** When a company has at least one
//!    active unit, every income and expense line, and every line on an
//!    inter-unit account, must carry a unit (a posting guard).
//! 3. **Movements between units are intercompany.** An entry that touches
//!    an account tagged `inter-unit-due-from` or `inter-unit-due-to` is
//!    intercompany. [`plan_transfer`] builds the shared-cost recharge: the
//!    receiving unit books the cost against `inter-unit-due-to`, the
//!    paying unit books the recovery against `inter-unit-due-from`, and
//!    both legs balance per unit. A posting guard keeps an intercompany
//!    entry's profit and loss netting to zero, so that the consolidated
//!    view can drop the whole entry without changing company totals (see
//!    [`UnitScope`]).
//!
//! Every write here is one batch: the change, its guard statements and its
//! `unit.*` audit event, the same shape as the chart commands
//! ([`crate::commands`]).

use crate::commands::change_with_audit;
use crate::entry::{EntryDraft, Line, Side};
use crate::error::AccountsError;
use cratefield_core::{Database, Statement};
use ledgers_audit::{AuditContext, AuditRecord, Outcome};
use ledgers_db::guard;
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;

/// The `dimension_value` kind of a business unit.
pub const UNIT_KIND: &str = "unit";
/// The chart tag of the receivable a unit holds on other units.
pub const TAG_DUE_FROM: &str = "inter-unit-due-from";
/// The chart tag of the payable a unit owes other units.
pub const TAG_DUE_TO: &str = "inter-unit-due-to";
/// The chart tag of the clearing account per-unit balancing uses.
pub const TAG_CLEARING: &str = "inter-unit-clearing";
/// The colour a unit gets when the caller names none.
pub const DEFAULT_COLOUR: &str = "#7FA6C9";

/// The audited command names of the unit writes.
pub const COMMAND_CREATE_UNIT: &str = "unit.create";
/// The audited command name of a rename (and recolour).
pub const COMMAND_RENAME_UNIT: &str = "unit.rename";
/// The audited command name of an archive.
pub const COMMAND_ARCHIVE_UNIT: &str = "unit.archive";

/// Longest unit name, in characters.
const NAME_MAX: usize = 80;
/// Shortest and longest unit codes, in ASCII alphanumerics.
const CODE_MIN: usize = 2;
const CODE_MAX: usize = 16;

/// A business unit as the books carry it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Unit {
    /// The short code (2 to 16 ASCII alphanumerics, upper case). Entry
    /// lines name the unit by this code.
    pub code: String,
    /// The name shown to people.
    pub name: String,
    /// The colour, `#RRGGBB`.
    pub colour: String,
    /// The logo document, when one is set.
    pub logo_document_id: Option<String>,
    /// Whether the unit is retired (hidden from new postings and the switcher).
    pub archived: bool,
    /// When the unit was created (ISO 8601 UTC).
    pub created_at: String,
}

/// What a unit is created with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnitDraft {
    /// The short code; validated and upper-cased.
    pub code: String,
    /// The name; trimmed, 1 to 80 characters.
    pub name: String,
    /// The colour, `#RRGGBB`; `None` takes [`DEFAULT_COLOUR`].
    pub colour: Option<String>,
    /// An existing document to use as the logo.
    pub logo_document_id: Option<String>,
}

/// The reporting scope: the whole company, or one business unit.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum UnitScope {
    /// Company totals. Intercompany entries are eliminated.
    #[default]
    Consolidated,
    /// One unit's books: the lines that carry this unit code, intercompany
    /// entries included.
    Unit(String),
}

impl UnitScope {
    /// Parses the wire form: `None` or `all` is consolidated; anything else
    /// is a unit code (validated against the company's units by the caller).
    ///
    /// # Errors
    /// [`AccountsError::InvalidUnit`] when the code is not a valid code.
    pub fn parse(value: Option<&str>) -> Result<Self, AccountsError> {
        match value.map(str::trim) {
            None | Some("") => Ok(Self::Consolidated),
            Some(code) if code.eq_ignore_ascii_case("all") => Ok(Self::Consolidated),
            Some(code) => Ok(Self::Unit(validate_code(code)?)),
        }
    }

    /// The SQL condition that keeps the lines of this scope, for a query
    /// over `journal_line` aliased `jl` and `journal_entry` aliased `je`.
    /// Returns the condition (empty when none is needed) and its values.
    ///
    /// Consolidated scope drops intercompany entries whole. A unit scope
    /// keeps the lines carrying the unit code, intercompany ones included.
    #[must_use]
    pub fn line_filter(&self) -> (String, Vec<SeaValue>) {
        match self {
            Self::Consolidated => (
                format!(
                    " AND NOT EXISTS (SELECT 1 FROM journal_line ix \
                     JOIN account xa ON xa.company_id = je.company_id AND xa.number = ix.account_number \
                     WHERE ix.entry_id = je.id AND (xa.tags LIKE '%{TAG_DUE_FROM}%' OR xa.tags LIKE '%{TAG_DUE_TO}%'))"
                ),
                vec![],
            ),
            Self::Unit(code) => (
                " AND EXISTS (SELECT 1 FROM journal_line_dimension jld \
                 WHERE jld.company_id = je.company_id AND jld.entry_id = jl.entry_id \
                 AND jld.line_no = jl.line_no AND jld.kind = 'unit' AND jld.value = ?)"
                    .to_owned(),
                vec![SeaValue::from(code.as_str())],
            ),
        }
    }
}

/// Validates a unit code: 2 to 16 ASCII alphanumerics, returned upper-cased.
///
/// # Errors
/// [`AccountsError::InvalidUnit`] when the code breaks the rule.
pub fn validate_code(code: &str) -> Result<String, AccountsError> {
    let trimmed = code.trim();
    let length = trimmed.chars().count();
    if !(CODE_MIN..=CODE_MAX).contains(&length)
        || !trimmed.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(AccountsError::InvalidUnit {
            reason: "a unit code is 2 to 16 letters or digits".to_owned(),
        });
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper == "ALL" {
        return Err(AccountsError::InvalidUnit {
            reason: "ALL names the consolidated view, not a unit".to_owned(),
        });
    }
    Ok(upper)
}

/// Validates a unit name: trimmed, 1 to 80 characters, no control characters.
///
/// # Errors
/// [`AccountsError::InvalidUnit`] when the name breaks the rule.
pub fn validate_name(name: &str) -> Result<String, AccountsError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > NAME_MAX {
        return Err(AccountsError::InvalidUnit {
            reason: format!("a unit name is 1 to {NAME_MAX} characters"),
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AccountsError::InvalidUnit {
            reason: "a unit name may not contain control characters".to_owned(),
        });
    }
    Ok(trimmed.to_owned())
}

/// Validates a colour: `#` and six hex digits, returned upper-cased.
///
/// # Errors
/// [`AccountsError::InvalidUnit`] when the colour is not `#RRGGBB`.
pub fn validate_colour(colour: &str) -> Result<String, AccountsError> {
    let trimmed = colour.trim();
    let valid = trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err(AccountsError::InvalidUnit {
            reason: "a unit colour is #RRGGBB".to_owned(),
        });
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// Lists the company's units, by code. Archived units only when asked.
///
/// # Errors
/// [`AccountsError::Database`] when the query fails.
pub async fn list_units(
    db: &dyn Database,
    company_id: &str,
    include_archived: bool,
) -> Result<Vec<Unit>, AccountsError> {
    let mut sql = format!("{SELECT_UNIT} WHERE company_id = ? AND kind = ?");
    if !include_archived {
        sql.push_str(" AND archived = 0");
    }
    sql.push_str(" ORDER BY code");
    let rows = db
        .query(&Statement::with_values(
            sql,
            vec![SeaValue::from(company_id), SeaValue::from(UNIT_KIND)],
        ))
        .await?;
    rows.rows.iter().map(unit_from_row).collect()
}

/// One unit by code, archived or not.
///
/// # Errors
/// [`AccountsError::Database`] when the query fails.
pub async fn find_unit(
    db: &dyn Database,
    company_id: &str,
    code: &str,
) -> Result<Option<Unit>, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            format!("{SELECT_UNIT} WHERE company_id = ? AND kind = ? AND code = ?"),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(UNIT_KIND),
                SeaValue::from(code),
            ],
        ))
        .await?;
    rows.rows.first().map(unit_from_row).transpose()
}

/// Whether the company has units enabled: at least one active unit. While
/// it does, posting rules require a unit on every profit-and-loss line.
///
/// # Errors
/// [`AccountsError::Database`] when the query fails.
pub async fn units_enabled(db: &dyn Database, company_id: &str) -> Result<bool, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS n FROM dimension_value WHERE company_id = ? AND kind = ? AND archived = 0"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(UNIT_KIND)],
        ))
        .await?;
    let count: i64 = rows.rows.first().and_then(|row| row.get("n")).unwrap_or(0);
    Ok(count > 0)
}

const SELECT_UNIT: &str = "SELECT code, name, colour, logo_document_id, archived, created_at \
     FROM dimension_value";

fn unit_from_row(row: &cratefield_core::Row) -> Result<Unit, AccountsError> {
    let column = |name: &str| {
        AccountsError::Database(cratefield_core::DbError::Query(format!(
            "dimension_value.{name} missing"
        )))
    };
    let archived: i64 = row.get("archived").ok_or_else(|| column("archived"))?;
    Ok(Unit {
        code: row.get("code").ok_or_else(|| column("code"))?,
        name: row.get("name").ok_or_else(|| column("name"))?,
        colour: row.get("colour").ok_or_else(|| column("colour"))?,
        logo_document_id: row.get("logo_document_id"),
        archived: archived != 0,
        created_at: row.get("created_at").ok_or_else(|| column("created_at"))?,
    })
}

/// The one active account a chart tag names. Zero matches and several
/// matches are both refusals: the chart must name exactly one account.
///
/// # Errors
/// [`AccountsError::ChartTagMissing`] or [`AccountsError::ChartTagAmbiguous`]
/// when the tag does not name exactly one active account;
/// [`AccountsError::Database`] when the query fails.
pub async fn account_for_tag(
    db: &dyn Database,
    company_id: &str,
    tag: &str,
) -> Result<(String, String), AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT number, account_type FROM account \
             WHERE company_id = ? AND archived = 0 AND tags LIKE ? ORDER BY number"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(format!("%{tag}%")),
            ],
        ))
        .await?;
    match rows.rows.as_slice() {
        [] => Err(AccountsError::ChartTagMissing {
            tag: tag.to_owned(),
        }),
        [row] => {
            let number: String = row.get("number").ok_or_else(|| {
                AccountsError::Database(cratefield_core::DbError::Query(
                    "account.number missing".to_owned(),
                ))
            })?;
            let account_type: String = row.get("account_type").ok_or_else(|| {
                AccountsError::Database(cratefield_core::DbError::Query(
                    "account.account_type missing".to_owned(),
                ))
            })?;
            Ok((number, account_type))
        }
        _ => Err(AccountsError::ChartTagAmbiguous {
            tag: tag.to_owned(),
        }),
    }
}

/// Creates a business unit. The chart must carry the inter-unit clearing
/// account first, because a unit with its own books needs it to balance.
///
/// # Errors
/// [`AccountsError::InvalidUnit`] for a bad code, name, colour or logo;
/// [`AccountsError::UnitExists`] when the code is taken;
/// [`AccountsError::ChartTagMissing`] when the chart has no clearing account;
/// [`AccountsError::Database`] when the write fails.
pub async fn create_unit(
    db: &dyn Database,
    company_id: &str,
    draft: UnitDraft,
    now: String,
    audit: &AuditContext,
) -> Result<Unit, AccountsError> {
    let code = validate_code(&draft.code)?;
    let name = validate_name(&draft.name)?;
    let colour = validate_colour(draft.colour.as_deref().unwrap_or(DEFAULT_COLOUR))?;
    let logo = validate_logo(draft.logo_document_id.as_deref())?;
    if find_unit(db, company_id, &code).await?.is_some() {
        return Err(AccountsError::UnitExists { code });
    }
    account_for_tag(db, company_id, TAG_CLEARING).await?;

    let mut statements = logo_guards(company_id, logo.as_deref());
    statements.push(Statement::with_values(
        "INSERT INTO dimension_value (company_id, kind, code, name, own_books, created_at, colour, logo_document_id, archived) \
         VALUES (?, ?, ?, ?, 1, ?, ?, ?, 0)"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(UNIT_KIND),
            SeaValue::from(code.as_str()),
            SeaValue::from(name.as_str()),
            SeaValue::from(now.as_str()),
            SeaValue::from(colour.as_str()),
            SeaValue::from(logo.clone()),
        ],
    ));
    let input = serde_json::json!({ "code": code, "name": name, "colour": colour, "logo": logo });
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            COMMAND_CREATE_UNIT,
            &input,
            Outcome::Allowed,
            vec![code.clone()],
            None,
        ),
        &now,
        &statements,
    )
    .await?;
    find_unit(db, company_id, &code)
        .await?
        .ok_or(AccountsError::UnitNotFound { code })
}

/// Renames a unit and optionally changes its colour and logo. An archived
/// unit is refused: restoring one is not a supported action.
///
/// # Errors
/// [`AccountsError::UnitNotFound`], [`AccountsError::UnitArchived`],
/// [`AccountsError::InvalidUnit`], or [`AccountsError::Database`].
pub async fn rename_unit(
    db: &dyn Database,
    company_id: &str,
    code: &str,
    change: UnitChange,
    now: String,
    audit: &AuditContext,
) -> Result<Unit, AccountsError> {
    let code = validate_code(code)?;
    let current = active_unit(db, company_id, &code).await?;
    let name = match change.name.as_deref() {
        Some(name) => validate_name(name)?,
        None => current.name.clone(),
    };
    let colour = match change.colour.as_deref() {
        Some(colour) => validate_colour(colour)?,
        None => current.colour.clone(),
    };
    let logo = match change.logo_document_id {
        Some(logo) => validate_logo(logo.as_deref())?,
        None => current.logo_document_id.clone(),
    };
    let mut statements = logo_guards(company_id, logo.as_deref());
    statements.push(guard_active_unit(company_id, &code));
    statements.push(Statement::with_values(
        "UPDATE dimension_value SET name = ?, colour = ?, logo_document_id = ? \
         WHERE company_id = ? AND kind = ? AND code = ?"
            .to_owned(),
        vec![
            SeaValue::from(name.as_str()),
            SeaValue::from(colour.as_str()),
            SeaValue::from(logo.clone()),
            SeaValue::from(company_id),
            SeaValue::from(UNIT_KIND),
            SeaValue::from(code.as_str()),
        ],
    ));
    statements.push(guard::release_statement(active_unit_guard_key(&code)));
    let input = serde_json::json!({ "code": code, "name": name, "colour": colour, "logo": logo });
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            COMMAND_RENAME_UNIT,
            &input,
            Outcome::Allowed,
            vec![code.clone()],
            None,
        ),
        &now,
        &statements,
    )
    .await?;
    active_unit_after(db, company_id, &code).await
}

/// Archives a unit: it leaves the switcher and stops taking new postings,
/// and its history stays exactly as posted.
///
/// # Errors
/// [`AccountsError::UnitNotFound`], [`AccountsError::UnitArchived`] when it
/// is already archived, or [`AccountsError::Database`].
pub async fn archive_unit(
    db: &dyn Database,
    company_id: &str,
    code: &str,
    now: String,
    audit: &AuditContext,
) -> Result<Unit, AccountsError> {
    let code = validate_code(code)?;
    active_unit(db, company_id, &code).await?;
    let statements = vec![
        guard_active_unit(company_id, &code),
        Statement::with_values(
            "UPDATE dimension_value SET archived = 1 \
             WHERE company_id = ? AND kind = ? AND code = ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(UNIT_KIND),
                SeaValue::from(code.as_str()),
            ],
        ),
        guard::release_statement(active_unit_guard_key(&code)),
    ];
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            COMMAND_ARCHIVE_UNIT,
            &serde_json::json!({ "code": code }),
            Outcome::Allowed,
            vec![code.clone()],
            None,
        ),
        &now,
        &statements,
    )
    .await?;
    active_unit_after(db, company_id, &code).await
}

/// The changes a rename may carry. `None` keeps the field; a logo of
/// `Some(None)` clears it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct UnitChange {
    /// A new name.
    pub name: Option<String>,
    /// A new colour, `#RRGGBB`.
    pub colour: Option<String>,
    /// A new logo document, or `Some(None)` to clear it.
    pub logo_document_id: Option<Option<String>>,
}

async fn active_unit(
    db: &dyn Database,
    company_id: &str,
    code: &str,
) -> Result<Unit, AccountsError> {
    let unit =
        find_unit(db, company_id, code)
            .await?
            .ok_or_else(|| AccountsError::UnitNotFound {
                code: code.to_owned(),
            })?;
    if unit.archived {
        return Err(AccountsError::UnitArchived {
            code: code.to_owned(),
        });
    }
    Ok(unit)
}

async fn active_unit_after(
    db: &dyn Database,
    company_id: &str,
    code: &str,
) -> Result<Unit, AccountsError> {
    find_unit(db, company_id, code)
        .await?
        .ok_or_else(|| AccountsError::UnitNotFound {
            code: code.to_owned(),
        })
}

fn validate_logo(logo: Option<&str>) -> Result<Option<String>, AccountsError> {
    match logo.map(str::trim) {
        None | Some("") => Ok(None),
        Some(id) if id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric()) => {
            Ok(Some(id.to_owned()))
        }
        Some(_) => Err(AccountsError::InvalidUnit {
            reason: "a logo must name a stored document".to_owned(),
        }),
    }
}

/// The probe pair that proves a logo names a document of this company:
/// an assert and its release, in the same batch. No logo, no probe.
fn logo_guards(company_id: &str, logo: Option<&str>) -> Vec<Statement> {
    match logo {
        None => Vec::new(),
        Some(id) => {
            let key = guard::guard_key(&["unit-logo", id]);
            vec![
                guard::assert_statement(
                    key.clone(),
                    "(SELECT COUNT(*) FROM document WHERE company_id = ? AND id = ?) = 1",
                    vec![SeaValue::from(company_id), SeaValue::from(id)],
                ),
                guard::release_statement(key),
            ]
        }
    }
}

fn active_unit_guard_key(code: &str) -> String {
    guard::guard_key(&["unit-active", code])
}

fn guard_active_unit(company_id: &str, code: &str) -> Statement {
    guard::assert_statement(
        active_unit_guard_key(code),
        "(SELECT COUNT(*) FROM dimension_value WHERE company_id = ? AND kind = ? AND code = ? AND archived = 0) = 1",
        vec![
            SeaValue::from(company_id),
            SeaValue::from(UNIT_KIND),
            SeaValue::from(code),
        ],
    )
}

/// One recharge between two units: the paying unit recovers a shared cost
/// from the receiving unit. Amounts are in the company's base currency.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TransferDraft {
    /// The journal the entry posts into.
    pub journal_id: String,
    /// The unit that paid the shared cost and recovers it.
    pub from_unit: String,
    /// The unit the cost is recharged to.
    pub to_unit: String,
    /// The amount, in the company's base currency, positive.
    pub amount: Amount,
    /// The document date (YYYY-MM-DD).
    pub document_date: time::Date,
    /// The posting date (YYYY-MM-DD).
    pub posting_date: time::Date,
    /// The tax point date; `None` takes the posting date.
    pub tax_point_date: Option<time::Date>,
    /// The description.
    pub description: String,
    /// The income account on the paying unit that books the recovery.
    pub recovery_account: String,
    /// The expense account on the receiving unit that books the cost.
    pub cost_account: String,
    /// The company's base currency.
    pub base_currency: Currency,
}

/// Builds the four-line intercompany entry for a recharge, after checking
/// both units are active and distinct, the amount is positive, and the
/// two accounts are an income and an expense. The per-unit legs are:
///
/// - receiving unit: debit the cost account, credit `inter-unit-due-to`;
/// - paying unit: debit `inter-unit-due-from`, credit the recovery account.
///
/// Each unit's two lines net to zero, so the entry balances per unit and
/// its profit and loss nets to zero: the consolidated view eliminates it.
///
/// # Errors
/// [`AccountsError::InvalidUnit`] for a bad unit or amount,
/// [`AccountsError::UnitNotFound`] / [`AccountsError::UnitArchived`] for an
/// unknown or retired unit, [`AccountsError::TransferAccount`] when an
/// account has the wrong class, [`AccountsError::ChartTagMissing`] /
/// [`AccountsError::ChartTagAmbiguous`] when the chart lacks the due pair,
/// or [`AccountsError::Database`].
pub async fn plan_transfer(
    db: &dyn Database,
    company_id: &str,
    draft: &TransferDraft,
) -> Result<EntryDraft, AccountsError> {
    let from = validate_code(&draft.from_unit)?;
    let to = validate_code(&draft.to_unit)?;
    if from == to {
        return Err(AccountsError::InvalidUnit {
            reason: "a transfer needs two different units".to_owned(),
        });
    }
    if draft.amount.minor() <= 0 {
        return Err(AccountsError::InvalidUnit {
            reason: "a transfer amount is positive".to_owned(),
        });
    }
    if draft.amount.currency() != draft.base_currency {
        return Err(AccountsError::InvalidUnit {
            reason: "a transfer is posted in the company's base currency".to_owned(),
        });
    }
    active_unit(db, company_id, &from).await?;
    active_unit(db, company_id, &to).await?;
    let (recovery, recovery_type) = (
        draft.recovery_account.as_str(),
        account_type_of(db, company_id, &draft.recovery_account).await?,
    );
    if recovery_type != "income" {
        return Err(AccountsError::TransferAccount {
            number: recovery.to_owned(),
            expected: "income".to_owned(),
        });
    }
    let (cost, cost_type) = (
        draft.cost_account.as_str(),
        account_type_of(db, company_id, &draft.cost_account).await?,
    );
    if cost_type != "expense" {
        return Err(AccountsError::TransferAccount {
            number: cost.to_owned(),
            expected: "expense".to_owned(),
        });
    }
    let (due_from, _) = account_for_tag(db, company_id, TAG_DUE_FROM).await?;
    let (due_to, _) = account_for_tag(db, company_id, TAG_DUE_TO).await?;

    let leg = |account: &str, side: Side, unit: &str| Line {
        account_number: account.to_owned(),
        side,
        base: draft.amount,
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::from([("unit".to_owned(), unit.to_owned())]),
        memo: None,
    };
    let lines = vec![
        leg(&draft.cost_account, Side::Debit, &to),
        leg(&due_to, Side::Credit, &to),
        leg(&due_from, Side::Debit, &from),
        leg(&draft.recovery_account, Side::Credit, &from),
    ];
    Ok(EntryDraft {
        journal_id: draft.journal_id.clone(),
        document_date: draft.document_date,
        posting_date: draft.posting_date,
        tax_point_date: draft.tax_point_date,
        base_currency: draft.base_currency,
        description: draft.description.clone(),
        source_documents: Vec::new(),
        external_id: None,
        source_app: None,
        reverses: None,
        lines,
    })
}

async fn account_type_of(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<String, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT account_type FROM account WHERE company_id = ? AND number = ? AND archived = 0"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(number)],
        ))
        .await?;
    let row = rows
        .rows
        .first()
        .ok_or_else(|| AccountsError::AccountNotFound {
            number: number.to_owned(),
        })?;
    row.get("account_type").ok_or_else(|| {
        AccountsError::Database(cratefield_core::DbError::Query(
            "account.account_type missing".to_owned(),
        ))
    })
}
