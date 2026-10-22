//! The journal-entry domain model: lines, dates, the balance invariant
//! and immutability by construction (docs/PLAN.md §3 rules 2, 3, 5).
//!
//! Everything here is plain data plus validation — **no SQL, no
//! posting**. The pipeline has exactly three shapes:
//!
//! 1. [`EntryDraft`] — what a caller composes: dates, description,
//!    source-document links, reversal target and [`Line`]s.
//! 2. [`Entry::from_draft`] — validation (see below). The output
//!    carries `number: None` and `recorded_at: None` — an entry that
//!    has never touched the database.
//! 3. [`crate::posting::post_entry`] — the only thing that turns a
//!    validated entry into a posted one, assigning its number and
//!    `recorded_at` inside the posting batch. There is no fourth
//!    shape: no update, no delete, no unpost (§3 rule 3 — corrections
//!    are reversals, posted as new entries).
//!
//! # The balance invariant (the canonical statement)
//!
//! A line is a [`Side`] (debit or credit) and a **positive** base
//! [`Amount`]; storage signs it (`base_minor` debit `+`, credit `−`).
//! The signed base sum of every line must be zero — always, single-
//! and multi-currency entries alike. When every line's transaction
//! amount is in one currency, that currency's signed sum must be zero
//! too (you cannot pay a USD bill with a USD bank movement that nets
//! to something else, whatever the FX); when the transaction amounts
//! span several currencies — a genuine FX trade — only the base sum is
//! checked, because per-currency legs of a trade do not balance.
//! Violations are [`JournalError::Unbalanced`]: the base difference and
//! the non-zero per-currency differences as `ledgers-money` amounts,
//! so a caller can render them exactly.
//!
//! # Money and dates
//!
//! Money is `ledgers-money` end to end: `Amount` in integer minor
//! units, `Rate` scaled by 10¹², `Rounding::HalfEven` for the FX
//! reconciliation — never a float anywhere (AGENTS.md; the crate denies
//! `clippy::float_arithmetic`). Dates are `time::Date` in the domain
//! and ISO 8601 `"YYYY-MM-DD"` strings only at the DB boundary.
//! The domain types deliberately carry no serde (core's `time` has no
//! serde feature); M1's command layer maps into its own wire types.

use cratefield_core::{Database, Statement};
use ledgers_money::{Amount, Currency, MoneyError, Rate, Rounding};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use time::Date;

/// The maximum number of lines one entry may carry.
pub const MAX_LINES: usize = 2000;

/// Which side of the entry a line posts to. The amount itself is always
/// positive; the side is the sign.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Side {
    /// Debit: assets and expenses up, the left column.
    Debit,
    /// Credit: liabilities, equity and income up, the right column.
    Credit,
}

impl Side {
    /// The storage sign: debit `+1`, credit `−1` — the multiplier that
    /// turns `(side, positive amount)` into a signed minor unit.
    #[must_use]
    pub fn sign(self) -> i64 {
        match self {
            Side::Debit => 1,
            Side::Credit => -1,
        }
    }

    /// The opposite side.
    #[must_use]
    pub const fn flip(self) -> Self {
        match self {
            Side::Debit => Side::Credit,
            Side::Credit => Side::Debit,
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Side::Debit => "debit",
            Side::Credit => "credit",
        })
    }
}

/// The foreign-exchange data one line carries when its transaction
/// currency is not the company's base currency: what the document says
/// (the transaction amount) and the rate it converts at.
///
/// The pair must reconcile exactly under half-even rounding:
/// `transaction.mul_rate(rate, Rounding::HalfEven) == base`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Fx {
    /// The amount in the transaction currency, as the document states
    /// it (positive; the line's side supplies the sign).
    pub transaction: Amount,
    /// The conversion rate to the base currency.
    pub rate: Rate,
}

/// One line of one entry: an account, a side, a positive amount in the
/// company's base currency, and everything optional a line can carry.
///
/// `base` is denominated in the entry's base currency (checked by
/// [`Entry::from_draft`]); when the transaction currency differs, `fx`
/// carries the document-side amount and rate, and validation reconciles
/// the two exactly.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    /// The account's number in the company's chart.
    pub account_number: String,
    /// Debit or credit.
    pub side: Side,
    /// The positive amount in the base currency.
    pub base: Amount,
    /// Foreign-exchange data; `None` when the line is a pure base
    /// currency line.
    pub fx: Option<Fx>,
    /// The tax code the line posts under (`"19-U"`, …), when the entry
    /// is a tax document line.
    pub tax_code: Option<String>,
    /// Free-form dimensions for reporting cuts (cost centre, project),
    /// a string map so any scheme fits without a schema change.
    pub dimensions: BTreeMap<String, String>,
    /// The human note on the line; data, never instructions (§3 rule 8).
    pub memo: Option<String>,
}

