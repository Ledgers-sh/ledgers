//! The clap command tree, generated from the command registry.
//!
//! Each registry command becomes a subcommand path: `bills.match` is
//! `ledgers bills match`, and `reports.trial_balance` is `ledgers reports
//! trial-balance`. A leaf has:
//!
//! - one positional argument per `{placeholder}` in its HTTP path,
//! - `--input <JSON|@FILE|->`, the full input object,
//! - one `--<field>` flag per input-schema property (not for reserved names),
//! - `--dry-run`, only if the command supports it.
//!
//! Its help text comes from the input schema's `description` fields and the
//! command's name, HTTP route, tier and idempotence.

use crate::error::{CliError, Result};
use crate::request::{Invocation, path_param_names, property_type, schema_properties};
use clap::{Arg, ArgAction, ArgMatches, Command};
use ledgers_commands::{CommandDescriptor, NOT_TAX_ADVICE, Registry};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Top-level commands that are not generated from the registry.
pub const BUILTIN_COMMANDS: [&str; 3] = ["serve", "completions", "mcp"];

/// Long flag names a registry field may not use (they belong to the CLI itself).
const RESERVED_LONG: [&str; 8] = [
    "input",
    "dry-run",
    "json",
    "profile",
    "url",
    "read-only",
    "help",
    "version",
];

/// Global flags, parsed from the root matches.
#[derive(Clone, Debug, Default)]
pub struct GlobalFlags {
    /// `--json`: machine output.
    pub json: bool,
    /// `--profile NAME`.
    pub profile: Option<String>,
    /// `--url URL`.
    pub url: Option<String>,
}

impl GlobalFlags {
    /// Extracts the global flags from root matches.
    #[must_use]
    pub fn from_matches(matches: &ArgMatches) -> Self {
        GlobalFlags {
            json: matches.get_flag("json"),
            profile: matches.get_one::<String>("profile").cloned(),
            url: matches.get_one::<String>("url").cloned(),
        }
    }
}

/// Builds the CLI registry from the commands the workspace defines.
///
/// # Errors
/// Returns an error if a command is registered twice, if two commands map to
/// the same CLI path, if one command's path is a prefix of another's, or if a
/// top-level group would shadow a built-in command.
pub fn cli_registry() -> std::result::Result<Registry, String> {
    build_cli_registry(ledgers_commands::registry::default_commands())
}

/// Builds a CLI registry from the given command descriptors, with the same
/// checks as [`cli_registry`].
///
/// # Errors
/// See [`cli_registry`].
pub fn build_cli_registry(
    descriptors: Vec<CommandDescriptor>,
) -> std::result::Result<Registry, String> {
    let mut registry = Registry::new();
    for descriptor in descriptors {
        registry.register(descriptor)?;
    }

    let mut leaves = BTreeSet::new();
    for descriptor in registry.iter() {
        let path = cli_path(&descriptor.name);
        if !leaves.insert(path.join(" ")) {
            return Err(format!(
                "two commands map to the CLI path `{}`",
                path.join(" ")
            ));
        }
        if BUILTIN_COMMANDS.contains(&path[0].as_str()) {
            return Err(format!(
                "command {} shadows a built-in command",
                descriptor.name
            ));
        }
    }
    for leaf in &leaves {
        let words: Vec<&str> = leaf.split(' ').collect();
        for prefix_len in 1..words.len() {
            let prefix = words[..prefix_len].join(" ");
            if leaves.contains(&prefix) {
                return Err(format!("`{prefix}` is both a command and a group"));
            }
        }
    }
    Ok(registry)
}

/// The CLI subcommand path for a registry command name: `reports.trial_balance`
/// becomes `["reports", "trial-balance"]`.
#[must_use]
pub fn cli_path(name: &str) -> Vec<String> {
    name.split('.').map(|part| part.replace('_', "-")).collect()
}

/// Finds the registry command for a parsed subcommand path.
#[must_use]
pub fn find_descriptor<'a>(
    registry: &'a Registry,
    path: &[String],
) -> Option<&'a CommandDescriptor> {
    registry.iter().find(|d| cli_path(&d.name) == path)
}

/// The subcommand names from the root down to the deepest match, and that match.
#[must_use]
pub fn leaf_path(matches: &ArgMatches) -> (Vec<String>, &ArgMatches) {
    let mut path = Vec::new();
    let mut current = matches;
    while let Some((name, sub)) = current.subcommand() {
        path.push(name.to_string());
        current = sub;
    }
    (path, current)
}

