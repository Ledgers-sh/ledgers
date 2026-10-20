//! The journal domain model: the books an entry posts into, each with
//! its own gapless number sequence per fiscal year
//! (docs/PLAN.md §3 rule 5).
//!
//! Everything here is plain data plus validation and the small store
//! surface for the `journal` table (this issue keeps journals and their
//! entries in `ledgers-ledger` beside the chart, per docs/PLAN.md §6).
//! Posting lives in [`crate::posting`]; the entry lines themselves in
//! [`crate::entry`].
//!
//! The rules this module owns:
//!
//! - **A journal's identity is `(company_id, code)`** — the human code
//!   (`"SJ"`, `"BJ"`), unique per company; the ULID `id` is what
//!   numbering, entries and the API key off of (the storage
//!   conventions: ULID ids, natural keys for humans).
//! - **Codes are short, uppercase, stable** — 1..=16 ASCII
//!   uppercase letters, digits or hyphens, because journal codes appear
//!   on printed legal documents (`GoBD`) and in every report's filter
//!   surface; a code that can contain whitespace or case variants is a
//!   code an agent cannot quote back reliably.
//! - **`JournalKind` is data, not behaviour** — the five classical
//!   kinds. Which kinds may post what is policy (`ledgers-policy`,
//!   M1), never a hidden rule here; the year-end opening/closing kinds
//!   belong to the fiscal-year issue that produces them.

use crate::entry::JournalError;
use cratefield_core::{Database, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements};
use ledgers_db::chain;
use ledgers_db::retry::{self, RetryPolicy};
use sea_query::Value as SeaValue;
use serde::{Deserialize, Serialize};

/// The maximum length of a journal code.
pub const MAX_CODE_LEN: usize = 16;
/// The maximum length of a journal name.
pub const MAX_NAME_LEN: usize = 200;

/// What kind of business a journal records. Stored as lowercase strings
/// (`"general"`, `"sales"`, `"purchases"`, `"bank"`, `"cash"`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalKind {
    /// The catch-all journal (GJ); everything without a nicer home.
    General,
    /// Customer invoices and credit notes (sales journal, SJ).
    Sales,
    /// Vendor bills and their corrections (purchases journal, PJ).
    Purchases,
    /// Bank account movements reconciled against statements (BJ).
    Bank,
    /// Cash on hand (Kassenbuch; legally special in several
    /// jurisdictions — no gaps, no deletion, ever).
    Cash,
}

impl JournalKind {
    /// Every kind, in declaration order — for round-trip tests and
    /// pickers.
    #[must_use]
    pub const fn all() -> [JournalKind; 5] {
        [
            JournalKind::General,
            JournalKind::Sales,
            JournalKind::Purchases,
            JournalKind::Bank,
            JournalKind::Cash,
        ]
    }
}

impl std::fmt::Display for JournalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            JournalKind::General => "general",
            JournalKind::Sales => "sales",
            JournalKind::Purchases => "purchases",
            JournalKind::Bank => "bank",
            JournalKind::Cash => "cash",
        })
    }
}

impl std::str::FromStr for JournalKind {
    type Err = JournalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "general" => Ok(Self::General),
            "sales" => Ok(Self::Sales),
            "purchases" => Ok(Self::Purchases),
            "bank" => Ok(Self::Bank),
            "cash" => Ok(Self::Cash),
            _ => Err(JournalError::UnknownKind {
                value: s.to_owned(),
            }),
        }
    }
}

/// One journal of one company: the destination (and number sequence)
/// entries post into.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Journal {
    /// The company this journal belongs to (no cross-module foreign
    /// key — see the journal migration's comments).
    pub company_id: String,
    /// The journal's ULID id: what `numbering_counter`, `journal_entry`
    /// and the API mean by this journal.
    pub id: String,
    /// The human code, unique per company (`"SJ"`, `"BJ"`); validated
    /// by [`Journal::validate_code`].
    pub code: String,
    /// The kind — names the business the journal records.
    pub kind: JournalKind,
    /// The human-readable name ("Sales journal").
    pub name: String,
}

/// The input for creating one journal: everything the caller chooses,
/// nothing the system owns (no `company_id` or `id`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JournalDraft {
    /// The journal code (validated by [`Journal::validate_code`]).
    pub code: String,
    /// The kind.
    pub kind: JournalKind,
    /// The human-readable name (validated non-empty, bounded).
    pub name: String,
}

