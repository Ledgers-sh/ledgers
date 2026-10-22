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

/// Default command descriptors for the connected apps API (#79).
/// These descriptors define the HTTP/MCP/CLI surfaces for entries and reports.
#[must_use]
pub fn default_commands() -> Vec<CommandDescriptor> {
    vec![
        // Entries commands
        CommandDescriptor {
            name: "entries.post".to_string(),
            tier: "User".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "POST".to_string(),
            http_path: "/v1/entries".to_string(),
            cli: "ledgers entries post".to_string(),
            mcp: Some("entries_post".to_string()),
            curl: "curl -X POST \"$LEDGERS_URL/v1/entries\" -H \"Authorization: Bearer $LEDGERS_KEY\" --json @input.json".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        CommandDescriptor {
            name: "entries.reverse".to_string(),
            tier: "User".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "POST".to_string(),
            http_path: "/v1/entries/{external_id}/reverse".to_string(),
            cli: "ledgers entries reverse".to_string(),
            mcp: Some("entries_reverse".to_string()),
            curl: "curl -X POST \"$LEDGERS_URL/v1/entries/{external_id}/reverse\" -H \"Authorization: Bearer $LEDGERS_KEY\" --json @input.json".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        CommandDescriptor {
            name: "entries.list".to_string(),
            tier: "Read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/entries".to_string(),
            cli: "ledgers entries list".to_string(),
            mcp: Some("entries_list".to_string()),
            curl: "curl -X GET \"$LEDGERS_URL/v1/entries\" -H \"Authorization: Bearer $LEDGERS_KEY\"".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        // Reports commands
        CommandDescriptor {
            name: "reports.trial_balance".to_string(),
            tier: "Read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/reports/trial-balance".to_string(),
            cli: "ledgers reports trial-balance".to_string(),
            mcp: Some("reports_trial_balance".to_string()),
            curl: "curl -X GET \"$LEDGERS_URL/v1/reports/trial-balance\" -H \"Authorization: Bearer $LEDGERS_KEY\"".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        CommandDescriptor {
            name: "reports.profit_and_loss".to_string(),
            tier: "Read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/reports/p-and-l".to_string(),
            cli: "ledgers reports p-and-l".to_string(),
            mcp: Some("reports_profit_and_loss".to_string()),
            curl: "curl -X GET \"$LEDGERS_URL/v1/reports/p-and-l\" -H \"Authorization: Bearer $LEDGERS_KEY\"".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        CommandDescriptor {
            name: "reports.balance_sheet".to_string(),
            tier: "Read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/reports/balance-sheet".to_string(),
            cli: "ledgers reports balance-sheet".to_string(),
            mcp: Some("reports_balance_sheet".to_string()),
            curl: "curl -X GET \"$LEDGERS_URL/v1/reports/balance-sheet\" -H \"Authorization: Bearer $LEDGERS_KEY\"".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
        CommandDescriptor {
            name: "reports.account_balance_by_dimension".to_string(),
            tier: "Read".to_string(),
            human_only: false,
            supports_dry_run: false,
            idempotent: true,
            http_method: "GET".to_string(),
            http_path: "/v1/reports/account-balance-by-dimension".to_string(),
            cli: "ledgers reports account-balance-by-dimension".to_string(),
            mcp: Some("reports_account_balance_by_dimension".to_string()),
            curl: "curl -X GET \"$LEDGERS_URL/v1/reports/account-balance-by-dimension\" -H \"Authorization: Bearer $LEDGERS_KEY\"".to_string(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
        },
    ]
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
