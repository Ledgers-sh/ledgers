//! The error type for every fallible operation in this crate.
//!
//! Account operations never panic: malformed input, unknown references
//! and rejected templates all come back as `Err`([`AccountsError`]). Every
//! `Display` string is written to be read by an agent (docs/PLAN.md §4:
//! one command surface whose failures must be self-describing).

use crate::lint::{Violation, format_violations};
use crate::mapping::{MappingViolation, format_mapping_violations};
use thiserror::Error;

/// Every way an account, chart or mapping operation can fail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AccountsError {
    /// No account with this number exists in the company.
    #[error("account not found: {number}")]
    AccountNotFound {
        /// The number that was looked up.
        number: String,
    },

    /// An account with this number already exists (the chart is keyed by
    /// `(company_id, number)`).
    #[error("account already exists: {number}")]
    AccountExists {
        /// The conflicting number.
        number: String,
    },

    /// The parent named by a draft or import does not exist in the
    /// company — parents are created before children, always.
    #[error("parent account not found: {parent}")]
    ParentNotFound {
        /// The parent number that was referenced.
        parent: String,
    },

    /// A malformed account number.
    #[error("invalid account number {number:?}: {reason}")]
    InvalidNumber {
        /// The number as it was given.
        number: String,
        /// A short, static description of what was wrong.
        reason: &'static str,
    },

    /// A malformed tag (not non-empty lowercase kebab-case, a duplicate,
    /// or too many tags on one account).
    #[error("invalid tag {tag:?}")]
    InvalidTag {
        /// The offending tag (or a description, for the too-many case).
        tag: String,
    },

    /// A name key outside the `account.` namespace.
    #[error("invalid account name key {key:?}: must start with \"account.\"")]
    InvalidNameKey {
        /// The key as it was given.
        key: String,
    },

    /// A currency code `ledgers-money` does not know. Codes are validated
    /// with `Currency::from_code`; this carries the code for agents.
    #[error("unknown currency: {code}")]
    UnknownCurrency {
        /// The code that failed validation.
        code: String,
    },

    /// An account type string that is not one of the five classes.
    #[error("unknown account type: {value:?} (expected asset|liability|equity|income|expense)")]
    UnknownAccountType {
        /// The offending value.
        value: String,
    },

    /// A normal-balance string that is not debit or credit.
    #[error("unknown normal balance: {value:?} (expected debit|credit)")]
    UnknownNormalBalance {
        /// The offending value.
        value: String,
    },

    /// An `ebilanz` key was present but empty; present keys must be
    /// non-empty position identifiers.
    #[error("empty ebilanz position on account {number}")]
    EmptyEbilanzPosition {
        /// The account carrying the empty key.
        number: String,
    },

    /// A chart template failed lint; the violations list every finding so
    /// a data author can fix the file in one pass.
    #[error("chart template rejected: {}", format_violations(violations))]
    TemplateRejected {
        /// Every violation the linter found.
        violations: Vec<Violation>,
    },

    /// An SKR number mapping failed validation against its target chart.
    #[error("number mapping rejected: {}", format_mapping_violations(violations))]
    MappingRejected {
        /// Every violation the validator found.
        violations: Vec<MappingViolation>,
    },

    /// Accounts are never deleted — posted entries are immutable, so the
    /// account they reference must outlive them; archive instead.
    #[error("accounts are never deleted; archive instead (docs/PLAN.md §3 rule 3)")]
    DeleteUnsupported,

    /// A fiscal-year spec failed validation (month or label out of
    /// range); `reason` names the field and its bounds.
    #[error("invalid fiscal year: {reason}")]
    InvalidFiscalYear {
        /// What was wrong, field and bounds included.
        reason: String,
    },

    /// A fiscal year with this label already exists for the company
    /// (the table is keyed by `(company_id, label)`).
    #[error("fiscal year already exists: {label}")]
    FiscalYearExists {
        /// The conflicting label.
        label: i64,
    },

    /// No fiscal year with this label exists for the company.
    #[error("fiscal year not found: {label}")]
    FiscalYearMissing {
        /// The label that was looked up.
        label: i64,
    },

    /// The fiscal year after this one does not exist, so the year-end
    /// close has nowhere to carry its opening balances to. Open the next
    /// year first.
    #[error("next fiscal year missing: {label} (open it before closing into it)")]
    NextFiscalYearMissing {
        /// The missing year's label (the closed year's label plus one).
        label: i64,
    },

    /// The fiscal year being opened overlaps one the company already
    /// has — spans may not share a day.
    #[error("fiscal year {label} ({starts_on}..={ends_on}) overlaps an existing fiscal year")]
    FiscalYearOverlap {
        /// The label that was being opened.
        label: i64,
        /// The rejected year's inclusive start.
        starts_on: String,
        /// The rejected year's inclusive end.
        ends_on: String,
    },

    /// A period is not in the state the requested transition needs
    /// (closing a closed year's stragglers, reopening an open period,
    /// year-end with prior periods still open).
    #[error("period {period} is {from}, not valid for {to}")]
    PeriodStateInvalid {
        /// The period id whose state refused the transition.
        period: String,
        /// The state the period is actually in.
        from: crate::period::PeriodState,
        /// The transition's target state.
        to: crate::period::PeriodState,
    },

    /// The action is reserved for a person; an agent key can never
    /// perform it (docs/PLAN.md §5: closing or reopening a period is
    /// human-only, enforced server-side).
    #[error("action {action:?} is human-only; an agent key cannot perform it")]
    HumanOnly {
        /// The refused action's name (`"close_period"`, ...).
        action: String,
    },

    /// A reason is required and the empty/whitespace-only string given
    /// is not one.
    #[error("a non-empty reason is required")]
    ReasonRequired,

    /// A supplied reason is malformed: longer than 512 characters or
    /// containing control characters (reasons live on one audit line).
    #[error("invalid reason: {reason}")]
    InvalidReason {
        /// What was wrong with it.
        reason: &'static str,
    },

    /// The close was refused: every registered check ran and at least
    /// one found blockers. The report lists every blocker type with
    /// counts and links; a person may override with a reason, an agent
    /// may not.
    #[error("{report}")]
    CloseBlocked {
        /// The complete blocker report.
        report: crate::close::CloseReport,
    },

    /// The company's chart has no usable retained-earnings account: the
    /// year-end close needs exactly one leaf, unarchived account tagged
    /// `retained-earnings`.
    #[error("no single leaf unarchived retained-earnings account in company {company_id}")]
    RetainedEarningsMissing {
        /// The company whose chart lacks the account.
        company_id: String,
    },

    /// A guard batch exhausted its retry budget on races it kept losing
    /// (`ledgers_db::GuardError::Conflict`, decomposed so this enum
    /// stays `Clone`); surface as contention, not as a refusal.
    #[error("the batch still conflicts after {attempts} attempts")]
    Conflict {
        /// The attempts made, including the first.
        attempts: u32,
    },

    /// A stored period status is not one of the three states — a row
    /// this crate's state machine never wrote. Fail-loud, not guess.
    #[error("unknown period status: {value:?} (expected open|soft-closed|closed)")]
    UnknownPeriodStatus {
        /// The status string as stored.
        value: String,
    },

    /// A statement failed on the `Database` port.
    #[error(transparent)]
    Database(#[from] cratefield_core::DbError),

    /// An exact-money operation failed — overflow at the storage
    /// boundary or a currency mismatch while building a close.
    #[error(transparent)]
    Money(#[from] ledgers_money::MoneyError),

    /// A bundled or supplied TOML file could not be parsed at all.
    #[error(transparent)]
    Template(#[from] toml::de::Error),

    /// A dimension rule string is not one of the two valid values.
    #[error("unknown dimension rule: {value:?} (expected required|forbidden)")]
    UnknownDimensionRule {
        /// The offending value.
        value: String,
    },

    /// A business-unit value is malformed: the code, name, colour, logo or
    /// a transfer's amount or units.
    #[error("invalid business unit: {reason}")]
    InvalidUnit {
        /// What was wrong, in words a person can act on.
        reason: String,
    },

    /// No business unit has this code.
    #[error("business unit {code} does not exist")]
    UnitNotFound {
        /// The code asked for.
        code: String,
    },

    /// A business unit with this code already exists.
    #[error("business unit {code} already exists")]
    UnitExists {
        /// The code taken.
        code: String,
    },

    /// The unit is archived: it takes no changes and no new postings.
    #[error("business unit {code} is archived")]
    UnitArchived {
        /// The archived unit's code.
        code: String,
    },

    /// The chart has no active account carrying a tag the books need
    /// (the inter-unit clearing account, or the due-from/due-to pair).
    #[error("the chart has no active account tagged {tag}")]
    ChartTagMissing {
        /// The tag that is missing.
        tag: String,
    },

    /// More than one active account carries a tag that must name exactly one.
    #[error("more than one active account is tagged {tag}")]
    ChartTagAmbiguous {
        /// The tag that is ambiguous.
        tag: String,
    },

    /// A transfer names an account of the wrong class.
    #[error("account {number} must be an {expected} account for this transfer")]
    TransferAccount {
        /// The account number given.
        number: String,
        /// The class it must be.
        expected: String,
    },

    /// A custom account name is empty, too long or contains control
    /// characters.
    #[error("invalid account name: {reason}")]
    InvalidDisplayName {
        /// A short, static description of what was wrong.
        reason: &'static str,
    },

    /// The account cannot be archived: it still carries a non-zero balance
    /// in some base currency. Archiving retires an account from new
    /// postings; its history must net to zero first (a reversal).
    #[error(
        "account {number} has a non-zero balance and cannot be archived; post a reversal to bring it to zero first"
    )]
    AccountHasBalance {
        /// The account number.
        number: String,
    },

    /// Re-parenting would make an account its own ancestor.
    #[error("account {number} cannot sit under {parent}: that would make it its own ancestor")]
    ParentCycle {
        /// The account being moved.
        number: String,
        /// The parent that was requested.
        parent: String,
    },

    /// A child must share its parent's account class, so the chart's
    /// grouping by class stays true.
    #[error(
        "account {number} is {account_type} but its parent {parent} is {parent_type}; a child must have its parent's class"
    )]
    ParentTypeMismatch {
        /// The account being created or moved.
        number: String,
        /// The parent's number.
        parent: String,
        /// The account's class.
        account_type: String,
        /// The parent's class.
        parent_type: String,
    },

    /// A parent that already holds postings cannot take children: it would
    /// turn into a heading while it still carries a balance.
    #[error("parent account {parent} already has postings and cannot take children")]
    ParentHasPostings {
        /// The parent number.
        parent: String,
    },

    /// The account's class cannot change once it has postings or children.
    #[error(
        "account {number} cannot change class: it has {reason}; post a reversal or move its children first"
    )]
    TypeChangeRefused {
        /// The account number.
        number: String,
        /// Why the change is refused, in words.
        reason: &'static str,
    },
}