/// The caller-composed entry, before validation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntryDraft {
    /// The journal (identified by id) the entry posts into; the journal
    /// owns the number sequence.
    pub journal_id: String,
    /// The document date — when the business event happened (invoice
    /// date, statement date).
    pub document_date: Date,
    /// The posting date — when the entry enters the books; names the
    /// period the period-open guard checks.
    pub posting_date: Date,
    /// The tax point date — when VAT becomes chargeable; `None` means
    /// the posting date is the tax point.
    pub tax_point_date: Option<Date>,
    /// The base currency every line's `base` amount is denominated in.
    pub base_currency: Currency,
    /// The human description.
    pub description: String,
    /// Links to source documents (`"invoice:INV-2026-042"`), data not
    /// instructions.
    pub source_documents: Vec<String>,
    /// External ID from the source application (for idempotency).
    pub external_id: Option<String>,
    /// The application that posted this entry.
    pub source_app: Option<String>,
    /// When set, this entry reverses that one (a posted entry id);
    /// corrections are reversals (§3 rule 3).
    pub reverses: Option<String>,
    /// The lines; validated to [`MAX_LINES`], at least two, none zero.
    pub lines: Vec<Line>,
}

/// A validated entry: balanced, FX-reconciled, VAT-period derived — and
/// not yet posted. [`crate::posting::post_entry`] is the only
/// constructor of the posted state (number and `record_at` set), and
/// nothing anywhere constructs an update to one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// The company whose books the entry enters.
    pub company_id: String,
    /// The journal the entry posts into (its number sequence).
    pub journal_id: String,
    /// The document date (see [`EntryDraft::document_date`]).
    pub document_date: Date,
    /// The posting date (see [`EntryDraft::posting_date`]).
    pub posting_date: Date,
    /// The tax point date; the posting date when the draft carried none.
    pub tax_point_date: Date,
    /// The base currency of every line's base amount.
    pub base_currency: Currency,
    /// `"YYYY-MM"` — the VAT period, derived from the tax point date.
    pub vat_period: String,
    /// The human description.
    pub description: String,
    /// Source-document links.
    pub source_documents: Vec<String>,
    /// External ID from the source application (for idempotency).
    pub external_id: Option<String>,
    /// The application that posted this entry.
    pub source_app: Option<String>,
    /// The entry this one reverses, when it is a reversal.
    pub reverses: Option<String>,
    /// The lines, in order, validated.
    pub lines: Vec<Line>,
    /// `None` until [`crate::posting::post_entry`] assigns it inside
    /// the posting batch; a posted entry's number is immutable.
    pub number: Option<i64>,
    /// `None` until posting stamps it; a posted entry's `recorded_at`
    /// is immutable.
    pub recorded_at: Option<String>,
}

impl Entry {
    /// Validates a draft into an [`Entry`] owned by `company_id`.
    /// Every line's base amount is in the draft's `base_currency` and
    /// strictly positive (the FX transaction amount too), every FX pair
    /// reconciles exactly, there are `2..=MAX_LINES` lines, none zero,
    /// and the entry balances (see the module docs for the canonical
    /// statement of the balance invariant). `number` and `recorded_at`
    /// come back `None` — only posting sets them.
    ///
    /// # Errors
    /// [`JournalError::TooFewLines`], [`JournalError::TooManyLines`],
    /// [`JournalError::ZeroLine`], [`JournalError::NegativeAmount`],
    /// [`JournalError::Money`] (a base amount in the wrong currency, or
    /// overflow while summing), [`JournalError::FxMismatch`] when a
    /// line's `transaction × rate` does not equal its base amount,
    /// [`JournalError::Unbalanced`] when the sums are not zero.
    pub fn from_draft(draft: EntryDraft, company_id: &str) -> Result<Self, JournalError> {
        let base_currency = draft.base_currency;
        let lines_count = draft.lines.len();
        if lines_count < 2 {
            return Err(JournalError::TooFewLines { lines: lines_count });
        }
        if lines_count > MAX_LINES {
            return Err(JournalError::TooManyLines { lines: lines_count });
        }
        for line in &draft.lines {
            if line.base.is_zero() {
                return Err(JournalError::ZeroLine {
                    account: line.account_number.clone(),
                });
            }
            if line.base.minor().is_negative()
                || line
                    .fx
                    .as_ref()
                    .is_some_and(|fx| fx.transaction.minor().is_negative())
            {
                return Err(JournalError::NegativeAmount {
                    account: line.account_number.clone(),
                });
            }
            if line.base.currency() != base_currency {
                return Err(JournalError::Money(MoneyError::CurrencyMismatch {
                    expected: base_currency,
                    actual: line.base.currency(),
                }));
            }
            if let Some(fx) = &line.fx
                && fx.transaction.currency() == base_currency
            {
                return Err(JournalError::FxSameCurrency {
                    currency: base_currency.code().to_owned(),
                });
            }
            if let Some(fx) = &line.fx {
                // mul_rate keeps the transaction's currency tag — the
                // value it returns is the base-currency equivalent, so
                // re-tag it before comparing with the declared base.
                let converted = fx
                    .transaction
                    .mul_rate(fx.rate, Rounding::HalfEven)
                    .map_err(JournalError::Money)?;
                let expected = Amount::new(converted.minor(), base_currency);
                if expected != line.base {
                    return Err(JournalError::FxMismatch {
                        account: line.account_number.clone(),
                        transaction: fx.transaction.to_canonical_string(),
                        rate: fx.rate.to_string(),
                        base: line.base.to_canonical_string(),
                    });
                }
            }
        }
        let balance = Balance::of(&draft.lines, base_currency)?;
        if !balance.is_zero() {
            return Err(balance.unbalanced());
        }
        let tax_point_date = draft.tax_point_date.unwrap_or(draft.posting_date);
        Ok(Entry {
            company_id: company_id.to_owned(),
            journal_id: draft.journal_id,
            vat_period: vat_period_of(tax_point_date),
            document_date: draft.document_date,
            posting_date: draft.posting_date,
            tax_point_date,
            base_currency,
            description: draft.description,
            source_documents: draft.source_documents,
            external_id: draft.external_id,
            source_app: draft.source_app,
            reverses: draft.reverses,
            lines: draft.lines,
            number: None,
            recorded_at: None,
        })
    }

