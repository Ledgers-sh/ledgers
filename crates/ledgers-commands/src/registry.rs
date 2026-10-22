//! The command registry: metadata derived purely from the Command trait,
//! surface descriptors (CLI, MCP, HTTP, curl), and registry operations.

use crate::auth::Tier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Metadata about a command: derived purely from the Command trait.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CommandMeta {
    /// The command's name (e.g., "bills.match").
    pub name: String,
    /// The required tier.
    pub tier: Tier,
    /// Whether it's human-only.
    pub human_only: bool,
    /// Whether it supports dry-run.
    pub supports_dry_run: bool,
    /// Whether it's idempotent.
    pub idempotent: bool,
}

/// Surface descriptor for one command: name, tier, schemas, and
/// per-surface (CLI, MCP, HTTP) representations.
#[derive(Clone, Debug, Serialize)]
pub struct CommandDescriptor {
    /// The command's name (resource.verb).
    pub name: String,
    /// The required policy tier.
    pub tier: String,
    /// True if human-only.
    pub human_only: bool,
    /// True if dry-run is supported.
    pub supports_dry_run: bool,
    /// True if idempotent.
    pub idempotent: bool,
    /// The HTTP method (GET for Read tier, POST otherwise).
    pub http_method: String,
    /// The HTTP path (e.g., "/v1/bills/{id}/match").
    pub http_path: String,
    /// The CLI form (e.g., "ledgers bills match").
    pub cli: String,
    /// The MCP tool name, or None if human-only.
    pub mcp: Option<String>,
    /// A curl command example.
    pub curl: String,
    /// The input schema (`serde_json::Value` from `schemars`).
    pub input_schema: serde_json::Value,
    /// The output schema (`serde_json::Value` from `schemars`).
    pub output_schema: serde_json::Value,
}

impl CommandDescriptor {
    /// Creates a descriptor from a command trait instance.
    ///
    /// Must validate the name format and check for duplicates at registry time.
    ///
    /// # Errors
    /// Returns an error if the command name is invalid.
    pub fn from_command<C: crate::Command + ?Sized>(
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
    ) -> Result<Self, String> {
        let name = C::NAME;
        validate_command_name(name)?;

        let tier = C::TIER;
        let http_method = if tier == Tier::Read { "GET" } else { "POST" };

        let http_path = http_path_for(name, C::TARGET);
        let cli = cli_form_for(name);
        let mcp = if C::HUMAN_ONLY {
            None
        } else {
            Some(mcp_form_for(name))
        };
        let curl = curl_example(http_method, &http_path, name);

        Ok(CommandDescriptor {
            name: name.to_string(),
            tier: tier.to_string(),
            human_only: C::HUMAN_ONLY,
            supports_dry_run: C::SUPPORTS_DRY_RUN,
            idempotent: C::IDEMPOTENT,
            http_method: http_method.to_string(),
            http_path,
            cli,
            mcp,
            curl,
            input_schema,
            output_schema,
        })
    }
}

/// Validates the command name format: lowercase alphanumerics, underscores,
/// dashes, and dots, in the form "resource.verb" or "resource.sub.verb".
///
/// # Errors
/// Returns an error if the name is empty, contains uppercase or special characters,
/// or doesn't contain a dot.
fn validate_command_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("command name cannot be empty".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
    {
        return Err(format!(
            "command name {name:?} must be lowercase alphanumeric, underscores, dashes, dots"
        ));
    }
    if !name.contains('.') {
        return Err(format!(
            "command name {name:?} must contain at least one dot (resource.verb)"
        ));
    }
    Ok(())
}

/// Converts a command name to HTTP path (underscores to dashes).
#[allow(clippy::single_match_else)]
fn http_path_for(name: &str, target: Option<&str>) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    let path_parts = parts
        .iter()
        .map(|p| p.replace('_', "-"))
        .collect::<Vec<_>>();

    match target {
        Some(target_field) => {
            // e.g., "bills.match" with target "id" -> "/v1/bills/{id}/match"
            let mut result = "/v1".to_string();
            for (i, part) in path_parts.iter().enumerate() {
                if i == path_parts.len() - 1 {
                    // Last part is the verb, but we need to insert {target} before it
                    result.push('/');
                    result.push('{');
                    result.push_str(&target_field.replace('_', "-"));
                    result.push('}');
                }
                result.push('/');
                result.push_str(part);
            }
            result
        }
        None => {
            let mut result = "/v1".to_string();
            for part in path_parts {
                result.push('/');
                result.push_str(&part);
            }
            result
        }
    }
}