impl From<ledgers_db::GuardError> for AccountsError {
    /// Decomposes the retry helper's error so this enum stays `Clone`
    /// and `PartialEq`: conflicts keep their attempt count, database
    /// refusals pass through unchanged.
    fn from(err: ledgers_db::GuardError) -> Self {
        match err {
            ledgers_db::GuardError::Conflict { attempts } => Self::Conflict { attempts },
            ledgers_db::GuardError::Database(db) => Self::Database(db),
        }
    }
}

/// `account.operation.failed` problem: an account operation failed.
pub const ACCOUNT_OPERATION_FAILED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "account-operation-failed",
    status: cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Account operation failed",
    description: "The account operation was refused: unknown account, validation error, or \
                      conflict with existing data.",
};

/// `account.not-found` problem.
pub const ACCOUNT_NOT_FOUND: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "account-not-found",
    status: cratefield_core::axum::http::StatusCode::NOT_FOUND,
    title: "Account not found",
    description: "No account with this number exists in the company.",
};

/// `account.exists` problem: the number is taken.
pub const ACCOUNT_EXISTS: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "account-exists",
    status: cratefield_core::axum::http::StatusCode::CONFLICT,
    title: "Account already exists",
    description: "An account with this number already exists for the company.",
};

/// `unit.not_found` problem: no business unit has this code.
pub const UNIT_NOT_FOUND: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "unit-not-found",
    status: cratefield_core::axum::http::StatusCode::NOT_FOUND,
    title: "Business unit not found",
    description: "No business unit of this company has that code.",
};