impl Journal {
    /// Validates a journal code: 1..=16 ASCII uppercase letters, digits
    /// or hyphens. Lowercase is rejected on purpose — codes are quoted
    /// in filters and on paper, and one casing is one less thing for an
    /// agent to get wrong.
    ///
    /// # Errors
    /// [`JournalError::InvalidJournalCode`] with the reason when the
    /// code has the wrong length or a character outside `[A-Z0-9-]`.
    pub fn validate_code(code: &str) -> Result<(), JournalError> {
        let len = code.len();
        if !(1..=MAX_CODE_LEN).contains(&len) {
            return Err(JournalError::InvalidJournalCode {
                code: code.to_owned(),
                reason: "journal codes are 1 to 16 characters",
            });
        }
        if !code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(JournalError::InvalidJournalCode {
                code: code.to_owned(),
                reason: "journal codes are uppercase letters, digits and hyphens only",
            });
        }
        Ok(())
    }

    /// Validates a journal name: non-empty and at most
    /// [`MAX_NAME_LEN`] characters.
    ///
    /// # Errors
    /// [`JournalError::InvalidJournalName`] when empty or too long.
    pub fn validate_name(name: &str) -> Result<(), JournalError> {
        if name.is_empty() {
            return Err(JournalError::InvalidJournalName {
                reason: "journal names are not empty",
            });
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(JournalError::InvalidJournalName {
                reason: "journal names are at most 200 characters",
            });
        }
        Ok(())
    }

    /// Validates a draft and freezes it into a [`Journal`] owned by
    /// `company_id` with the caller-supplied ULID `id`.
    ///
    /// # Errors
    /// [`JournalError::InvalidJournalCode`] or
    /// [`JournalError::InvalidJournalName`] from validation.
    pub fn from_draft(
        draft: JournalDraft,
        company_id: &str,
        id: &str,
    ) -> Result<Journal, JournalError> {
        Self::validate_code(&draft.code)?;
        Self::validate_name(&draft.name)?;
        Ok(Journal {
            company_id: company_id.to_owned(),
            id: id.to_owned(),
            code: draft.code,
            kind: draft.kind,
            name: draft.name,
        })
    }
}

/// The journal table name, as declared by [`crate::module::LedgerModule`].
const TABLE: &str = "journal";

/// Creates one journal row: validates the draft, requires the code to
/// be free in the company, inserts. `id` is the caller-supplied ULID
/// (the command layer owns id generation); `created_at` an ISO 8601 UTC
/// timestamp bound from code.
///
/// The insert and its `journal.create` audit event are **one** batch, so
/// a journal never exists without the record that created it; the head
/// is read inside the plan closure, so a chain fork re-plans.
///
/// # Errors
/// [`JournalError::InvalidJournalCode`] /
/// [`JournalError::InvalidJournalName`] from validation;
/// [`JournalError::JournalExists`] when the code is taken in the
/// company; [`JournalError::Database`] otherwise.
pub async fn create_journal(
    db: &dyn Database,
    company_id: &str,
    draft: JournalDraft,
    id: &str,
    created_at: &str,
    audit: &AuditContext,
) -> Result<Journal, JournalError> {
    let journal = Journal::from_draft(draft, company_id, id)?;
    if find_journal(db, company_id, &journal.code).await?.is_some() {
        return Err(JournalError::JournalExists {
            code: journal.code.clone(),
        });
    }
    let insert = insert_statement(&journal, created_at);
    let audited = AuditRecord::from_input(
        "journal.create",
        &serde_json::json!({
            "code": journal.code,
            "kind": journal.kind.to_string(),
            "name": journal.name,
        }),
        Outcome::Allowed,
        vec![journal.id.clone()],
        None,
    );
    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let (insert, audit, audited) = (insert.clone(), audit.clone(), audited.clone());
            let created_at = created_at.to_owned();
            async move {
                let head = chain::read_head(db, company_id).await?;
                let appended = append_statements(
                    company_id,
                    &head,
                    &audit,
                    &audited,
                    &created_at,
                    &AuditConfig::default(),
                )
                .map_err(|err| cratefield_core::DbError::Query(err.to_string()))?;
                let mut statements = vec![insert];
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        |_| {},
    )
    .await
    .map_err(|err| match err {
        // The shared sniffer, not a private copy: retry.rs documents this
        // crate's adoption of it as exactly this refactor.
        ledgers_db::GuardError::Database(db) if ledgers_db::retry::is_unique_violation(&db) => {
            JournalError::JournalExists {
                code: journal.code.clone(),
            }
        }
        other => JournalError::from(other),
    })?;
    Ok(journal)
}