    /// The entry's posting period, `"YYYY-MM"` of the posting date —
    /// what the period-open guard checks. (The VAT period is
    /// [`Entry::vat_period`], derived from the tax point date
    /// instead.)
    #[must_use]
    pub fn posting_period(&self) -> String {
        vat_period_of(self.posting_date)
    }
}

/// The VAT period of a tax point date: `"YYYY-MM"`, via the shared
/// `ledgers-db` reduction (same shape, one implementation everywhere).
///
/// # Panics
/// Never: a `time::Date` always serialises as `"YYYY-MM-DD"`, which is
/// exactly the shape the reduction accepts.
#[must_use]
pub fn vat_period_of(tax_point: Date) -> String {
    ledgers_db::period::period_of_date(&iso_date_string(tax_point))
        .expect("a time::Date is always YYYY-MM-DD")
}

/// The DB-boundary spelling of a date: ISO `"YYYY-MM-DD"`, bound from
/// code — dates live as `time::Date` in the domain and as text only at
/// the storage edge. Built from the components (a `time::Date` is a
/// calendar date by construction, so the spelling is exact).
pub(crate) fn iso_date_string(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// The signed sums of an entry's lines (see the module docs for the
/// invariant's canonical statement): the base total always, the single
/// transaction currency's total when one applies.
struct Balance {
    base_difference: Amount,
    /// `Some((currency, difference))` when the single-currency leg
    /// applies — every line's transaction amount (fx or not) in one
    /// currency, that currency not the base.
    currency_difference: Option<(Currency, Amount)>,
}

impl Balance {
    /// Sums the lines: base always; the single transaction currency
    /// when there is exactly one and it is not the base (when it IS
    /// the base the leg is the base sum, already checked).
    fn of(lines: &[Line], base: Currency) -> Result<Self, JournalError> {
        let mut base_difference = Amount::new(0, base);
        let mut currencies: BTreeMap<Currency, Amount> = BTreeMap::new();
        for line in lines {
            let signed = signed_minor(line)?;
            base_difference = base_difference
                .checked_add(Amount::new(i128::from(signed), base))
                .map_err(JournalError::Money)?;
            // The transaction leg is the FX pair's own amount — the
            // base leg's minor units are NOT it, that is the whole
            // point of the second sum — or the base amount itself when
            // the line has no FX.
            let transaction = match &line.fx {
                Some(fx) => Amount::new(
                    i128::from(line.side.sign()) * fx.transaction.minor(),
                    fx.transaction.currency(),
                ),
                None => Amount::new(i128::from(signed), base),
            };
            let currency = transaction.currency();
            let entry_sum = currencies
                .remove(&currency)
                .unwrap_or(Amount::new(0, currency));
            currencies.insert(
                currency,
                entry_sum
                    .checked_add(transaction)
                    .map_err(JournalError::Money)?,
            );
        }
        let currency_difference = match currencies.len() {
            1 => {
                let (currency, sum) = currencies
                    .first_key_value()
                    .expect("len checked as exactly one");
                (*currency != base).then_some((*currency, *sum))
            }
            _ => None,
        };
        Ok(Balance {
            base_difference,
            currency_difference,
        })
    }

    fn is_zero(&self) -> bool {
        self.base_difference.is_zero()
            && self
                .currency_difference
                .is_none_or(|(_, difference)| difference.is_zero())
    }

    fn unbalanced(self) -> JournalError {
        JournalError::Unbalanced {
            base_difference: self.base_difference,
            // Only currencies whose legs are actually off: a balanced
            // transaction leg is not part of the problem an agent
            // needs to fix.
            currency_differences: self
                .currency_difference
                .filter(|(_, difference)| !difference.is_zero())
                .map(|(currency, difference)| vec![(currency, difference)])
                .unwrap_or_default(),
        }
    }
}

/// The signed base minor units of a line: `(side, positive amount)` as
/// debit-positive storage units, narrowed to `i64` — the storage
/// boundary's width, checked rather than clamped.
fn signed_minor(line: &Line) -> Result<i64, JournalError> {
    let signed = i128::from(line.side.sign()) * line.base.minor();
    i64::try_from(signed).map_err(|_| JournalError::Money(MoneyError::Overflow))
}

/// Every way a journal or entry operation can fail.
///
/// The variants an agent can act on carry structured data (the
/// differences of an [`JournalError::Unbalanced`], the offending
/// account of an [`JournalError::FxMismatch`]); everything database
/// shaped is a transparent [`JournalError::Database`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The entry's lines do not sum to zero (the invariant's canonical
    /// statement is the module docs). `base_difference` is the signed
    /// base-currency sum (positive = debit-heavy);
    /// `currency_differences` carries the single transaction
    /// currency's non-zero difference, if that leg applies. A
    /// multi-currency entry's list is empty by design: it balances in
    /// base only.
    #[error("entry does not balance: base difference {base_difference}; {}", currency_differences.iter().map(|(c, d)| format!("{d} {c}")).collect::<Vec<_>>().join(", "))]
    Unbalanced {
        /// The signed base-currency sum; zero is balanced.
        base_difference: Amount,
        /// `(currency, signed difference)` for the single-currency leg,
        /// when it applies; empty for base-only and multi-currency
        /// entries.
        currency_differences: Vec<(Currency, Amount)>,
    },

    /// Fewer than two lines — an entry moves value between at least
    /// two accounts.
    #[error("an entry needs at least two lines, got {lines}")]
    TooFewLines {
        /// The line count the draft carried.
        lines: usize,
    },

    /// More than [`MAX_LINES`] lines.
    #[error("an entry carries at most {MAX_LINES} lines, got {lines}")]
    TooManyLines {
        /// The line count the draft carried.
        lines: usize,
    },

    /// A line whose amount is zero — it posts nothing and only pads
    /// the entry.
    #[error("line on account {account} has a zero amount")]
    ZeroLine {
        /// The account the zero line named.
        account: String,
    },

    /// A line with a negative base or FX transaction amount — the
    /// amount is magnitude, the side is the sign; a negative amount is
    /// a side encoded twice.
    #[error("line on account {account} has a negative amount (the side carries the sign)")]
    NegativeAmount {
        /// The account the negative line named.
        account: String,
    },

    /// A line's FX pair does not reconcile:
    /// `transaction × rate ≠ base` under half-even rounding.
    #[error("fx mismatch on account {account}: {transaction} × {rate} ≠ {base}")]
    FxMismatch {
        /// The account the line named.
        account: String,
        /// The transaction amount, canonical string.
        transaction: String,
        /// The rate, canonical string.
        rate: String,
        /// The base amount the line declared, canonical string.
        base: String,
    },

    /// A line carries FX data whose transaction currency is the base
    /// currency — FX is for foreign amounts only.
    #[error("line carries fx in the base currency {currency}")]
    FxSameCurrency {
        /// The base currency code.
        currency: String,
    },

    /// A malformed journal code (see [`Journal::validate_code`]).
    #[error("invalid journal code {code:?}: {reason}")]
    InvalidJournalCode {
        /// The code as given.
        code: String,
        /// What was wrong with it.
        reason: &'static str,
    },

    /// A malformed journal name.
    #[error("invalid journal name: {reason}")]
    InvalidJournalName {
        /// What was wrong with it.
        reason: &'static str,
    },

    /// A journal kind string this build does not know.
    #[error("unknown journal kind: {value:?}")]
    UnknownKind {
        /// The offending value.
        value: String,
    },

    /// The journal code is already taken in the company.
    #[error("journal already exists: {code}")]
    JournalExists {
        /// The conflicting code.
        code: String,
    },

    /// No journal with this id exists in the company.
    #[error("journal not found: {id}")]
    JournalNotFound {
        /// The id that was looked up.
        id: String,
    },

    /// The entry names an account that does not exist in the company —
    /// the write-time guard's refusal mapped back to the domain.
    #[error("account {account} not found in the chart (or archived)")]
    UnknownAccount {
        /// The account number the guard rejected.
        account: String,
    },

    /// The entry's posting period is closed (or unknown — the guard
    /// fails closed), mapped back from the write-time refusal.
    #[error("posting period {period} is not open")]
    PeriodNotOpen {
        /// The `"YYYY-MM"` period that refused.
        period: String,
    },

    /// The journal the entry posts into does not belong to the posting
    /// company (or does not exist) — the ownership guard's refusal
    /// mapped back: one company can never consume another's number
    /// sequence.
    #[error("journal {journal} is not a journal of this company")]
    ForeignJournal {
        /// The journal id the guard rejected.
        journal: String,
    },

    /// The entry this one reverses does not belong to the posting
    /// company (or does not exist).
    #[error("reversal target {entry} is not an entry of this company")]
    ForeignReversal {
        /// The reversal target the guard rejected.
        entry: String,
    },

    /// The entry to be reversed is itself a reversal, which is forbidden.
    #[error("cannot reverse a reversal (entry {entry} is a reversal itself)")]
    ReversalOfReversal {
        /// The reversal entry being reversed.
        entry: String,
    },

    /// A reversal target has already been reversed by another entry.
    #[error("entry {entry} has already been reversed")]
    AlreadyReversed {
        /// The entry that was already reversed.
        entry: String,
    },

    /// An agent attempted to reverse a human-posted entry, which is human-only.
    #[error("reversing a human-posted entry is human-only; an agent key cannot perform it")]
    HumanOnlyReversal,

    /// The entry's base currency does not match the company's base currency.
    #[error(
        "entry base currency {entry_currency} does not match company base currency {company_currency}"
    )]
    BaseCurrencyMismatch {
        /// The company's base currency code.
        company_currency: String,
        /// The entry's base currency code.
        entry_currency: String,
    },

    /// The entry id was already used to post an entry to a different
    /// journal. Entry ids are idempotency keys: a resubmit with the same
    /// id returns the original entry (if it posted) or lets it post again
    /// (if the first attempt failed). Reusing an id across different
    /// journals is not a resubmit but a misuse of the key.
    #[error("entry id {entry_id} already posted to a different journal")]
    EntryIdAlreadyUsed {
        /// The entry id that was already posted.
        entry_id: String,
    },

    /// The batch still conflicted after every retry attempt —
    /// contention, not refusal.
    #[error("posting still conflicts after {attempts} attempts")]
    Conflict {
        /// The attempts the policy allowed.
        attempts: u32,
    },

    /// A money operation failed (wrong currency on a line, overflow
    /// while summing).
    #[error(transparent)]
    Money(#[from] MoneyError),

    /// A line on an account that requires a dimension does not carry it.
    #[error("account {account} requires dimension {dimension} on all lines")]
    DimensionRequired {
        /// The account that enforces the requirement.
        account: String,
        /// The dimension kind.
        dimension: String,
    },

    /// A line on an account that forbids a dimension carries it.
    #[error("account {account} forbids dimension {dimension}")]
    DimensionForbidden {
        /// The account that forbids the dimension.
        account: String,
        /// The dimension kind.
        dimension: String,
    },

    /// A line carries a dimension value that does not exist in the company.
    #[error("unknown {dimension} value on account {account}: {value}")]
    UnknownDimensionValue {
        /// The dimension kind.
        dimension: String,
        /// The account the line names.
        account: String,
        /// The unknown value.
        value: String,
    },

    /// An own-books unit's lines in this entry do not balance in base.
    #[error("unit {unit} does not balance in base currency")]
    UnitUnbalanced {
        /// The unit code whose lines don't balance.
        unit: String,
    },

    /// Per-unit balancing is enabled but no inter-unit clearing account
    /// exists or is ambiguous (need exactly one unarchived leaf tagged
    /// `inter-unit-clearing`).
    #[error("per-unit balancing requires exactly one unarchived inter-unit-clearing account")]
    ClearingAccountMissing,

    /// A statement failed on the `Database` port.
    #[error(transparent)]
    Database(#[from] cratefield_core::DbError),
}

impl From<ledgers_db::GuardError> for JournalError {
    /// A guard batch's failure splits on its two arms: a spent retry
    /// budget is [`JournalError::Conflict`], a refusal or engine
    /// failure is [`JournalError::Database`]. There is deliberately no
    /// `Guard` variant to catch both — callers (and `to_problem`)
    /// branch on the split, never on the raw guard error.
    fn from(err: ledgers_db::GuardError) -> Self {
        match err {
            ledgers_db::GuardError::Conflict { attempts } => JournalError::Conflict { attempts },
            ledgers_db::GuardError::Database(db) => JournalError::Database(db),
        }
    }
}

/// `journal.entry.posted` problem: the entry's lines do not balance.
pub const UNBALANCED_ENTRY: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "unbalanced-entry",
    status: cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Entry does not balance",
    description: "The entry's lines do not sum to zero in the base currency (nor, for a \
                  single-currency entry, in that currency).",
};