/// Builds the root command with built-ins and the registry's command tree.
#[must_use]
pub fn build_root(registry: &Registry) -> Command {
    let mut cmd = Command::new("ledgers")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Ledgers: agent-safe accounting")
        .long_about(
            "Ledgers is a double-entry accounting system designed for AI agents.\n\
             Every operation is executed with policy checks and audit logging.",
        )
        .after_help(NOT_TAX_ADVICE)
        .subcommand_required(true)
        .arg_required_else_help(true)
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
                .help("Server URL (overrides LEDGERS_URL and the profile)")
                .global(true),
        )
        .subcommand(build_serve())
        .subcommand(build_completions())
        .subcommand(build_mcp());

    let tree = group_tree(registry);
    for (name, node) in &tree.children {
        cmd = cmd.subcommand(build_node(name, node));
    }
    cmd
}

/// A node in the command tree: a group of subcommands or one leaf command.
#[derive(Default)]
struct Node<'a> {
    children: BTreeMap<String, Node<'a>>,
    leaf: Option<&'a CommandDescriptor>,
}

fn group_tree(registry: &Registry) -> Node<'_> {
    let mut root = Node::default();
    for descriptor in registry.iter() {
        let mut node = &mut root;
        for part in cli_path(&descriptor.name) {
            node = node.children.entry(part).or_default();
        }
        node.leaf = Some(descriptor);
    }
    root
}

fn build_node(name: &str, node: &Node<'_>) -> Command {
    if let Some(descriptor) = node.leaf {
        return build_leaf(name, descriptor);
    }
    let mut cmd = Command::new(intern(name))
        .about(format!("Commands under `ledgers {name}`"))
        .subcommand_required(true)
        .arg_required_else_help(true);
    for (child, child_node) in &node.children {
        cmd = cmd.subcommand(build_node(child, child_node));
    }
    cmd
}

/// Returns a `'static` copy of `text`, reusing an earlier copy when there is one.
///
/// clap 4 takes `'static` names for commands, arguments and ids. The registry's
/// names are only known at runtime, so each distinct name is leaked once; the set
/// of names is fixed by the registry, so the total is bounded.
fn intern(text: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let set = INTERNED.get_or_init(|| Mutex::new(BTreeSet::new()));
    let mut set = set.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = set.get(text) {
        return existing;
    }
    let leaked: &'static str = Box::leak(text.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// The clap argument id for a positional path placeholder.
fn path_arg_id(placeholder: &str) -> &'static str {
    intern(&format!("path:{placeholder}"))
}

/// The clap argument id for an input-schema field flag.
fn field_arg_id(field: &str) -> &'static str {
    intern(&format!("field:{field}"))
}

/// The long flag for an input field: `bill_id` becomes `--bill-id`.
fn kebab(field: &str) -> String {
    field.replace('_', "-")
}

fn build_leaf(name: &str, descriptor: &CommandDescriptor) -> Command {
    let description = descriptor
        .input_schema
        .get("description")
        .and_then(Value::as_str)
        .map_or_else(|| descriptor.name.clone(), str::to_string);
    let long_help = format!(
        "{description}\n\nCommand: {}\nHTTP: {} {}\nTier: {}\nIdempotent: {}",
        descriptor.name,
        descriptor.http_method,
        descriptor.http_path,
        descriptor.tier,
        descriptor.idempotent
    );
    let mut cmd = Command::new(intern(name))
        .about(description)
        .long_about(long_help);

    let placeholders = path_param_names(&descriptor.http_path);
    for placeholder in &placeholders {
        cmd = cmd.arg(
            Arg::new(path_arg_id(placeholder))
                .value_name(intern(&kebab(&placeholder.to_uppercase())))
                .help(format!("The {placeholder} path argument"))
                .required(true),
        );
    }

    cmd = cmd.arg(
        Arg::new("input")
            .long("input")
            .value_name("JSON|@FILE|-")
            .help("Command input as a JSON object, @FILE to read one, or - for stdin"),
    );

    let mut seen = HashSet::new();
    for (field, schema) in schema_properties(&descriptor.input_schema) {
        let long = kebab(&field);
        if RESERVED_LONG.contains(&long.as_str()) || !seen.insert(long.clone()) {
            continue;
        }
        cmd = cmd.arg(field_arg(&field, &long, &schema));
    }

    if descriptor.supports_dry_run {
        cmd = cmd.arg(
            Arg::new("dry_run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Preview the command and its policy decision; write nothing"),
        );
    }
    cmd
}