/// Converts a command name to CLI form: dashes instead of underscores
/// (e.g., "bills.match" -> "ledgers bills match").
fn cli_form_for(name: &str) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    let mut cli = "ledgers".to_string();
    for part in parts {
        cli.push(' ');
        cli.push_str(&part.replace('_', "-"));
    }
    cli
}

/// Converts a command name to MCP tool name: underscores become the tool
/// name separator (e.g., `"bills.match"` -> `"bills_match"`).
fn mcp_form_for(name: &str) -> String {
    name.replace('.', "_")
}

/// Generates a curl example for the command.
fn curl_example(http_method: &str, http_path: &str, _name: &str) -> String {
    let url_part = format!("$LEDGERS_URL{http_path}");
    match http_method {
        "GET" => {
            format!("curl -X GET \"{url_part}\" -H \"Authorization: Bearer $LEDGERS_KEY\"")
        }
        _ => {
            format!(
                "curl -X POST \"{url_part}\" -H \"Authorization: Bearer $LEDGERS_KEY\" --json @input.json"
            )
        }
    }
}

/// The command registry: collection of all registered commands, sorted by name,
/// with operations to add and iterate.
#[derive(Clone)]
pub struct Registry {
    commands: BTreeMap<String, CommandDescriptor>,
}

impl Registry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Registry {
            commands: BTreeMap::new(),
        }
    }

    /// Registers a command, validating its name and checking for duplicates.
    ///
    /// # Errors
    /// Returns an error if the command is already registered.
    pub fn register(&mut self, descriptor: CommandDescriptor) -> Result<(), String> {
        if self.commands.contains_key(&descriptor.name) {
            return Err(format!(
                "command {:?} is already registered",
                descriptor.name
            ));
        }
        self.commands.insert(descriptor.name.clone(), descriptor);
        Ok(())
    }

    /// Returns a command by name, or None if not found.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&CommandDescriptor> {
        self.commands.get(name)
    }

    /// Iterates over all commands in name order.
    pub fn iter(&self) -> impl Iterator<Item = &CommandDescriptor> {
        self.commands.values()
    }

    /// Serializes the registry to JSON (all descriptors in name order).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!(self.commands.values().collect::<Vec<_>>())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// The descriptors of every command the server serves, in name order: the
/// HTTP path, tier and surface names. Schemas are empty here (this crate
/// cannot see the domain types); the server fills them in from the typed
/// commands.
#[must_use]
fn entry(name: &str, tier: &str, method: &str, path: &str, cli: &str) -> CommandDescriptor {
    CommandDescriptor {
        name: name.to_string(),
        tier: tier.to_string(),
        human_only: false,
        supports_dry_run: false,
        idempotent: true,
        http_method: method.to_string(),
        http_path: path.to_string(),
        cli: cli.to_string(),
        mcp: Some(name.replace('.', "_")),
        curl: if method == "GET" {
            format!("curl -X GET \"$LEDGERS_URL{path}\" -H \"Authorization: Bearer $LEDGERS_KEY\"")
        } else {
            format!(
                "curl -X POST \"$LEDGERS_URL{path}\" -H \"Authorization: Bearer $LEDGERS_KEY\" --json @input.json"
            )
        },
        input_schema: serde_json::json!({}),
        output_schema: serde_json::json!({}),
    }
}

