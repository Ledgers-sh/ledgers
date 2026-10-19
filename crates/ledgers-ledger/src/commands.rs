//! The account commands: the library functions the future
//! `ledgers-commands` registry (M1) wraps with policy, audit and routing.
//! Everything here takes `&dyn Database` and an explicit `now` (ISO 8601
//! UTC) — no clock, no ids and no policy decisions live in this crate.
//!
//! The contract this layer owns:
//!
//! - parents before children, always — a draft whose parent is missing is
//!   [`AccountsError::ParentNotFound`], and imports order the chart so a
//!   child never precedes its parent;
//! - numbers are unique per company (the primary key is the final word;
//!   these functions pre-check so the common case gets
//!   [`AccountsError::AccountExists`] instead of a driver message);
//! - accounts are archived, never deleted — [`delete_account`] always
//!   fails, on purpose (docs/PLAN.md §3 rule 3);
//! - validation is server-side and total: number, tags, name-key prefix
//!   and (at parse time, wherever a code string is accepted) currency
//!   via `ledgers-money`.

use crate::account::{Account, AccountDraft, AccountEntry};
use crate::chart::ChartTemplate;
use crate::error::AccountsError;
use crate::lint::{self, REQUIRED_SYSTEM_TAGS, Violation, ViolationKind};
use crate::store;
use cratefield_core::Database;

/// What [`import_template`] did: which template, and the account numbers
/// it created (sorted).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportReport {
    /// The imported template's id.
    pub template_id: String,
    /// The numbers of the accounts created, ascending.
    pub created: Vec<String>,
}

/// Creates one account: validates the draft, requires the parent to exist
/// (when the draft names one), inserts, and returns the re-read entry so
/// the caller sees the derived postable state, not just what it sent.
///
/// The new node cannot create a cycle: it has no children yet, so its
/// parent link can only attach below an existing acyclic tree.
///
/// # Errors
/// [`AccountsError::InvalidNumber`], [`AccountsError::InvalidTag`] and
/// [`AccountsError::InvalidNameKey`] from draft validation;
/// [`AccountsError::ParentNotFound`] when the parent is absent;
/// [`AccountsError::AccountExists`] when the number is taken;
/// [`AccountsError::Database`] otherwise.
pub async fn create_account(
    db: &dyn Database,
    company_id: &str,
    draft: AccountDraft,
    now: String,
) -> Result<AccountEntry, AccountsError> {
    let account = draft.into_account(company_id, &now)?;
    if let Some(parent) = &account.parent
        && store::find_account(db, company_id, parent).await?.is_none()
    {
        return Err(AccountsError::ParentNotFound {
            parent: parent.clone(),
        });
    }
    if store::find_account(db, company_id, &account.number)
        .await?
        .is_some()
    {
        return Err(AccountsError::AccountExists {
            number: account.number.clone(),
        });
    }
    store::insert_account(db, &account).await?;
    entry_for(db, company_id, &account.number).await
}

/// Returns one account as an entry (with its derived postable state).
///
/// # Errors
/// [`AccountsError::AccountNotFound`] when the account does not exist;
/// [`AccountsError::Database`] otherwise.
pub async fn show_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<AccountEntry, AccountsError> {
    entry_for(db, company_id, number).await
}

/// Lists the company's accounts by number, optionally filtered to
/// unarchived ones. Entries carry the derived postable state.
///
/// # Errors
/// [`AccountsError::Database`] when a statement fails.
pub async fn list_accounts(
    db: &dyn Database,
    company_id: &str,
    include_archived: bool,
) -> Result<Vec<AccountEntry>, AccountsError> {
    let entries = store::list_account_entries(db, company_id).await?;
    if include_archived {
        Ok(entries)
    } else {
        Ok(entries
            .into_iter()
            .filter(|entry| !entry.account.archived)
            .collect())
    }
}

