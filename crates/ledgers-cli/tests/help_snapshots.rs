//! Snapshot tests for CLI help text.
//! These tests verify that help output doesn't change unexpectedly.
//! Run with `INSTA_UPDATE=always cargo test --test help_snapshots` to update snapshots.

use clap::Command;
use ledgers_cli::app;
use ledgers_commands::{NOT_TAX_ADVICE, Registry};

fn format_help(cmd: &mut Command) -> String {
    let mut help = Vec::new();
    cmd.write_help(&mut help).ok();
    String::from_utf8(help).unwrap_or_default()
}

#[test]
fn test_root_help() {
    let registry = Registry::new();
    let mut cmd = app::build_root(&registry);
    let help = format_help(&mut cmd);

    insta::assert_snapshot!(help);
}

#[test]
fn test_serve_help() {
    let registry = Registry::new();
    let mut cmd = app::build_root(&registry);

    if let Some(serve_cmd) = cmd.find_subcommand_mut("serve") {
        let help = format_help(serve_cmd);
        insta::assert_snapshot!(help);
    } else {
        panic!("serve subcommand not found");
    }
}

#[test]
fn test_completions_help() {
    let registry = Registry::new();
    let mut cmd = app::build_root(&registry);

    if let Some(completions_cmd) = cmd.find_subcommand_mut("completions") {
        let help = format_help(completions_cmd);
        insta::assert_snapshot!(help);
    } else {
        panic!("completions subcommand not found");
    }
}

#[test]
fn test_root_help_contains_disclaimer() {
    let registry = Registry::new();
    let mut cmd = app::build_root(&registry);
    let help = format_help(&mut cmd);

    assert!(
        help.contains(NOT_TAX_ADVICE),
        "root help must contain NOT_TAX_ADVICE: {NOT_TAX_ADVICE}"
    );
}
