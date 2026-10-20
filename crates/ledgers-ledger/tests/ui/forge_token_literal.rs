//! Compile-fail: the only way to get an ExecToken is through its private executor.
//!
//! This test verifies that external code cannot construct an ExecToken by building
//! a struct literal, since the _private field is not accessible outside the crate.

fn main() {
    let _token = ledgers_commands::ExecToken { _private: () };
}