fn field_arg(field: &str, long: &str, schema: &Value) -> Arg {
    let arg = Arg::new(field_arg_id(field)).long(intern(long));
    let arg = if property_type(schema) == "boolean" {
        arg.action(ArgAction::SetTrue)
    } else {
        arg.value_name(value_name_for(property_type(schema)))
    };
    match schema.get("description").and_then(Value::as_str) {
        Some(help) => arg.help(help.to_string()),
        None => arg,
    }
}

fn value_name_for(ty: &str) -> &'static str {
    match ty {
        "string" => "TEXT",
        "integer" | "number" => "NUMBER",
        "array" | "object" => "JSON",
        _ => "VALUE",
    }
}

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

fn build_mcp() -> Command {
    Command::new("mcp")
        .about("Serve MCP over stdio, relaying to the server with your agent key")
        .long_about(
            "Reads newline-delimited JSON-RPC messages from stdin, relays each to the server's \
             /v1/mcp endpoint with your API key as Bearer token, and writes responses to stdout.\n\n\
             This allows MCP clients (like Claude) to use Ledgers as a tool.",
        )
        .arg(
            Arg::new("read_only")
                .long("read-only")
                .help("Read-only mode: only list and allow read tools")
                .action(ArgAction::SetTrue),
        )
}

/// Builds the input object for a leaf command from its parsed arguments.
///
/// Precedence within the object: `--input` first, then `--<field>` flags override
/// its keys. Path placeholders are kept in [`Invocation::path_params`].
///
/// # Errors
/// Returns [`CliError::InvalidInput`] for malformed `--input` JSON, a non-object
/// `--input`, or a field flag whose value does not parse as its schema type.
pub fn invocation(descriptor: &CommandDescriptor, matches: &ArgMatches) -> Result<Invocation> {
    let mut inv = Invocation::default();

    for placeholder in path_param_names(&descriptor.http_path) {
        if let Ok(Some(value)) = matches.try_get_one::<String>(path_arg_id(&placeholder)) {
            inv.path_params.insert(placeholder, value.clone());
        }
    }

    let mut input: Map<String, Value> = match matches.try_get_one::<String>("input") {
        Ok(Some(raw)) => read_input(raw)?,
        _ => Map::new(),
    };

    for (field, schema) in schema_properties(&descriptor.input_schema) {
        let id = field_arg_id(&field);
        if property_type(&schema) == "boolean" {
            if matches!(matches.try_get_one::<bool>(id), Ok(Some(true))) {
                input.insert(field, Value::Bool(true));
            }
        } else if let Ok(Some(raw)) = matches.try_get_one::<String>(id) {
            input.insert(
                field.clone(),
                parse_field(&field, property_type(&schema), raw)?,
            );
        }
    }

    inv.input = input;
    if descriptor.supports_dry_run {
        inv.dry_run = matches!(matches.try_get_one::<bool>("dry_run"), Ok(Some(true)));
    }
    Ok(inv)
}

/// Reads `--input`: a JSON literal, `@path` for a file, or `-` for stdin.
fn read_input(raw: &str) -> Result<Map<String, Value>> {
    let text = if raw == "-" {
        std::io::read_to_string(std::io::stdin().lock())
            .map_err(|e| CliError::InvalidInput(format!("cannot read stdin: {e}")))?
    } else if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::InvalidInput(format!("cannot read {path}: {e}")))?
    } else {
        raw.to_string()
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| CliError::InvalidInput(format!("--input is not valid JSON: {e}")))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(CliError::InvalidInput(
            "--input must be a JSON object".into(),
        )),
    }
}

/// Parses one field flag's text according to the schema type.
fn parse_field(field: &str, ty: &str, raw: &str) -> Result<Value> {
    if ty == "string" {
        return Ok(Value::String(raw.to_string()));
    }
    if ty == "any" {
        return Ok(serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string())));
    }
    let value: Value = serde_json::from_str(raw).map_err(|e| {
        CliError::InvalidInput(format!("--{} expects a {ty} value: {e}", kebab(field)))
    })?;
    let ok = match ty {
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    };
    if ok {
        Ok(value)
    } else {
        Err(CliError::InvalidInput(format!(
            "--{} expects a {ty} value",
            kebab(field)
        )))
    }
}
