//! Compile-fail tests for `ExecToken`.
//!
//! These tests verify that `ExecToken` enforces its security invariants:
//! - The struct cannot be created with a literal (private field).
//! - The constructor and Default trait are private.
//! - `plan_post_entry` and `plan_create_account` require an `ExecToken` in the first parameter;
//!   passing something else fails type-checking.
//!
//! Each test case is in tests/ui/*.rs; the expected compiler error output is
//! in tests/ui/*.stderr, generated with `TRYBUILD=overwrite cargo test --test exec_token`.

#[test]
fn exec_token_forging_fails() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
