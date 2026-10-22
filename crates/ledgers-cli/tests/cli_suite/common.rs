//! Shared test helpers: descriptors for test commands and a `run` wrapper.

use ledgers_cli::{Context, ExitCode, run};
use ledgers_commands::{CommandDescriptor, Registry};
use serde_json::{Value, json};

/// A descriptor for a test command. Its `cli`/`mcp`/`curl` fields are not read by
/// the CLI, so they are filled with plausible values.
pub fn descriptor(
    name: &str,
    method: &str,
    path: &str,
    supports_dry_run: bool,
    input_schema: Value,
) -> CommandDescriptor {
    CommandDescriptor {
        name: name.to_string(),
        tier: if method == "GET" { "Read" } else { "User" }.to_string(),
        human_only: false,
        supports_dry_run,
        idempotent: true,
        http_method: method.to_string(),
        http_path: path.to_string(),
        cli: format!("ledgers {name}"),
        mcp: Some(name.replace('.', "_")),
        curl: String::new(),
        input_schema,
        output_schema: json!({}),
    }
}

/// A registry holding the given descriptors.
pub fn registry_of(descriptors: Vec<CommandDescriptor>) -> Registry {
    ledgers_cli::app::build_cli_registry(descriptors).expect("test registry is valid")
}

/// Runs the CLI with `args` (after the program name) and returns the exit code,
/// stdout and stderr.
pub async fn run_cli(
    registry: &Registry,
    ctx: &Context,
    words: &[&str],
) -> (ExitCode, String, String) {
    let mut argv = vec!["ledgers".to_string()];
    argv.extend(words.iter().map(|word| (*word).to_string()));
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run(argv, ctx, registry, &mut out, &mut err).await;
    (
        code,
        String::from_utf8(out).expect("stdout is UTF-8"),
        String::from_utf8(err).expect("stderr is UTF-8"),
    )
}

/// A context that points at a mock server with a fixed key and no config file.
pub fn context_for(url: &str) -> Context {
    Context {
        config_path: None,
        url: Some(url.to_string()),
        api_key: Some("test-key".to_string()),
        profile: None,
    }
}
