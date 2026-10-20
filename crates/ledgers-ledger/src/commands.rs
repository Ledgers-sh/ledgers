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
use cratefield_core::{Database, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements, record};
use ledgers_db::chain;
use ledgers_db::retry::{self, RetryPolicy};
use std::time::Duration;

/// What [`import_template`] did: which template, and the account numbers
/// it created (sorted).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportReport {
    /// The imported template's id.
    pub template_id: String,
    /// The numbers of the accounts created, ascending.
    pub created: Vec<String>,
}

/// The library's sleep: none. Retries re-plan without backing off — the
/// wasm-clean rule (ADR 0004) is that the library never sleeps or
/// blocks; callers that can block pass their own sleeper.
fn no_sleep(_: Duration) {}

/// Runs one command's change and its audit event as **one** batch —
/// `[change…, audit]`, the audit last so it exists exactly when the
/// change does — re-reading the audit head inside the plan closure so a
/// fork (a unique violation on `(company_id, seq)`, the one failure
/// class ADR 0004 allows a retry for) re-plans onto a fresh chain.
async fn change_with_audit(
    db: &dyn Database,
    company_id: &str,
    audit: &AuditContext,
    record: &AuditRecord,
    at: &str,
    change: &[Statement],
) -> Result<(), AccountsError> {
    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let (audit, record, change) = (audit.clone(), record.clone(), change.to_vec());
            let at = at.to_owned();
            async move {
                let head = chain::read_head(db, company_id).await?;
                let appended = append_statements(
                    company_id,
                    &head,
                    &audit,
                    &record,
                    &at,
                    &AuditConfig::default(),
                )
                .map_err(|err| cratefield_core::DbError::Query(err.to_string()))?;
                let mut statements = change;
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        no_sleep,
    )
    .await?;
    Ok(())
}

/// The one `denied` event a refused command owes the chain, written as
/// its own batch — nothing else changed, so there is nothing to ride
/// along with.
///
/// A failure to record the refusal is returned to the caller in place of
/// the refusal: an unaudited denial is the worse outcome of the two, and
/// a command that "probably denied" is not a decision anybody can rely
/// on.
async fn deny(
    db: &dyn Database,
    company_id: &str,
    audit: &AuditContext,
    audit_record: &AuditRecord,
    at: &str,
) -> Result<(), AccountsError> {
    record(
        db,
        &AuditConfig::default(),
        company_id,
        audit,
        audit_record,
        at,
        RetryPolicy::default(),
        no_sleep,
    )
    .await
    .map(|_| ())
    .map_err(|err| AccountsError::Database(cratefield_core::DbError::Query(err.to_string())))
}

/// The audited command name an account creation records under.
const COMMAND_CREATE_ACCOUNT: &str = "account.create";

/// Validates a draft and generates the create statement (shared helper).
///
/// # Errors
/// [`AccountsError::InvalidNumber`], [`AccountsError::InvalidTag`] and
/// [`AccountsError::InvalidNameKey`] from draft validation;
/// [`AccountsError::ParentNotFound`] when the parent is absent;
/// [`AccountsError::AccountExists`] when the number is taken;
/// [`AccountsError::Database`] otherwise.
async fn validate_and_create_statement(
    db: &dyn Database,
    company_id: &str,
    draft: &AccountDraft,
    now: &str,
) -> Result<(Account, Vec<Statement>), AccountsError> {
    let account = draft.clone().into_account(company_id, now)?;
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
    let statements = vec![store::insert_account_statement(&account)];
    Ok((account, statements))
}

/// Plans one account creation: validates the draft, requires the parent to exist
/// (when the draft names one), and returns the SQL statements and the account entry.
///
/// This is the token-taking planner: the executor calls this to get statements,
/// then appends the audit event in the same batch.
///
/// # Errors
/// [`AccountsError::InvalidNumber`], [`AccountsError::InvalidTag`] and
/// [`AccountsError::InvalidNameKey`] from draft validation;
/// [`AccountsError::ParentNotFound`] when the parent is absent;
/// [`AccountsError::AccountExists`] when the number is taken;
/// [`AccountsError::Database`] otherwise.
pub async fn plan_create_account(
    _token: &ledgers_commands::ExecToken,
    db: &dyn Database,
    company_id: &str,
    draft: &AccountDraft,
    now: &str,
) -> Result<(Vec<Statement>, AccountEntry), AccountsError> {
    let (account, statements) = validate_and_create_statement(db, company_id, draft, now).await?;
    // A newly created account has no children yet, so has_children is false.
    let entry = AccountEntry {
        account,
        has_children: false,
    };
    Ok((statements, entry))
}

