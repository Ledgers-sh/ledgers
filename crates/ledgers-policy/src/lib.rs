#![forbid(unsafe_code)]
//! Agent key policy: tiers, limits, and decision-making for command authorization.
//!
//! An agent key carries a tier (`read`, `draft`, `post-within-limits`) and per-currency
//! limits. The policy engine decides whether an agent may run a command, given its
//! metadata and the journal preview (money movement). This crate implements the
//! decision logic, rolling daily limit accounting, and the [`KeyPolicy`] that
//! plugs into the command executor.
//!
//! The key types:
//! - [`AgentKey`]: the full key policy (id, tier, limits, dates, revocation status).
//! - [`CurrencyLimit`]: per-currency allowances (per-entry and per-rolling-day in minor units).
//! - [`CommandClass`]: what a command is (`Read`, `Draft`, `Post`, `HumanOnly`, `PolicyChange`).
//! - [`Actor`]: who's running it (`Person` or `Agent`).
//! - [`DecisionInput`]: what the command would do (effect, used-today, journals, accounts).
//! - [`decide`] and [`explain`]: pure decision logic returning `Allow`, `NeedsApproval`, or `Deny`.
//! - [`limit::company_day`]: converts now to the company's local calendar day in UTC.
//! - [`KeyPolicy`]: implements [`ledgers_commands::Policy`] using agent keys and
//!   rolling daily limits.

pub mod commands;
pub mod decide;
pub mod key;
pub mod limit;
pub mod module;
pub mod policy;
pub mod store;
pub mod surface;

pub use decide::{Actor, CommandClass, Decision, DecisionInput, Explanation, decide, explain};
pub use key::{AgentKey, CurrencyLimit};
pub use module::PolicyModule;
pub use policy::KeyPolicy;

/// The `agent_key` migration, in apply order: the portable set (SQLite dialect,
/// referenced as the canonical version).
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "agent_key",
        include_str!("../migrations/sqlite/0001_agent_key.sql"),
    )];

/// The Postgres override set: identical DDL for the tables.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "agent_key",
        include_str!("../migrations/postgres/0001_agent_key.sql"),
    )];

// Validate migrations are present and correctly named.
#[cfg(test)]
#[test]
fn check_migrations() {
    cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
    cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
}