/// The command registry's built-in descriptors.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn default_commands() -> Vec<CommandDescriptor> {
    let mut commands = vec![
        entry(
            "accounts.list",
            "read",
            "GET",
            "/v1/accounts",
            "ledgers accounts list",
        ),
        entry(
            "accounts.get",
            "read",
            "GET",
            "/v1/accounts/{number}",
            "ledgers accounts get",
        ),
        chart_write(
            "accounts.create",
            "POST",
            "/v1/accounts",
            "ledgers accounts create",
        ),
        chart_write(
            "accounts.update",
            "POST",
            "/v1/accounts/{number}/update",
            "ledgers accounts update",
        ),
        chart_write(
            "accounts.archive",
            "POST",
            "/v1/accounts/{number}/archive",
            "ledgers accounts archive",
        ),
        chart_write(
            "accounts.restore",
            "POST",
            "/v1/accounts/{number}/restore",
            "ledgers accounts restore",
        ),
        entry(
            "entries.get",
            "read",
            "GET",
            "/v1/entries/{external_id}",
            "ledgers entries get",
        ),
        entry(
            "entries.list",
            "read",
            "GET",
            "/v1/entries",
            "ledgers entries list",
        ),
        entry(
            "entries.post",
            "post-within-limits",
            "POST",
            "/v1/entries",
            "ledgers entries post",
        ),
        entry(
            "entries.reverse",
            "post-within-limits",
            "POST",
            "/v1/entries/{external_id}/reverse",
            "ledgers entries reverse",
        ),
        entry(
            "reports.account_balance_by_dimension",
            "read",
            "GET",
            "/v1/reports/account-balance-by-dimension",
            "ledgers reports account-balance-by-dimension",
        ),
        entry(
            "reports.balance_sheet",
            "read",
            "GET",
            "/v1/reports/balance-sheet",
            "ledgers reports balance-sheet",
        ),
        entry(
            "reports.profit_and_loss",
            "read",
            "GET",
            "/v1/reports/p-and-l",
            "ledgers reports p-and-l",
        ),
        entry(
            "documents.list",
            "read",
            "GET",
            "/v1/documents",
            "ledgers documents list",
        ),
        entry(
            "documents.get",
            "read",
            "GET",
            "/v1/documents/{id}",
            "ledgers documents get",
        ),
        entry(
            "documents.download_url",
            "read",
            "GET",
            "/v1/documents/{id}/url",
            "ledgers documents url",
        ),
        entry(
            "bills.extract",
            "draft",
            "POST",
            "/v1/bills/extract",
            "ledgers bills extract",
        ),
        entry(
            "bills.create",
            "draft",
            "POST",
            "/v1/bills",
            "ledgers bills create",
        ),
        entry(
            "bills.list",
            "read",
            "GET",
            "/v1/bills",
            "ledgers bills list",
        ),
        entry(
            "bills.get",
            "read",
            "GET",
            "/v1/bills/{id}",
            "ledgers bills get",
        ),
        entry(
            "bills.update",
            "draft",
            "POST",
            "/v1/bills/{id}/update",
            "ledgers bills update",
        ),
        entry(
            "bills.discard",
            "draft",
            "POST",
            "/v1/bills/{id}/discard",
            "ledgers bills discard",
        ),
        entry(
            "bills.post",
            "post-within-limits",
            "POST",
            "/v1/bills/{id}/post",
            "ledgers bills post",
        ),
        entry(
            "reports.trial_balance",
            "read",
            "GET",
            "/v1/reports/trial-balance",
            "ledgers reports trial-balance",
        ),
        entry(
            "reports.trends",
            "read",
            "GET",
            "/v1/reports/trends",
            "ledgers reports trends",
        ),
    ];
    commands.extend(human_only_commands());
    commands.extend(approval_commands());
    commands
}

/// A chart-changing command. It is human-only by default: agent keys get a
/// 403 unless their policy sets `chart_changes`, so the MCP server does not
/// offer it as a tool. It is not idempotent.
#[must_use]
fn chart_write(name: &str, method: &str, path: &str, cli: &str) -> CommandDescriptor {
    CommandDescriptor {
        human_only: true,
        idempotent: false,
        mcp: None,
        ..entry(name, "draft", method, path, cli)
    }
}