/// `journal.post.refused` problem: the posting batch was refused by a
/// write-time guard (unknown account, closed period) or malformed input.
pub const JOURNAL_ENTRY_REFUSED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "journal-entry-refused",
    status: cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Journal entry refused",
    description: "The entry was refused before posting: unknown or archived account, closed \
                  posting period, or malformed entry data.",
};

/// `journal.not-found` problem.
pub const JOURNAL_NOT_FOUND: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "journal-not-found",
    status: cratefield_core::axum::http::StatusCode::NOT_FOUND,
    title: "Journal not found",
    description: "No journal with this id exists for the company.",
};

/// `journal.exists` problem: the code is taken.
pub const JOURNAL_EXISTS: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "journal-exists",
    status: cratefield_core::axum::http::StatusCode::CONFLICT,
    title: "Journal already exists",
    description: "A journal with this code already exists for the company.",
};

/// `journal.post.conflict` problem: the retry budget ran out on races.
pub const JOURNAL_POST_CONFLICT: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "journal-post-conflict",
    status: cratefield_core::axum::http::StatusCode::SERVICE_UNAVAILABLE,
    title: "Posting conflicted",
    description: "The posting batch kept losing a concurrency race; retry the post later.",
};

impl JournalError {
    /// The RFC 9457 problem this error renders as: structured
    /// differences ride along as extension members so an agent can act
    /// on them without scraping prose.
    ///
    /// # Panics
    /// Never on this dependency graph; `serde_json::Value` construction
    /// from strings and arrays is infallible here.
    #[must_use]
    pub fn to_problem(&self) -> cratefield_core::Problem {
        use cratefield_core::Problem;
        match self {
            JournalError::Unbalanced {
                base_difference,
                currency_differences,
            } => Problem::new(&UNBALANCED_ENTRY)
                .with_detail("debits must equal credits")
                .with_extension("baseCurrency", base_difference.currency().code().to_owned())
                .with_extension("baseDifference", base_difference.to_canonical_string())
                .with_extension(
                    "currencyDifferences",
                    serde_json::Value::Array(
                        currency_differences
                            .iter()
                            .map(|(currency, difference)| {
                                serde_json::json!({
                                    "currency": currency.code(),
                                    "difference": difference.to_canonical_string(),
                                })
                            })
                            .collect(),
                    ),
                ),
            JournalError::TooFewLines { lines } | JournalError::TooManyLines { lines } => {
                Problem::new(&JOURNAL_ENTRY_REFUSED)
                    .with_detail(self.to_string())
                    .with_extension("lines", *lines as u64)
            }
            JournalError::ZeroLine { account }
            | JournalError::FxMismatch { account, .. }
            | JournalError::NegativeAmount { account }
            | JournalError::UnknownAccount { account } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("account", account.clone()),
            JournalError::FxSameCurrency { currency } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("currency", currency.clone()),
            JournalError::BaseCurrencyMismatch {
                company_currency,
                entry_currency,
            } => base_currency_mismatch_problem(self, company_currency, entry_currency),
            JournalError::InvalidJournalCode { code, .. } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("code", code.clone()),
            JournalError::InvalidJournalName { .. }
            | JournalError::HumanOnlyReversal
            | JournalError::ClearingAccountMissing => {
                Problem::new(&JOURNAL_ENTRY_REFUSED).with_detail(self.to_string())
            }
            JournalError::UnknownKind { value } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("kind", value.clone()),
            JournalError::JournalExists { code } => Problem::new(&JOURNAL_EXISTS)
                .with_detail(self.to_string())
                .with_extension("code", code.clone()),
            JournalError::JournalNotFound { id } => Problem::new(&JOURNAL_NOT_FOUND)
                .with_detail(self.to_string())
                .with_extension("id", id.clone()),
            JournalError::PeriodNotOpen { period } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("period", period.clone()),
            JournalError::ForeignJournal { journal } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("journal", journal.clone()),
            JournalError::ForeignReversal { entry }
            | JournalError::ReversalOfReversal { entry }
            | JournalError::AlreadyReversed { entry } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("entry", entry.clone()),
            JournalError::EntryIdAlreadyUsed { entry_id } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("entryId", entry_id.clone()),
            JournalError::Conflict { attempts } => Problem::new(&JOURNAL_POST_CONFLICT)
                .with_detail(self.to_string())
                .with_extension("attempts", u64::from(*attempts)),
            JournalError::DimensionRequired { account, dimension }
            | JournalError::DimensionForbidden { account, dimension } => {
                Problem::new(&JOURNAL_ENTRY_REFUSED)
                    .with_detail(self.to_string())
                    .with_extension("account", account.clone())
                    .with_extension("dimension", dimension.clone())
            }
            JournalError::UnknownDimensionValue {
                dimension,
                account,
                value,
            } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("dimension", dimension.clone())
                .with_extension("account", account.clone())
                .with_extension("value", value.clone()),
            JournalError::UnitUnbalanced { unit } => Problem::new(&JOURNAL_ENTRY_REFUSED)
                .with_detail(self.to_string())
                .with_extension("unit", unit.clone()),
            JournalError::Money(_) | JournalError::Database(_) => {
                // No internals in the body (architecture section 6):
                // the underlying error is the caller's to log.
                Problem::new(&cratefield_core::SLUGS.internal)
            }
        }
    }
}

