//! The command registry: every public command in this crate, and what
//! the audit log owes for it (docs/PLAN.md §3 rule 6).
//!
//! The acceptance criterion for the audit trail is "every mutating or
//! denied command emits exactly one event", and a table nobody reads is
//! not how that is enforced — it is how it decays. So the table is data
//! ([`COMMANDS`]), the test in `tests/audit_registry.rs` reads it, and
//! a new `pub async fn` that is not in it fails the build's test run
//! rather than quietly going unaudited.
//!
//! The kinds say what the log owes:
//!
//! - [`CommandKind::Mutating`] — changes the books; exactly one
//!   `allowed` event, in the same batch as the change.
//! - [`CommandKind::HumanOnly`] — changes the books, but only for a
//!   person. An agent key's attempt writes one `denied` event; the
//!   person's run writes one `allowed` event.
//! - [`CommandKind::Refused`] — never succeeds; writes exactly one
//!   `denied` event every time it is called.
//! - [`CommandKind::Read`] — observes; no event (a list query is not a
//!   change, and auditing it would bury the changes that are).

/// What kind of audit obligation a command carries.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CommandKind {
    /// Changes the books: one `allowed` event per successful run.
    Mutating,
    /// Changes the books, but only for a person: a `denied` event for
    /// an agent key, an `allowed` one for a person.
    HumanOnly,
    /// Always refuses: one `denied` event per attempt.
    Refused,
    /// Observes only: no event.
    Read,
}

/// One entry of the registry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CommandSpec {
    /// The dotted command name the audit event records.
    pub name: &'static str,
    /// The function, `module::name` as it is exported.
    pub function: &'static str,
    /// What the audit log owes for it.
    pub kind: CommandKind,
}

/// Every public command in [`commands`], [`posting`], [`period`] and
/// [`journal`], with the audit obligation each one carries.
///
/// [`commands`]: crate::commands
/// [`posting`]: crate::posting
/// [`period`]: crate::period
/// [`journal`]: crate::journal
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "account.create",
        function: "commands::create_account",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "account.create",
        function: "commands::plan_create_account",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "account.show",
        function: "commands::show_account",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "account.list",
        function: "commands::list_accounts",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "account.archive",
        function: "commands::archive_account",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "account.delete",
        function: "commands::delete_account",
        kind: CommandKind::Refused,
    },
    CommandSpec {
        name: "account.import_template",
        function: "commands::import_template",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.create",
        function: "journal::create_journal",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.find",
        function: "journal::find_journal",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "journal.get",
        function: "journal::get_journal",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::post_entry",
        kind: CommandKind::Mutating,
    },
    // The raw and planner variants are the same command reached through
    // the port and the test scaffolding; they post through the same
    // batch and record under the same name, so the registry test counts
    // them together rather than three times over.
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::post_raw_entry",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::post_raw_entry_with",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::plan_post_batch",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::plan_post_batch_with",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "journal.post_entry",
        function: "posting::plan_post_entry",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "fiscal_year.open",
        function: "period::open_fiscal_year",
        kind: CommandKind::Mutating,
    },
    CommandSpec {
        name: "period.soft_close",
        function: "period::soft_close_period",
        kind: CommandKind::HumanOnly,
    },
    CommandSpec {
        name: "period.close",
        function: "period::close_period",
        kind: CommandKind::HumanOnly,
    },
    CommandSpec {
        name: "period.reopen",
        function: "period::reopen_period",
        kind: CommandKind::HumanOnly,
    },
    CommandSpec {
        name: "fiscal_year.close",
        function: "period::close_fiscal_year",
        kind: CommandKind::HumanOnly,
    },
    CommandSpec {
        name: "fiscal_year.get",
        function: "period::fiscal_year",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "fiscal_year.periods",
        function: "period::fiscal_year_periods",
        kind: CommandKind::Read,
    },
    CommandSpec {
        name: "period.state",
        function: "period::period_state",
        kind: CommandKind::Read,
    },
];

/// The public functions that are deliberately **not** commands, with
/// the reason each one is exempt. A function listed here is a
/// deliberate, reviewed decision; anything unlisted and unregistered
/// fails `tests/audit_registry.rs`.
pub const EXEMPT: &[(&str, &str)] = &[
    (
        "posting::dry_run_entry",
        "read-only validation; no audit event",
    ),
    (
        "posting::plan_post_batch_statements_only",
        "internal helper used by plan_post_entry; audit event appended by executor",
    ),
];
