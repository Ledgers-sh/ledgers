//! The Journal port: the posting surface the period machinery in
//! [`crate::period`] drives — docs/PLAN.md §6, the `ledgers-ledger` row
//! ("journals, entries, ... numbering"). This module defines the port
//! only and holds no table of its own. The production implementation is
//! [`crate::ledger_journal::EntryJournal`], which runs the #67 posting
//! batch ([`crate::posting`]) over the `journal_entry` / `journal_line`
//! tables. [`crate::period`]'s commands consume the port to read
//! balances and post the year-end closing entry, so a fiscal-year close
//! runs against whatever journal the composition supplies.
//!
//! # The posting contract
//!
//! [`Journal::post`] is the one method that writes, and its
//! implementations own the correctness rules that make closed-period
//! rejection hold **directly on the domain service** (docs/PLAN.md §3
//! rule 4):
//!
//! - the **period-postable guard runs inside the posting
//!   `batch_atomic`** — `ledgers_db::period`'s postable assert over the
//!   entry's period, so an `open` period admits everyone, a
//!   `soft-closed` period admits person-driven posts only, and a
//!   `closed` (or missing) period refuses the batch for everyone. An
//!   implementation that checks the period before the batch, in Rust,
//!   has a read-then-write race the guard does not (ADR 0004);
//! - the full reference batch composes the `ledgers-db` guards:
//!   numbering → lines → period-postable → balanced → releases →
//!   daily-limit consume → audit append (`ledgers_db::suite::plan_post`
//!   is the worked shape);
//! - numbering is gapless per `(journal, fiscal year)`, and the audit
//!   append claims the chain head inside the same batch.
//!
//! Amounts on this port are signed minor units, debit-positive, in the
//! company's base currency (converted before the port for
//! foreign-currency entries — FX revaluation is its own later issue,
//! #42). Money is [`ledgers_money::Amount`] end to end; `to_i64` at the
//! storage boundary is the implementer's job.

use crate::error::AccountsError;
use ledgers_money::{Amount, Currency};

/// A boxed [`std::future::Future`] — the return shape that keeps the
/// [`Journal`] trait object-safe over `&dyn Database` borrows.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// One line of an entry to post: an account and a signed amount,
/// debit-positive, in the company base currency.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewEntryLine {
    /// The target account's number in the chart.
    pub account_number: String,
    /// Signed minor units: debit positive, credit negative, base
    /// currency.
    pub amount: Amount,
}

/// An entry to post through [`Journal::post`]. The `entry_id` is the
/// caller's (deterministic ids make crash recovery and idempotent reruns
/// possible — see [`crate::period::close_fiscal_year`]); `period`, when
/// set, overrides the period derived from `entry_date` (the one way an
/// entry can name an adjustment period, which no date reduces to).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewEntry {
    /// The entry's id (a ULID in normal operation; deterministic for
    /// system entries).
    pub entry_id: String,
    /// The journal the entry posts into — also the numbering scope.
    pub journal_id: String,
    /// The fiscal year the numbering counter is scoped to.
    pub fiscal_year: i64,
    /// ISO 8601 calendar date `"YYYY-MM-DD"`.
    pub entry_date: String,
    /// The period to post into; `None` derives it from `entry_date`.
    pub period: Option<String>,
    /// ISO 8601 UTC timestamp, bound from code.
    pub created_at: String,
    /// Free-text memo (single line; data, never instructions).
    pub memo: String,
    /// The lines; must sum to zero in the base currency (the posting
    /// batch's balanced guard is the backstop).
    pub lines: Vec<NewEntryLine>,
    /// The audit subject posting (`"person:{id}"` / `"agent:{key_id}"`
    /// — see [`crate::period::Actor::subject`]).
    pub actor_subject: String,
    /// Whether a person (not an agent key) drives this post: the
    /// period-postable guard admits `soft-closed` periods for persons
    /// only.
    pub human: bool,
    /// The daily post allowance charged to `actor_subject`.
    pub daily_limit: u64,
}

/// An entry as it exists once posted: immutable from here on
/// (docs/PLAN.md §3 rule 3 — corrections are reversals).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PostedEntry {
    /// The entry's id.
    pub entry_id: String,
    /// The gapless number the journal assigned.
    pub number: i64,
    /// ISO 8601 calendar date the entry is dated.
    pub entry_date: String,
}

/// One account's balance: signed minor units, debit-positive, in the
/// company base currency.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AccountBalance {
    /// The account's number in the chart.
    pub account_number: String,
    /// Debit-positive balance in base-currency minor units.
    pub balance: Amount,
}

/// The posting surface: what [`crate::period`]'s close and year-end
/// machinery needs from the journal. Implementations run the full guard
/// batch (see the module docs) — the trait is a port, not a suggestion.
pub trait Journal: Send + Sync {
    /// The company's base currency — the currency every [`Amount`] on
    /// this port is denominated in.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the read fails or the company
    /// has no base currency row to read.
    fn base_currency<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        company_id: &'a str,
    ) -> BoxFuture<'a, Result<Currency, AccountsError>>;

    /// Every account of the company that has ever been posted to, zero
    /// balances included — absence means "never posted", not "zero", so
    /// callers that care filter zeros themselves. Ordered by account
    /// number.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the read fails.
    fn balances<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        company_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AccountBalance>, AccountsError>>;

    /// How many draft (unposted) entries the company holds with dates
    /// in the inclusive range `from..=to` — the number the
    /// draft-entries close check reports. Draft state belongs to the
    /// journal crate; this port only counts.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the read fails.
    fn draft_count<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        company_id: &'a str,
        from: &'a str,
        to: &'a str,
    ) -> BoxFuture<'a, Result<i64, AccountsError>>;

    /// Posts one entry through the full guard batch and returns it as
    /// posted, gapless number included. The period-postable guard MUST
    /// run inside the `batch_atomic` (module docs): that is what makes
    /// a closed period's rejection a property of the domain service
    /// rather than of each caller's discipline.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when a guard fires — a closed
    /// period, an unbalanced entry, a spent daily allowance — or the
    /// engine fails; [`AccountsError::Conflict`] when the retry budget
    /// ran out on races the batch kept losing.
    fn post<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        entry: &'a NewEntry,
    ) -> BoxFuture<'a, Result<PostedEntry, AccountsError>>;

    /// Finds one posted entry by id, or `None` — the crash-recovery
    /// probe the year-end close uses before re-posting its closing
    /// entry.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the read fails.
    fn find_entry<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        company_id: &'a str,
        entry_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<PostedEntry>, AccountsError>>;

    /// Makes sure the journal `journal_id` exists for `company_id`,
    /// creating it under `code` and `name` when absent, and is a no-op
    /// when it already exists. The year-end close's closing journal is
    /// one per fiscal year, so nothing else provisions it; the close
    /// calls this before posting.
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the write or read fails, or
    /// when `code` is already taken by a different journal in the
    /// company.
    fn ensure_journal<'a>(
        &'a self,
        db: &'a dyn cratefield_core::Database,
        company_id: &'a str,
        journal_id: &'a str,
        code: &'a str,
        name: &'a str,
        created_at: &'a str,
    ) -> BoxFuture<'a, Result<(), AccountsError>>;
}