/// The human-only commands: agents get a 403 on them, and the MCP server does
/// not offer them as tools.
#[must_use]
fn human_only_commands() -> Vec<CommandDescriptor> {
    fn human_entry(
        name: &str,
        tier: &str,
        method: &str,
        path: &str,
        cli: &str,
    ) -> CommandDescriptor {
        CommandDescriptor {
            human_only: true,
            idempotent: method == "GET",
            mcp: None,
            ..entry(name, tier, method, path, cli)
        }
    }
    vec![
        human_entry(
            "bills.confirm",
            "draft",
            "POST",
            "/v1/bills/{id}/confirm",
            "ledgers bills confirm",
        ),
        human_entry("keys.list", "read", "GET", "/v1/keys", "ledgers keys list"),
        human_entry(
            "keys.create",
            "post-within-limits",
            "POST",
            "/v1/keys",
            "ledgers keys create",
        ),
        human_entry(
            "keys.revoke",
            "post-within-limits",
            "POST",
            "/v1/keys/{id}/revoke",
            "ledgers keys revoke",
        ),
    ]
}

/// The approval inbox commands: person only, so agent keys are refused them.
fn approval_commands() -> Vec<CommandDescriptor> {
    vec![
        human_only(entry(
            "approvals.list",
            "read",
            "GET",
            "/v1/approval-requests",
            "ledgers approvals list",
        )),
        human_only(entry(
            "approvals.get",
            "read",
            "GET",
            "/v1/approval-requests/{id}",
            "ledgers approvals get",
        )),
        human_only(entry(
            "approvals.decide",
            "read",
            "POST",
            "/v1/approval-requests/{id}/decision",
            "ledgers approvals decide",
        )),
        human_only(entry(
            "approvals.decide_bulk",
            "read",
            "POST",
            "/v1/approval-requests/bulk-decision",
            "ledgers approvals decide-bulk",
        )),
    ]
}

/// Marks a descriptor human-only: agent keys are refused it.
fn human_only(mut descriptor: CommandDescriptor) -> CommandDescriptor {
    descriptor.human_only = true;
    descriptor
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_command_names() {
        assert!(validate_command_name("bills.match").is_ok());
        assert!(validate_command_name("journal.entry.post").is_ok());
        assert!(validate_command_name("bills_match").is_err()); // missing dot
        assert!(validate_command_name("Bills.Match").is_err()); // uppercase
        assert!(validate_command_name("").is_err()); // empty
    }

    #[test]
    fn http_path_generation() {
        assert_eq!(http_path_for("bills.match", None), "/v1/bills/match");
        assert_eq!(
            http_path_for("bills.match", Some("id")),
            "/v1/bills/{id}/match"
        );
        assert_eq!(
            http_path_for("entry_import.validate", None),
            "/v1/entry-import/validate"
        );
    }

    #[test]
    fn cli_form_generation() {
        assert_eq!(cli_form_for("bills.match"), "ledgers bills match");
        assert_eq!(
            cli_form_for("entry_import.validate"),
            "ledgers entry-import validate"
        );
    }

    #[test]
    fn mcp_form_generation() {
        assert_eq!(mcp_form_for("bills.match"), "bills_match");
        assert_eq!(
            mcp_form_for("entry_import.validate"),
            "entry_import_validate"
        );
    }

    #[test]
    fn registry_operations() {
        let mut registry = Registry::new();
        let desc1 = CommandDescriptor {
            name: "bills.match".to_string(),
            tier: "draft".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
            http_method: "POST".to_string(),
            http_path: "/v1/bills/match".to_string(),
            cli: "ledgers bills match".to_string(),
            mcp: Some("bills_match".to_string()),
            curl: "curl ...".to_string(),
            input_schema: json!({}),
            output_schema: json!({}),
        };
        assert!(registry.register(desc1).is_ok());
        assert!(registry.get("bills.match").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registry_command_name_validation() {
        let mut reg = Registry::new();

        let desc = CommandDescriptor {
            name: "test.insert".to_string(),
            tier: "draft".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
            http_method: "POST".to_string(),
            http_path: "/v1/test/insert".to_string(),
            cli: "ledgers test insert".to_string(),
            mcp: Some("test_insert".to_string()),
            curl: "curl ...".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        };
        assert!(reg.register(desc).is_ok());

        let dup = CommandDescriptor {
            name: "test.insert".to_string(),
            tier: "draft".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: false,
            http_method: "POST".to_string(),
            http_path: "/v1/test/insert".to_string(),
            cli: "ledgers test insert".to_string(),
            mcp: Some("test_insert".to_string()),
            curl: "curl ...".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        };
        assert!(reg.register(dup).is_err());
    }
}