/// `unit.conflict` problem: the unit code is taken, or the unit is archived.
pub const UNIT_CONFLICT: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "unit-conflict",
    status: cratefield_core::axum::http::StatusCode::CONFLICT,
    title: "Business unit conflict",
    description: "The business unit already exists, or is archived and takes no changes.",
};

/// `unit.refused` problem: a unit value or a recharge is malformed.
pub const UNIT_REFUSED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "unit-refused",
    status: cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Business unit operation refused",
    description: "The business unit value or the recharge between units is not allowed.",
};

impl AccountsError {
    /// The RFC 9457 problem this error renders as.
    ///
    /// # Panics
    /// Never on this dependency graph; `serde_json::Value` construction
    /// from strings is infallible here.
    #[must_use]
    pub fn to_problem(&self) -> cratefield_core::Problem {
        use cratefield_core::Problem;
        match self {
            AccountsError::AccountNotFound { number } => Problem::new(&ACCOUNT_NOT_FOUND)
                .with_detail(self.to_string())
                .with_extension("number", number.clone()),
            AccountsError::AccountExists { number } => Problem::new(&ACCOUNT_EXISTS)
                .with_detail(self.to_string())
                .with_extension("number", number.clone()),
            AccountsError::ParentNotFound { parent } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("parent", parent.clone()),
            AccountsError::InvalidNumber { number, reason } => {
                Problem::new(&ACCOUNT_OPERATION_FAILED)
                    .with_detail(self.to_string())
                    .with_extension("number", number.clone())
                    .with_extension("reason", reason.to_string())
            }
            AccountsError::InvalidTag { tag } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("tag", tag.clone()),
            AccountsError::InvalidNameKey { key } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("key", key.clone()),
            AccountsError::UnknownCurrency { code } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("code", code.clone()),
            AccountsError::UnknownAccountType { value } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("value", value.clone()),
            AccountsError::UnknownNormalBalance { value } => {
                Problem::new(&ACCOUNT_OPERATION_FAILED)
                    .with_detail(self.to_string())
                    .with_extension("value", value.clone())
            }
            AccountsError::AccountHasBalance { number } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("number", number.clone())
                .with_extension("reason", "non-zero-balance".to_owned()),
            AccountsError::UnitNotFound { code } => Problem::new(&UNIT_NOT_FOUND)
                .with_detail(self.to_string())
                .with_extension("code", code.clone()),
            AccountsError::UnitExists { code } | AccountsError::UnitArchived { code } => {
                Problem::new(&UNIT_CONFLICT)
                    .with_detail(self.to_string())
                    .with_extension("code", code.clone())
            }
            AccountsError::InvalidUnit { .. }
            | AccountsError::ChartTagMissing { .. }
            | AccountsError::ChartTagAmbiguous { .. }
            | AccountsError::TransferAccount { .. } => {
                Problem::new(&UNIT_REFUSED).with_detail(self.to_string())
            }
            AccountsError::ParentHasPostings { parent } => Problem::new(&ACCOUNT_OPERATION_FAILED)
                .with_detail(self.to_string())
                .with_extension("parent", parent.clone())
                .with_extension("reason", "parent-has-postings".to_owned()),
            _ => {
                // Other errors map to the generic failure problem
                Problem::new(&ACCOUNT_OPERATION_FAILED).with_detail(self.to_string())
            }
        }
    }
}