/// Archives an account so it accepts no new postings but keeps its
/// history. Works regardless of postings — that is the point of
/// archiving: posted entries are immutable, so the account they reference
/// is retired, never removed. Idempotent: archiving an archived account
/// returns it unchanged.
///
/// # Errors
/// [`AccountsError::AccountNotFound`] when the account does not exist;
/// [`AccountsError::Database`] otherwise.
pub async fn archive_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
    now: String,
) -> Result<AccountEntry, AccountsError> {
    let existing = entry_for(db, company_id, number).await?;
    if !existing.account.archived {
        store::set_archived(db, company_id, number, true, &now).await?;
        return entry_for(db, company_id, number).await;
    }
    Ok(existing)
}

/// Deleting accounts is not an operation; this thin wrapper over
/// [`store::delete_account`] exists so the command surface encodes
/// "deleting fails" next to create/show/list/archive, and so the future
/// registry can expose the guaranteed failure as a documented outcome.
///
/// # Errors
/// Always [`AccountsError::DeleteUnsupported`].
pub async fn delete_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<(), AccountsError> {
    store::delete_account(db, company_id, number).await
}

/// Imports a chart template into a company's books: lints the chart
/// (every violation reported together), refuses when any of its numbers
/// already exist — imports are for fresh ledgers; extending a live chart
/// is account-by-account `create_account` — then inserts the whole chart
/// as one atomic batch, parents before children.
///
/// After the batch, the company is verified to carry each of
/// [`REQUIRED_SYSTEM_TAGS`] exactly once, re-read from the database
/// (tags are space-separated text, so there is no query-by-tag; the
/// verification counts what actually landed).
///
/// # Errors
/// [`AccountsError::TemplateRejected`] when lint fails or the post-import
/// verification finds a missing system account;
/// [`AccountsError::AccountExists`] when a chart number is already
/// present; [`AccountsError::Database`] otherwise.
pub async fn import_template(
    db: &dyn Database,
    company_id: &str,
    chart: &ChartTemplate,
    now: String,
) -> Result<ImportReport, AccountsError> {
    let violations = lint::lint_chart(chart);
    if !violations.is_empty() {
        return Err(AccountsError::TemplateRejected { violations });
    }

    let existing = store::list_accounts(db, company_id).await?;
    if let Some(clash) = chart
        .in_dependency_order()
        .iter()
        .find(|account| existing.iter().any(|row| row.number == account.number))
    {
        return Err(AccountsError::AccountExists {
            number: clash.number.clone(),
        });
    }

    let accounts: Vec<Account> = chart
        .in_dependency_order()
        .iter()
        .map(|account| account.to_account(company_id, &now))
        .collect();
    store::insert_accounts_batch(db, &accounts).await?;

    // The post-import invariant, checked against what the database now
    // holds rather than what the chart claimed.
    let stored = store::list_accounts(db, company_id).await?;
    let mut missing = Vec::new();
    for tag in REQUIRED_SYSTEM_TAGS {
        let carriers = stored.iter().filter(|a| a.has_tag(tag)).count();
        if carriers != 1 {
            missing.push(Violation {
                kind: ViolationKind::MissingSystemAccount { tag },
                account: None,
            });
        }
    }
    if !missing.is_empty() {
        return Err(AccountsError::TemplateRejected {
            violations: missing,
        });
    }

    let mut created: Vec<String> = chart.accounts.iter().map(|a| a.number.clone()).collect();
    created.sort_unstable();
    Ok(ImportReport {
        template_id: chart.id.clone(),
        created,
    })
}

/// Reads one account back as an entry — account row plus the derived
/// has-children bit — or [`AccountsError::AccountNotFound`].
async fn entry_for(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<AccountEntry, AccountsError> {
    let account = store::find_account(db, company_id, number)
        .await?
        .ok_or_else(|| AccountsError::AccountNotFound {
            number: number.to_owned(),
        })?;
    let has_children = store::has_children(db, company_id, number).await?;
    Ok(AccountEntry {
        account,
        has_children,
    })
}
