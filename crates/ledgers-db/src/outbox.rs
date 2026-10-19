//! The outbox table posting batches enqueue their events into — the
//! harness's own durable-work queue, pinned into this crate's
//! migrations.
//!
//! `cratefield_core::Outbox` ships the at-least-once queue (issue #128
//! of the harness): a module writes an outbox row **inside the same
//! `batch_atomic`** as its state change, so the row is durable exactly
//! when the change is. This module pins that pattern's table into the
//! guard foundation's migrations — the same delegation
//! [`crate::limit`] makes for the usage meter — and hands writing
//! crates the table name: [`OUTBOX_TABLE`]. A test holds
//! `0002_outbox.sql` byte-equal to
//! `Outbox::new(OUTBOX_TABLE).create_table_sql()`, so the migration
//! cannot drift from the implementation that drains it.
//!
//! Enqueueing is `Outbox::new(OUTBOX_TABLE).enqueue_statement(..)`
//! appended to the posting batch (see `ledgers-ledger`'s posting module
//! for the worked instance). Draining — claiming, completing, bounded
//! retry — is the venture's scheduled entry point via
//! `Outbox::drain_within`, not this crate's business.

/// The shared outbox table, as owned by this crate's migration.
pub const OUTBOX_TABLE: &str = "outbox";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migration_carries_the_harness_ddl_verbatim() {
        let migration = include_str!("../migrations/sqlite/0002_outbox.sql");
        let harness_ddl = cratefield_core::Outbox::new(OUTBOX_TABLE).create_table_sql();
        assert!(
            migration.contains(&harness_ddl),
            "0002_outbox.sql must carry Outbox::new(OUTBOX_TABLE).create_table_sql() byte for \
             byte, or the migrated table drifts from the implementation that drains it"
        );
    }

    #[test]
    fn the_migration_stays_in_the_portable_subset() {
        let migration = include_str!("../migrations/sqlite/0002_outbox.sql");
        assert!(
            cratefield_core::lint_portable_sql(migration).is_empty(),
            "0002_outbox.sql left the portable-SQL subset"
        );
    }
}
