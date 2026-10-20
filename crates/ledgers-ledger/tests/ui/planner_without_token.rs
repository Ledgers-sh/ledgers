//! Compile-fail: calling plan_post_entry with something that is not an ExecToken fails.

use cratefield_core::Database;
use ledgers_ledger::entry::Entry;
use ledgers_ledger::posting::plan_post_entry;

fn main() {
    check_planner_needs_token();
}

#[allow(dead_code)]
async fn check_planner_needs_token() {
    let db: &dyn Database = todo!();
    let entry: &Entry = todo!();

    // Pass &() instead of &ExecToken: this should fail with a type mismatch.
    let _plan = plan_post_entry(
        &(),         // Wrong type: should be &ExecToken
        db,
        "company",
        entry,
        2026i64,
        "2026-02-01",
        "request_id",
    )
    .await;
}