/// The `journal` INSERT for one row, bound from code.
fn insert_statement(journal: &Journal, created_at: &str) -> Statement {
    Statement::with_values(
        format!(
            "INSERT INTO {TABLE} (id, company_id, code, kind, name, created_at)\n\
             VALUES (?, ?, ?, ?, ?, ?)"
        ),
        vec![
            SeaValue::from(journal.id.as_str()),
            SeaValue::from(journal.company_id.as_str()),
            SeaValue::from(journal.code.as_str()),
            SeaValue::from(journal.kind.to_string()),
            SeaValue::from(journal.name.as_str()),
            SeaValue::from(created_at),
        ],
    )
}

/// Finds one journal by company and code; `None` when absent.
///
/// # Errors
/// [`JournalError::Database`] when the query fails or a row cannot be
/// read back.
pub async fn find_journal(
    db: &dyn Database,
    company_id: &str,
    code: &str,
) -> Result<Option<Journal>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT id, company_id, code, kind, name FROM {TABLE}\n\
                 WHERE company_id = ? AND code = ?"
            ),
            vec![SeaValue::from(company_id), SeaValue::from(code)],
        ))
        .await?;
    rows.first().map(row_to_journal).transpose()
}

/// Reads one journal by its id; `None` when absent.
///
/// # Errors
/// [`JournalError::Database`] when the query fails or a row cannot be
/// read back.
pub async fn get_journal(db: &dyn Database, id: &str) -> Result<Option<Journal>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT id, company_id, code, kind, name FROM {TABLE} WHERE id = ?"),
            vec![SeaValue::from(id)],
        ))
        .await?;
    rows.first().map(row_to_journal).transpose()
}

/// Maps one result row to a [`Journal`]. A `kind` this build does not
/// know surfaces as an error rather than a silently defaulted value.
fn row_to_journal(row: &cratefield_core::Row) -> Result<Journal, JournalError> {
    let required = |column: &str| -> Result<String, JournalError> {
        row.get::<String>(column).ok_or_else(|| {
            JournalError::Database(cratefield_core::DbError::Query(format!(
                "column {column} missing or not text"
            )))
        })
    };
    let kind: JournalKind = required("kind")?.parse()?;
    Ok(Journal {
        company_id: required("company_id")?,
        id: required("id")?,
        code: required("code")?,
        kind,
        name: required("name")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_strings() {
        for kind in JournalKind::all() {
            let parsed: JournalKind = kind.to_string().parse().expect("known kind parses");
            assert_eq!(parsed, kind);
        }
        let err = "ledger".parse::<JournalKind>().unwrap_err();
        assert_eq!(
            err,
            JournalError::UnknownKind {
                value: "ledger".to_owned()
            }
        );
    }

    #[test]
    fn codes_bound_the_alphabet() {
        for good in ["G", "SJ", "BJ-1", "0123456789ABCDEF"] {
            assert!(Journal::validate_code(good).is_ok(), "{good} is valid");
        }
        for bad in [
            "",
            "sj",                // lowercase is one casing too many
            "Sales Journal",     // whitespace
            "SJ_1",              // underscore
            "0123456789ABCDEFG", // 17 characters
            "SÉJ",               // not ASCII
        ] {
            assert!(
                Journal::validate_code(bad).is_err(),
                "{bad:?} must be invalid"
            );
        }
    }

    #[test]
    fn names_are_nonempty_and_bounded() {
        assert!(Journal::validate_name("Sales journal").is_ok());
        assert!(Journal::validate_name("").is_err());
        let long = "x".repeat(MAX_NAME_LEN + 1);
        assert!(Journal::validate_name(&long).is_err());
        let exact = "x".repeat(MAX_NAME_LEN);
        assert!(Journal::validate_name(&exact).is_ok());
    }

    #[test]
    fn drafts_freeze_with_owner_and_id() {
        let journal = Journal::from_draft(
            JournalDraft {
                code: "SJ".to_owned(),
                kind: JournalKind::Sales,
                name: "Sales journal".to_owned(),
            },
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "01JOURNALID000000000000000",
        )
        .expect("valid draft");
        assert_eq!(journal.company_id, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(journal.code, "SJ");
        assert!(matches!(
            Journal::from_draft(
                JournalDraft {
                    code: "s j".to_owned(),
                    kind: JournalKind::General,
                    name: "x".to_owned()
                },
                "co",
                "id"
            ),
            Err(JournalError::InvalidJournalCode { .. })
        ));
    }
}
