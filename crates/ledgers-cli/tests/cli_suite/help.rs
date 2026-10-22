//! Snapshots of `--help` for the root, every built-in, every group, and every
//! registry command. Run `INSTA_UPDATE=always cargo test -p ledgers-cli` to refresh.

use super::common::{context_for, registry_of, run_cli};
use ledgers_cli::ExitCode;
use ledgers_cli::app::{build_root, cli_path};
use ledgers_commands::registry::default_commands;

/// The `--help` text for the command at `path` (empty path = root).
fn help_for(path: &[String]) -> String {
    let registry = registry_of(default_commands());
    let mut cmd = build_root(&registry);
    let mut current = &mut cmd;
    for name in path {
        current = current
            .find_subcommand_mut(name)
            .unwrap_or_else(|| panic!("no subcommand {name} under {path:?}"));
    }
    let mut buf = Vec::new();
    current.write_help(&mut buf).expect("help renders");
    String::from_utf8(buf).expect("help is UTF-8")
}

fn snapshot_name(path: &[String]) -> String {
    if path.is_empty() {
        "help_root".to_string()
    } else {
        format!("help_{}", path.join("_"))
    }
}

fn snapshot(path: &[String]) {
    let name = snapshot_name(path);
    insta::assert_snapshot!(name.as_str(), help_for(path));
}

fn words(path: &str) -> Vec<String> {
    path.split(' ').map(str::to_string).collect()
}

#[test]
fn root_and_builtin_help() {
    snapshot(&[]);
    for path in ["serve", "completions", "mcp"] {
        snapshot(&words(path));
    }
}

#[test]
fn group_and_command_help_for_every_registry_command() {
    let registry = registry_of(default_commands());
    let mut groups = std::collections::BTreeSet::new();
    for descriptor in registry.iter() {
        let path = cli_path(&descriptor.name);
        groups.insert(path[0].clone());
        snapshot(&path);
    }
    for group in groups {
        snapshot(&[group]);
    }
}

#[test]
fn root_help_lists_every_registry_group_and_command() {
    let registry = registry_of(default_commands());
    let help = help_for(&[]);
    for descriptor in registry.iter() {
        let group = &cli_path(&descriptor.name)[0];
        assert!(
            help.contains(group.as_str()),
            "group {group} missing from root help"
        );
    }
    let root = build_root(&registry);
    root.debug_assert();
}

#[tokio::test]
async fn completions_are_generated_from_the_registry() {
    let registry = registry_of(default_commands());
    let (code, out, _err) = run_cli(
        &registry,
        &context_for("http://127.0.0.1:9"),
        &["completions", "bash"],
    )
    .await;
    assert_eq!(code, ExitCode::Ok);
    assert!(
        out.contains("reverse"),
        "completions omit registry commands"
    );
    assert!(
        out.contains("trial-balance"),
        "completions omit hyphenated commands"
    );
}

#[tokio::test]
async fn unknown_completion_shell_is_an_error() {
    let registry = registry_of(default_commands());
    let (code, _out, err) = run_cli(
        &registry,
        &context_for("http://127.0.0.1:9"),
        &["completions", "tcsh"],
    )
    .await;
    assert_eq!(code, ExitCode::Error);
    assert!(err.contains("unsupported shell"), "{err}");
}
