//! Tests for `ledgers demo init`: creating and seeding the demo company.

use super::common::run_cli;
use ledgers_cli::{Context, ExitCode, cli_registry};

#[tokio::test]
async fn help_for_demo_init_shows_usage() {
    let registry = cli_registry().expect("registry");
    let ctx = Context {
        config_path: None,
        url: None,
        api_key: None,
        profile: None,
    };

    let (code, out, _err) = run_cli(&registry, &ctx, &["demo", "init", "--help"]).await;
    assert_eq!(code, ExitCode::Ok);
    assert!(out.contains("Halden Works GmbH fixture") || out.contains("Halden Works"));
    assert!(out.contains("--year"));
}
