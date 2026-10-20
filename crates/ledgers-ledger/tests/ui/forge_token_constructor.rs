//! Compile-fail: cannot call ExecToken::new() or Default::default() because these are private.

fn main() {
    check_new_fails();
    check_default_fails();
}

#[allow(dead_code)]
fn check_new_fails() {
    let _token = ledgers_commands::ExecToken::new();
}

#[allow(dead_code)]
fn check_default_fails() {
    let _token = <ledgers_commands::ExecToken as Default>::default();
}
