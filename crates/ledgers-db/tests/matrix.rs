//! The guard conformance matrix: [`suite::assert_guard_conformance`]
//! over every backing engine the environment provides — in-memory
//! SQLite always, a throwaway Postgres 16 database when
//! `FZ_TEST_POSTGRES_URL` names one (CI's service container). One test
//! definition, every dialect: the parity loop the testing kit documents,
//! the same shape as the storage and ledger matrices.

use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, suite};

#[test]
fn guard_conformance_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule)]) {
        // The suite panics with its own diagnosis on failure. Fresh
        // module instances per dialect: all_dialects runs the factory
        // once per leg.
        pollster::block_on(suite::assert_guard_conformance(&*kit.db));
    }
}