/// Creates one account: validates the draft, requires the parent to exist
/// (when the draft names one), inserts, and returns the re-read entry so
/// the caller sees the derived postable state, not just what it sent.
///
/// The insert and its `account.create` audit event are **one** batch,
/// so an account never exists without the record that created it.
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
    audit: &AuditContext,
) -> Result<AccountEntry, AccountsError> {
    let (account, statements) = validate_and_create_statement(db, company_id, &draft, &now).await?;
    let input = account_draft_input(&account);
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            COMMAND_CREATE_ACCOUNT,
            &input,
            Outcome::Allowed,
            vec![account.number.clone()],
            None,
        ),
        &now,
        &statements,
    )
    .await?;
    entry_for(db, company_id, &account.number).await
}

/// The `account.create` input: the draft as it was validated. Only its
/// hash is stored — an account's name key is chart data, not an input
/// anybody needs to read back out of the audit log.
fn account_draft_input(account: &Account) -> serde_json::Value {
    serde_json::json!({
        "number": account.number,
        "nameKey": account.name_key,
        "accountType": account.account_type.to_string(),
        "normalBalance": account.normal_balance.to_string(),
        "currency": account.currency.map(|code| code.code().to_owned()),
        "parent": account.parent,
        "tags": account.tags,
    })
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
/// returns it unchanged (and writes no audit event — nothing changed).
///
/// The `archived` update and its `account.archive` audit event are one
/// batch.
///
/// # Errors
/// [`AccountsError::AccountNotFound`] when the account does not exist;
/// [`AccountsError::Database`] otherwise.
pub async fn archive_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
    now: String,
    audit: &AuditContext,
) -> Result<AccountEntry, AccountsError> {
    let existing = entry_for(db, company_id, number).await?;
    if existing.account.archived {
        return Ok(existing);
    }
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            "account.archive",
            &serde_json::json!({ "number": number }),
            Outcome::Allowed,
            vec![number.to_owned()],
            None,
        ),
        &now,
        &[store::set_archived_statement(
            company_id, number, true, &now,
        )],
    )
    .await?;
    entry_for(db, company_id, number).await
}

/// Deleting accounts is not an operation; this thin wrapper over
/// [`store::delete_account`] exists so the command surface encodes
/// "deleting fails" next to create/show/list/archive, and so the future
/// registry can expose the guaranteed failure as a documented outcome.
///
/// The refusal is not silent: it writes exactly one `denied`
/// `account.delete` event, so an agent that keeps asking gets a trail of
/// the attempts.
///
/// # Errors
/// Always [`AccountsError::DeleteUnsupported`] — or, when that event
/// cannot be written, the database error, which is the more important
/// one to surface.
pub async fn delete_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
    now: String,
    audit: &AuditContext,
) -> Result<(), AccountsError> {
    deny(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            "account.delete",
            &serde_json::json!({ "number": number }),
            Outcome::Denied,
            vec![number.to_owned()],
            None,
        )
        .with_reason("accounts are never deleted; archive instead"),
        &now,
    )
    .await?;
    Err(AccountsError::DeleteUnsupported)
}

/// Imports a chart template into a company's books: lints the chart
/// (every violation reported together), refuses when any of its numbers
/// already exist — imports are for fresh ledgers; extending a live chart
/// is account-by-account `create_account` — then inserts the whole chart
/// as one atomic batch, parents before children, with the
/// `account.import_template` audit event appended after the inserts in
/// that same batch.
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
    audit: &AuditContext,
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
    // The chart brings exactly one carrier of every required system tag
    // (lint above), so an existing account already carrying one would
    // leave the company with two. Refuse that before writing: the import
    // batch is atomic, but the post-import check below runs after it has
    // committed and could only report the problem, not undo it.
    let duplicated: Vec<Violation> = REQUIRED_SYSTEM_TAGS
        .iter()
        .filter(|tag| existing.iter().any(|row| row.has_tag(tag)))
        .map(|&tag| Violation {
            kind: ViolationKind::DuplicateSystemAccount { tag },
            account: None,
        })
        .collect();
    if !duplicated.is_empty() {
        return Err(AccountsError::TemplateRejected {
            violations: duplicated,
        });
    }

    let accounts: Vec<Account> = chart
        .in_dependency_order()
        .iter()
        .map(|account| account.to_account(company_id, &now))
        .collect();
    let inserts: Vec<Statement> = accounts
        .iter()
        .map(store::insert_account_statement)
        .collect();
    // The audit event joins the inserts rather than following them in a
    // second batch: a chart must never be half-imported with no record,
    // nor recorded as imported before it was.
    change_with_audit(
        db,
        company_id,
        audit,
        &AuditRecord::from_input(
            "account.import_template",
            &serde_json::json!({ "template": chart.id }),
            Outcome::Allowed,
            vec![chart.id.clone()],
            None,
        ),
        &now,
        &inserts,
    )
    .await?;

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