/// Reads back the ids of the company's entries that reverse
/// `entry_id` — the `reverses` relation, exposed as a query rather
/// than a mutable `reversed_by` column, so no UPDATE path to a posted
/// entry exists anywhere (docs/PLAN.md §3 rule 3). Scoped to the
/// company like every read: one tenant can never see another's
/// reversals.
///
/// # Errors
/// [`JournalError::Database`] when the query fails.
pub async fn reversed_by(
    db: &dyn Database,
    company_id: &str,
    entry_id: &str,
) -> Result<Vec<String>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id FROM journal_entry\n\
             WHERE company_id = ? AND reverses = ? ORDER BY id"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(entry_id)],
        ))
        .await?;
    Ok(rows
        .rows
        .iter()
        .map(|row| row.get::<String>("id"))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default())
}

/// The refusal for an entry whose base currency is not the company's.
fn base_currency_mismatch_problem(
    error: &JournalError,
    company_currency: &str,
    entry_currency: &str,
) -> cratefield_core::Problem {
    cratefield_core::Problem::new(&JOURNAL_ENTRY_REFUSED)
        .with_detail(error.to_string())
        .with_extension("companyCurrency", company_currency.to_owned())
        .with_extension("entryCurrency", entry_currency.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cratefield_core::axum::response::IntoResponse;

    fn eur() -> Currency {
        Currency::from_code("EUR").expect("EUR is in the table")
    }
    fn usd() -> Currency {
        Currency::from_code("USD").expect("USD is in the table")
    }
    fn d(year: i32, month: time::Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).expect("a calendar date")
    }
    fn eur_minor(minor: i128) -> Amount {
        Amount::new(minor, eur())
    }
    fn usd_minor(minor: i128) -> Amount {
        Amount::new(minor, usd())
    }

    fn line(account: &str, side: Side, base: Amount) -> Line {
        Line {
            account_number: account.to_owned(),
            side,
            base,
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        }
    }

    fn draft(lines: Vec<Line>) -> EntryDraft {
        EntryDraft {
            journal_id: "01JOURNAL0000000000000000000".to_owned(),
            document_date: d(2026, time::Month::April, 10),
            posting_date: d(2026, time::Month::April, 10),
            tax_point_date: None,
            base_currency: eur(),
            description: "Opening test entry".to_owned(),
            source_documents: Vec::new(),
            external_id: None,
            source_app: None,
            reverses: None,
            lines,
        }
    }

    #[test]
    fn sides_sign_and_flip() {
        assert_eq!(Side::Debit.sign(), 1);
        assert_eq!(Side::Credit.sign(), -1);
        assert_eq!(Side::Debit.flip(), Side::Credit);
        assert_eq!(Side::Debit.to_string(), "debit");
        assert_eq!(Side::Credit.to_string(), "credit");
    }

    #[test]
    fn a_balanced_entry_validates() {
        let entry = Entry::from_draft(
            draft(vec![
                line("01000", Side::Debit, eur_minor(100)),
                line("04000", Side::Credit, eur_minor(100)),
            ]),
            "co",
        )
        .expect("balanced two-liner validates");
        assert_eq!(entry.number, None);
        assert_eq!(entry.recorded_at, None);
        assert_eq!(entry.vat_period, "2026-04");
        assert_eq!(entry.posting_period(), "2026-04");
        // The tax point defaults to the posting date.
        assert_eq!(entry.tax_point_date, entry.posting_date);
    }

    #[test]
    fn vat_period_comes_from_the_tax_point_date() {
        let mut input = draft(vec![
            line("01000", Side::Debit, eur_minor(1)),
            line("04000", Side::Credit, eur_minor(1)),
        ]);
        input.posting_date = d(2026, time::Month::April, 30);
        input.tax_point_date = Some(d(2026, time::Month::May, 2));
        let entry = Entry::from_draft(input, "co").expect("valid");
        assert_eq!(entry.vat_period, "2026-05");
        assert_eq!(entry.posting_period(), "2026-04");
    }

    #[test]
    fn one_line_or_zero_lines_is_refused() {
        assert_eq!(
            Entry::from_draft(draft(Vec::new()), "co").unwrap_err(),
            JournalError::TooFewLines { lines: 0 }
        );
        assert_eq!(
            Entry::from_draft(draft(vec![line("01000", Side::Debit, eur_minor(5))]), "co")
                .unwrap_err(),
            JournalError::TooFewLines { lines: 1 }
        );
        let many_debits: Vec<Line> = (0..=MAX_LINES)
            .map(|n| line(&format!("0{n}"), Side::Debit, eur_minor(1)))
            .collect();
        assert_eq!(
            Entry::from_draft(draft(many_debits), "co").unwrap_err(),
            JournalError::TooManyLines {
                lines: MAX_LINES + 1
            }
        );
    }

    #[test]
    fn a_zero_line_is_refused() {
        let err = Entry::from_draft(
            draft(vec![
                line("01000", Side::Debit, eur_minor(0)),
                line("04000", Side::Credit, eur_minor(0)),
            ]),
            "co",
        )
        .unwrap_err();
        assert_eq!(
            err,
            JournalError::ZeroLine {
                account: "01000".to_owned()
            }
        );
    }

    #[test]
    fn negative_amounts_are_refused_the_side_carries_the_sign() {
        // A negative base amount.
        let err = Entry::from_draft(
            draft(vec![
                line("01000", Side::Debit, eur_minor(-100)),
                line("04000", Side::Credit, eur_minor(100)),
            ]),
            "co",
        )
        .unwrap_err();
        assert_eq!(
            err,
            JournalError::NegativeAmount {
                account: "01000".to_owned()
            }
        );
        // A negative FX transaction amount, base amount positive.
        let mut debit = line("01000", Side::Debit, eur_minor(90));
        debit.fx = Some(Fx {
            transaction: usd_minor(-100),
            rate: Rate::parse("0.9").expect("a rate"),
        });
        let err = Entry::from_draft(
            draft(vec![debit, line("04000", Side::Credit, eur_minor(90))]),
            "co",
        )
        .unwrap_err();
        assert_eq!(
            err,
            JournalError::NegativeAmount {
                account: "01000".to_owned()
            }
        );
    }

    #[test]
    fn a_base_imbalance_is_refused_with_the_difference() {
        let err = Entry::from_draft(
            draft(vec![
                line("01000", Side::Debit, eur_minor(100)),
                line("04000", Side::Credit, eur_minor(80)),
            ]),
            "co",
        )
        .unwrap_err();
        // Debit-heavy: +20.00 EUR.
        assert_eq!(
            err,
            JournalError::Unbalanced {
                base_difference: eur_minor(20),
                currency_differences: Vec::new(),
            }
        );
    }

    #[test]
    fn a_single_currency_transaction_imbalance_is_refused_with_both_legs() {
        // USD 10.00 debit and USD 9.00 credit, both converted to EUR at
        // rates that make the base side balance exactly: the base sums
        // to zero, the USD leg does not.
        let mut debit = line("01000", Side::Debit, eur_minor(900));
        debit.fx = Some(Fx {
            transaction: usd_minor(1000),
            rate: Rate::parse("0.9").expect("a rate"),
        });
        let mut credit = line("04000", Side::Credit, eur_minor(900));
        // A different (transaction, rate) pair that multiplies to the
        // same base: USD 9.00 at rate 1 — so the base side balances
        // exactly while the USD leg is 10.00 − 9.00 short.
        credit.fx = Some(Fx {
            transaction: usd_minor(900),
            rate: Rate::ONE,
        });
        let err = Entry::from_draft(draft(vec![debit, credit]), "co").unwrap_err();
        assert_eq!(
            err,
            JournalError::Unbalanced {
                base_difference: eur_minor(0),
                currency_differences: vec![(usd(), usd_minor(100))],
            }
        );
    }

    #[test]
    fn a_mixed_currency_entry_balances_in_base_only() {
        // Dr USD 100.00 (fx to EUR 90.00), Cr EUR 90.00: two
        // transaction currencies, one balanced base sum.
        let mut debit = line("01000", Side::Debit, eur_minor(9000));
        debit.fx = Some(Fx {
            transaction: usd_minor(10_000),
            rate: Rate::parse("0.9").expect("a rate"),
        });
        let credit = line("04000", Side::Credit, eur_minor(9000));
        let entry = Entry::from_draft(draft(vec![debit, credit]), "co")
            .expect("mixed transaction currencies balance in base");
        assert_eq!(entry.vat_period, "2026-04");
    }

    #[test]
    fn fx_must_reconcile_exactly() {
        let mut debit = line("01000", Side::Debit, eur_minor(9001));
        debit.fx = Some(Fx {
            transaction: usd_minor(10_000),
            rate: Rate::parse("0.9").expect("a rate"),
        });
        let credit = line("04000", Side::Credit, eur_minor(9001));
        let err = Entry::from_draft(draft(vec![debit, credit]), "co").unwrap_err();
        assert!(matches!(err, JournalError::FxMismatch { .. }));
    }

    #[test]
    fn fx_in_the_base_currency_is_refused() {
        let mut debit = line("01000", Side::Debit, eur_minor(100));
        debit.fx = Some(Fx {
            transaction: eur_minor(100),
            rate: Rate::ONE,
        });
        let credit = line("04000", Side::Credit, eur_minor(100));
        let err = Entry::from_draft(draft(vec![debit, credit]), "co").unwrap_err();
        assert_eq!(
            err,
            JournalError::FxSameCurrency {
                currency: "EUR".to_owned()
            }
        );
    }

    #[test]
    fn a_wrong_currency_base_amount_is_refused() {
        let err = Entry::from_draft(
            draft(vec![
                line("01000", Side::Debit, usd_minor(100)),
                line("04000", Side::Credit, eur_minor(100)),
            ]),
            "co",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            JournalError::Money(MoneyError::CurrencyMismatch { .. })
        ));
    }

    #[test]
    fn the_unbalanced_problem_carries_structured_differences() {
        let err = JournalError::Unbalanced {
            base_difference: eur_minor(-2000),
            currency_differences: vec![(usd(), usd_minor(150))],
        };
        let problem = err.to_problem();
        let response = problem.into_response();
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .expect("a content type")
            .to_owned();
        let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
            response.into_body(),
            usize::MAX,
        ))
        .expect("a body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("problem+json");
        assert_eq!(
            status,
            cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(body["type"], "about:blank");
        assert_eq!(body["title"], "Entry does not balance");
        assert_eq!(body["status"], 422);
        assert_eq!(body["detail"], "debits must equal credits");
        assert_eq!(body["baseCurrency"], "EUR");
        assert_eq!(body["baseDifference"], "-20.00");
        assert_eq!(body["currencyDifferences"][0]["currency"], "USD");
        assert_eq!(body["currencyDifferences"][0]["difference"], "1.50");
    }

    #[test]
    fn refusal_problems_map_to_their_slugs() {
        // Asserted on the problem directly: the rendered body shape is
        // pinned once, by the unbalanced test above.
        let cases: [(JournalError, u16, &str); 6] = [
            (
                JournalError::JournalNotFound {
                    id: "01E".to_owned(),
                },
                404,
                "journal-not-found",
            ),
            (
                JournalError::JournalExists {
                    code: "SJ".to_owned(),
                },
                409,
                "journal-exists",
            ),
            (
                JournalError::PeriodNotOpen {
                    period: "2026-04".to_owned(),
                },
                422,
                "journal-entry-refused",
            ),
            (
                JournalError::ForeignJournal {
                    journal: "01J".to_owned(),
                },
                422,
                "journal-entry-refused",
            ),
            (
                JournalError::Conflict { attempts: 8 },
                503,
                "journal-post-conflict",
            ),
            (
                JournalError::ZeroLine {
                    account: "01000".to_owned(),
                },
                422,
                "journal-entry-refused",
            ),
        ];
        for (error, status, slug) in cases {
            let problem = error.to_problem();
            assert_eq!(problem.status.as_u16(), status, "{error}");
            assert_eq!(problem.slug, slug, "{error}");
            assert!(!problem.title.is_empty(), "{error}");
            assert!(problem.detail.is_some(), "{error} carries a detail");
        }
    }

    #[test]
    fn internal_errors_carry_no_internals() {
        let error = JournalError::Money(MoneyError::Overflow);
        let problem = error.to_problem();
        let response = problem.into_response();
        assert_eq!(response.status().as_u16(), 500);
        let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
            response.into_body(),
            usize::MAX,
        ))
        .expect("a body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("problem+json");
        assert_eq!(body["title"], "Internal error");
        assert!(body.get("detail").is_none());
    }
}
