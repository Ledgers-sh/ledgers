//! CLI application structure: registry-driven subcommands.
//!
//! Every command in the registry becomes a clap subcommand with:
//! - Positional `<id>` argument if the command has a target parameter ({id})
//! - `--input <JSON|@file|->` for the full input object (or individual prop flags)
//! - `--dry-run` flag if the command supports it
//! - Global flags: `--json`, `--profile`, `--url`, `--key`

use clap::{Arg, ArgAction, ArgMatches, Command};
use ledgers_commands::Registry;

/// Builds the root command with registry-driven subcommands.
pub fn build_root(registry: &Registry) -> Command {
    let mut cmd = Command::new("ledgers")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Ledgers: agent-safe accounting")
        .long_about(
            "Ledgers is a double-entry accounting system designed for AI agents.\n\
             Every operation is executed with policy checks and audit logging.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        // Global flags
        .arg(
            Arg::new("json")
                .long("json")
                .help("Output machine-readable JSON")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("profile")
                .long("profile")
                .value_name("NAME")
                .help("Profile name (default: from LEDGERS_PROFILE or 'default')")
                .global(true),
        )
        .arg(
            Arg::new("url")
                .long("url")
                .value_name("URL")
                .help("Server URL (overrides profile)")
                .env("LEDGERS_URL")
                .global(true),
        )
        .arg(
            Arg::new("key")
                .long("key")
                .value_name("KEY")
                .help("API key (overrides profile)")
                .env("LEDGERS_KEY")
                .global(true),
        )
        // Subcommands
        .subcommand(build_serve())
        .subcommand(build_completions())
        .subcommand(build_mcp());

    // Add registry-driven subcommands (clone to avoid lifetime issues)
    let descriptors: Vec<_> = registry.iter().cloned().collect();
    for descriptor in descriptors {
        cmd = cmd.subcommand(build_registry_command(&descriptor));
    }

    cmd
}

/// Builds the `ledgers serve` subcommand.
fn build_serve() -> Command {
    Command::new("serve")
        .about("Start the native Ledgers server")
        .long_about(
            "Starts the native Ledgers server on the configured address.\n\n\
             Environment variables (optional):\n  \
             DATABASE_PATH — SQLite database path (default: ledgers.db)\n  \
             BLOB_DIR — document storage directory (default: ./blobs)\n  \
             LISTEN_ADDR — server address (default: 127.0.0.1:3000)",
        )
}

/// Builds the `ledgers completions` subcommand.
fn build_completions() -> Command {
    Command::new("completions")
        .about("Generate shell completions")
        .arg(
            Arg::new("shell")
                .value_name("SHELL")
                .help("Shell type: bash, zsh, fish, powershell")
                .required(true),
        )
}

/// Builds the `ledgers mcp` subcommand.
fn build_mcp() -> Command {
    Command::new("mcp")
        .about("Serve MCP over stdio, relaying to the server with your agent key")
        .long_about(
            "Reads newline-delimited JSON-RPC messages from stdin, relays each to the server's \
             /mcp endpoint with your agent key as Bearer token, and writes responses to stdout.\n\n\
             This allows MCP clients (like Claude) to use Ledgers as a tool.",
        )
        .arg(
            Arg::new("read_only")
                .long("read-only")
                .help("Read-only mode: only list and allow read tools")
                .action(ArgAction::SetTrue),
        )
}

/// Builds a registry command subcommand.
/// Maps from command name "bills.match" to "bills match".
fn build_registry_command(descriptor: &ledgers_commands::CommandDescriptor) -> Command {
    let parts: Vec<&str> = descriptor.name.split('.').collect();
    let name = descriptor.name.clone();
    let method = descriptor.http_method.clone();
    let path = descriptor.http_path.clone();
    let tier = descriptor.tier.clone();
    let idempotent = descriptor.idempotent;
    let supports_dry_run = descriptor.supports_dry_run;
    let has_target = descriptor.http_path.contains("{id}");

    // Start with the first part as the resource name
    // Use Box::leak to get 'static strings for clap
    let first_part: &'static str = Box::leak(parts[0].to_string().into_boxed_str());
    let name_static: &'static str = Box::leak(name.clone().into_boxed_str());
    let cmd = Command::new(first_part).about(name_static);

    // Nest remaining parts as subcommands
    let mut current = cmd;
    for (i, part) in parts.iter().enumerate().skip(1) {
        let part_static: &'static str = Box::leak(part.to_string().into_boxed_str());
        let mut sub = Command::new(part_static);

        // For the leaf (last part), add the actual command arguments
        if i == parts.len() - 1 {
            // Add positional id if the command has a target
            if has_target {
                sub = sub.arg(
                    Arg::new("id")
                        .value_name("ID")
                        .help("Resource ID")
                        .required(true),
                );
            }

            // Add --input flag
            sub = sub.arg(
                Arg::new("input")
                    .long("input")
                    .value_name("JSON|@FILE|->")
                    .help("Command input as JSON, file (@path), or stdin (-)"),
            );

            // Add input property flags (e.g., --bill-id, --amount)
            // These are generated from the input_schema (TODO: implement)

            // Add --dry-run if supported
            if supports_dry_run {
                sub = sub.arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .help("Preview only, don't execute")
                        .action(ArgAction::SetTrue),
                );
            }

            let long_help = format!(
                "Command: {}\n\nHTTP: {} {}\n\nIDEMPOTENT: {}\n\nTIER: {}",
                name, method, path, idempotent, tier
            );
            let help_static: &'static str = Box::leak(long_help.into_boxed_str());
            sub = sub.about(name_static).long_about(help_static);
        }

        current = current.subcommand(sub);
    }

    current
}

/// Parses global flags from the root matches.
pub struct GlobalFlags {
    pub json: bool,
    pub profile: Option<String>,
    pub url: Option<String>,
    pub key: Option<String>,
}

impl GlobalFlags {
    /// Extracts global flags from matches.
    pub fn from_matches(matches: &ArgMatches) -> Self {
        GlobalFlags {
            json: matches.get_flag("json"),
            profile: matches.get_one::<String>("profile").map(|s| s.to_string()),
            url: matches.get_one::<String>("url").map(|s| s.to_string()),
            key: matches.get_one::<String>("key").map(|s| s.to_string()),
        }
    }
}
