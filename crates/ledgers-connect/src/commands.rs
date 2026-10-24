//! The connections command surface (issue #142):
//!
//! - `connections.list` and `connections.runs` (read),
//! - `connections.create`, `connections.update`, `connections.delete` and
//!   `connections.sync_now` (owner and admin, human only),
//! - `allocation_rules.list` (read) and `allocation_rules.set` (owner and admin),
//! - `reports.costs_by_unit` (read).
//!
//! The server does the vendor I/O and the sealing, then passes the result in
//! fields the server fills (`sealed_credential`, `fetched`, `fetch_error`); the
//! plans here validate everything and write it in one batch, so a sync either
//! lands whole or not at all. A sync that the vendor refused is still recorded
//! as a failed run.

use crate::allocation::{AllocationRule, suggest_unit, unit_for, validate_rules};
use crate::connector::{self, Connector, CredentialField};
use crate::http::sanitize_vendor_text;
use crate::model::{InvoiceRecord, SyncOutput};
use crate::split::{bill_lines, net_of_gross, split};
use crate::store::{self, ConnectionRow, InvoiceRow, UsageRow};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_money::Currency;
use schemars::JsonSchema;
use sea_query::Value as SeaValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The keys every connection's config may carry, besides the connector's own.
pub const COMMON_CONFIG_KEYS: &[&str] = &["expense_account", "vat_rate_bp"];

/// The connector-specific config keys a connector reads.
#[must_use]
pub fn connector_config_keys(connector: &str) -> &'static [&'static str] {
    match connector {
        "cloudflare" => &["account_id"],
        "planetscale" => &["organization", "currency"],
        "aws" => &["tag_key"],
        "vercel" => &["team_id"],
        _ => &[],
    }
}

const MAX_NAME: usize = 80;

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn derive_id(seed: &str, label: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ledgers-connect-id:");
    hasher.update(seed.as_bytes());
    hasher.update(b"/");
    hasher.update(label.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// The bytes a vendor invoice is stored as (its source document) and their
/// SHA-256. The server stores the same bytes in the blob store.
#[must_use]
pub fn invoice_source(invoice: &InvoiceRecord) -> (String, Vec<u8>) {
    let bytes = serde_json::to_vec(invoice).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    (hex::encode(hasher.finalize()), bytes)
}

/// One connector, as the app's gallery shows it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ConnectorView {
    /// The connector id.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// The credential fields a connection needs.
    pub credential_fields: Vec<CredentialField>,
    /// The hosts it may call.
    pub hosts: Vec<String>,
    /// The config keys it reads, besides the common ones.
    pub config_keys: Vec<String>,
}

fn connector_view(connector: &dyn Connector) -> ConnectorView {
    ConnectorView {
        id: connector.id().to_owned(),
        name: connector.display_name().to_owned(),
        credential_fields: connector.credential_fields().to_vec(),
        hosts: connector
            .allowed_hosts()
            .iter()
            .map(|h| (*h).to_owned())
            .collect(),
        config_keys: connector_config_keys(connector.id())
            .iter()
            .map(|k| (*k).to_owned())
            .collect(),
    }
}

/// A connection as the books see it. The credential is never in it: only
/// whether one is held.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ConnectionView {
    /// The connection id.
    pub id: String,
    /// The connector id.
    pub connector: String,
    /// The connector's name.
    pub connector_name: String,
    /// The name the company gave it.
    pub name: String,
    /// The non-secret settings.
    pub config: Value,
    /// The unit that receives unmapped resources.
    pub default_unit: String,
    /// Whether it is demo data (recorded fixtures, no tokens).
    pub is_demo: bool,
    /// Whether a token is held (the token itself is never shown).
    pub has_credential: bool,
    /// The last successful sync, RFC 3339.
    pub last_sync_at: Option<String>,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

fn connection_view(row: &ConnectionRow, has_credential: bool) -> ConnectionView {
    let connector_name = connector::by_id(&row.connector)
        .map_or_else(|| row.connector.clone(), |c| c.display_name().to_owned());
    ConnectionView {
        id: row.id.clone(),
        connector: row.connector.clone(),
        connector_name,
        name: row.name.clone(),
        config: serde_json::from_str(&row.config_json).unwrap_or(Value::Null),
        default_unit: row.default_unit.clone(),
        is_demo: row.is_demo,
        has_credential,
        last_sync_at: row.last_sync_at.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

/// One sync run, as the history shows it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct RunView {
    /// The run id.
    pub id: String,
    /// The window start.
    pub window_since: String,
    /// The window end.
    pub window_until: String,
    /// `succeeded` or `failed`.
    pub status: String,
    /// Invoices the vendor returned in the window.
    pub invoices_found: i64,
    /// Draft bills this run created.
    pub bills_created: i64,
    /// Usage rows this run stored.
    pub usage_rows: i64,
    /// The failure, when it failed.
    pub error_message: Option<String>,
    /// The notes the run recorded.
    pub notes: Vec<String>,
    /// When the run finished.
    pub finished_at: String,
}

fn run_view(row: store::RunRow) -> RunView {
    RunView {
        id: row.id,
        window_since: row.window_since,
        window_until: row.window_until,
        status: row.status,
        invoices_found: row.invoices_found,
        bills_created: row.bills_created,
        usage_rows: row.usage_rows,
        error_message: row.error_message,
        notes: serde_json::from_str(&row.notes_json).unwrap_or_default(),
        finished_at: row.finished_at,
    }
}

/// Reads a connection's `has_credential` flag for a list of connections.
async fn credential_set(db: &dyn Database, company_id: &str) -> Result<Vec<String>, CommandError> {
    Ok(store::connections_with_credential(db, company_id).await?)
}

async fn require_unit(db: &dyn Database, company_id: &str, code: &str) -> Result<(), CommandError> {
    match ledgers_ledger::units::find_unit(db, company_id, code)
        .await
        .map_err(|err| invalid(err.to_string()))?
    {
        Some(unit) if !unit.archived => Ok(()),
        Some(_) => Err(invalid(format!("business unit {code} is archived"))),
        None => Err(invalid(format!("business unit {code} does not exist"))),
    }
}

/// Checks the common and connector config. Unknown keys are refused, so a
/// secret cannot be stored in the config by mistake.
fn check_config(connector: &dyn Connector, config: &Value) -> Result<(), CommandError> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("config must be an object"))?;
    let allowed: Vec<&str> = COMMON_CONFIG_KEYS
        .iter()
        .copied()
        .chain(connector_config_keys(connector.id()).iter().copied())
        .collect();
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(format!(
                "{key} is not a config key of {}",
                connector.display_name()
            )));
        }
    }
    let expense = object
        .get("expense_account")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if expense.is_empty() || expense.len() > 20 {
        return Err(invalid(
            "expense_account is required: the chart account the vendor's cost books to",
        ));
    }
    if let Some(rate) = object.get("vat_rate_bp") {
        let valid = rate
            .as_i64()
            .is_some_and(|value| (0..=10_000).contains(&value));
        if !valid {
            return Err(invalid(
                "vat_rate_bp is a whole number of basis points from 0 to 10000",
            ));
        }
    }
    connector.validate_config(config).map_err(invalid)
}

fn check_name(name: &str) -> Result<String, CommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_NAME {
        return Err(invalid(format!(
            "a connection name is 1 to {MAX_NAME} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn connector_or_refuse(id: &str) -> Result<Box<dyn Connector>, CommandError> {
    connector::by_id(id).ok_or_else(|| invalid(format!("{id} is not a connector Ledgers ships")))
}

fn is_month(text: &str) -> bool {
    text.len() == 7 && connector::is_date(&format!("{text}-01"))
}

// ---------------------------------------------------------------- list

/// The input of `connections.list`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListInput {}

/// The output of `connections.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// The company's connections.
    pub connections: Vec<ConnectionView>,
    /// The connectors Ledgers ships.
    pub connectors: Vec<ConnectorView>,
}

/// `connections.list`.
pub struct ConnectionsList;

#[async_trait::async_trait]
impl Command for ConnectionsList {
    const NAME: &'static str = "connections.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = ListInput;
    type Output = ListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let rows = store::connections(db, &ctx.company_id).await?;
        let with_credential = credential_set(db, &ctx.company_id).await?;
        let connections = rows
            .iter()
            .map(|row| connection_view(row, with_credential.contains(&row.id)))
            .collect();
        let connectors = connector::all()
            .iter()
            .map(|c| connector_view(c.as_ref()))
            .collect();
        Ok(Plan::with_output(ListOutput {
            connections,
            connectors,
        }))
    }
}

// ---------------------------------------------------------------- create

/// The input of `connections.create`. `sealed_credential` is filled by the
/// server from the token it received; a client that sends it is overwritten.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CreateInput {
    /// The connection id (server-filled).
    #[serde(default)]
    pub id: String,
    /// The connector id: `cloudflare`, `anthropic` or `planetscale`.
    pub connector: String,
    /// The name the company gives it.
    pub name: String,
    /// The non-secret settings for the connector.
    pub config: Value,
    /// The unit that receives unmapped resources.
    pub default_unit: String,
    /// The sealed token (server-filled).
    #[serde(default)]
    pub sealed_credential: String,
    /// Who created it (server-filled).
    #[serde(default)]
    pub created_by: String,
    /// Demo connections hold no token: they sync from recorded fixtures. Only
    /// the demo seeder sets it (server-filled, false for every client).
    #[serde(default)]
    pub demo: bool,
    /// The token, as the client sends it. The server seals it and removes this
    /// field before the command runs, so it is never in the executor's input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// `connections.create`: a person only.
pub struct ConnectionsCreate;

#[async_trait::async_trait]
impl Command for ConnectionsCreate {
    const NAME: &'static str = "connections.create";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = CreateInput;
    type Output = ConnectionView;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let connector = connector_or_refuse(&input.connector)?;
        check_name(&input.name)?;
        check_config(connector.as_ref(), &input.config)?;
        require_unit(db, &ctx.company_id, &input.default_unit).await?;
        if input.demo {
            if !input.sealed_credential.is_empty() {
                return Err(invalid("a demo connection holds no token"));
            }
        } else if input.sealed_credential.is_empty() {
            return Err(invalid("a token is required to create a connection"));
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let connector = connector_or_refuse(&input.connector)?;
        let name = check_name(&input.name)?;
        if input.id.is_empty() {
            return Err(invalid("the connection id is missing"));
        }
        let existing = db
            .query(&Statement::with_values(
                "SELECT id FROM connector_connection WHERE company_id = ? AND name = ? AND deleted_at IS NULL".to_owned(),
                vec![SeaValue::from(ctx.company_id.as_str()), SeaValue::from(name.as_str())],
            ))
            .await?;
        if !existing.rows.is_empty() {
            return Err(invalid(format!("a connection named {name} already exists")));
        }
        let config_json =
            serde_json::to_string(&input.config).map_err(|err| invalid(err.to_string()))?;
        let row = ConnectionRow {
            id: input.id.clone(),
            connector: connector.id().to_owned(),
            name: name.clone(),
            config_json: config_json.clone(),
            default_unit: input.default_unit.clone(),
            is_demo: false,
            last_sync_at: None,
            created_at: ctx.now.clone(),
            updated_at: ctx.now.clone(),
        };
        let is_demo = i64::from(input.demo);
        let mut statements = vec![Statement::with_values(
            "INSERT INTO connector_connection (id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)".to_owned(),
            vec![
                SeaValue::from(input.id.as_str()),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(connector.id()),
                SeaValue::from(name.as_str()),
                SeaValue::from(config_json.as_str()),
                SeaValue::from(input.default_unit.as_str()),
                SeaValue::from(is_demo),
                SeaValue::from(input.created_by.as_str()),
                SeaValue::from(ctx.now.as_str()),
                SeaValue::from(ctx.now.as_str()),
            ],
        )];
        if !input.demo {
            statements.push(Statement::with_values(
                "INSERT INTO connector_credential (connection_id, company_id, sealed, updated_at) VALUES (?, ?, ?, ?)".to_owned(),
                vec![
                    SeaValue::from(input.id.as_str()),
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(input.sealed_credential.as_str()),
                    SeaValue::from(ctx.now.as_str()),
                ],
            ));
        }
        let row = ConnectionRow {
            is_demo: input.demo,
            ..row
        };
        Ok(Plan {
            statements,
            output: connection_view(&row, !input.demo),
            objects: vec![input.id.clone()],
        })
    }
}

// ---------------------------------------------------------------- update

/// The input of `connections.update`. Every field is optional; a sealed
/// credential is written only when the server fills it (a token was sent).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct UpdateInput {
    /// The connection.
    pub id: String,
    /// A new name.
    pub name: Option<String>,
    /// A new config (replaces the old one).
    pub config: Option<Value>,
    /// A new default unit.
    pub default_unit: Option<String>,
    /// A new sealed token (server-filled).
    #[serde(default)]
    pub sealed_credential: Option<String>,
    /// A new token, as the client sends it: sealed and removed by the server
    /// before the command runs, the same as on create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// `connections.update`: a person only.
pub struct ConnectionsUpdate;

#[async_trait::async_trait]
impl Command for ConnectionsUpdate {
    const NAME: &'static str = "connections.update";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = UpdateInput;
    type Output = ConnectionView;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let current = live_connection(db, ctx, &input.id).await?;
        if let Some(name) = &input.name {
            check_name(name)?;
        }
        if let Some(config) = &input.config {
            let connector = connector_or_refuse(&current.connector)?;
            check_config(connector.as_ref(), config)?;
        }
        if let Some(unit) = &input.default_unit {
            require_unit(db, &ctx.company_id, unit).await?;
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let mut row = live_connection(db, ctx, &input.id).await?;
        let mut statements = Vec::new();
        if let Some(name) = &input.name {
            row.name = check_name(name)?;
        }
        if let Some(config) = &input.config {
            row.config_json =
                serde_json::to_string(config).map_err(|err| invalid(err.to_string()))?;
        }
        if let Some(unit) = &input.default_unit {
            row.default_unit = unit.clone();
        }
        row.updated_at = ctx.now.clone();
        statements.push(Statement::with_values(
            "UPDATE connector_connection SET name = ?, config_json = ?, default_unit = ?, updated_at = ? WHERE company_id = ? AND id = ? AND deleted_at IS NULL".to_owned(),
            vec![
                SeaValue::from(row.name.as_str()),
                SeaValue::from(row.config_json.as_str()),
                SeaValue::from(row.default_unit.as_str()),
                SeaValue::from(ctx.now.as_str()),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(input.id.as_str()),
            ],
        ));
        if let Some(sealed) = &input.sealed_credential {
            statements.push(Statement::with_values(
                "UPDATE connector_credential SET sealed = ?, updated_at = ? WHERE company_id = ? AND connection_id = ?".to_owned(),
                vec![
                    SeaValue::from(sealed.as_str()),
                    SeaValue::from(ctx.now.as_str()),
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(input.id.as_str()),
                ],
            ));
        }
        // Unposted drafts follow the new default unit and config.
        statements.extend(resplit_statements(db, ctx, &row).await?);
        let held = input.sealed_credential.is_some()
            || credential_set(db, &ctx.company_id).await?.contains(&row.id);
        Ok(Plan {
            statements,
            output: connection_view(&row, held),
            objects: vec![row.id.clone()],
        })
    }
}

async fn live_connection(
    db: &dyn Database,
    ctx: &CommandContext,
    id: &str,
) -> Result<ConnectionRow, CommandError> {
    store::connection(db, &ctx.company_id, id)
        .await?
        .ok_or_else(|| invalid(format!("connection {id} does not exist")))
}

// ---------------------------------------------------------------- delete

/// The input of `connections.delete`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DeleteInput {
    /// The connection.
    pub id: String,
}

/// The output of `connections.delete`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DeleteOutput {
    /// The connection id.
    pub id: String,
    /// Always true when the plan runs.
    pub disconnected: bool,
}

/// `connections.delete`: removes the token and stops syncing. The history and
/// the bills already drafted stay, as the books keep them.
pub struct ConnectionsDelete;

#[async_trait::async_trait]
impl Command for ConnectionsDelete {
    const NAME: &'static str = "connections.delete";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = DeleteInput;
    type Output = DeleteOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        live_connection(db, ctx, &input.id).await?;
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        live_connection(db, ctx, &input.id).await?;
        let statements = vec![
            Statement::with_values(
                "DELETE FROM connector_credential WHERE company_id = ? AND connection_id = ?".to_owned(),
                vec![SeaValue::from(ctx.company_id.as_str()), SeaValue::from(input.id.as_str())],
            ),
            Statement::with_values(
                "UPDATE connector_connection SET deleted_at = ?, updated_at = ? WHERE company_id = ? AND id = ? AND deleted_at IS NULL".to_owned(),
                vec![
                    SeaValue::from(ctx.now.as_str()),
                    SeaValue::from(ctx.now.as_str()),
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(input.id.as_str()),
                ],
            ),
        ];
        Ok(Plan {
            statements,
            output: DeleteOutput {
                id: input.id.clone(),
                disconnected: true,
            },
            objects: vec![input.id.clone()],
        })
    }
}

// ---------------------------------------------------------------- sync

/// The input of `connections.sync_now`. The server fills `run_id`, `created_by`
/// and exactly one of `fetched` (the vendor's answer) or `fetch_error`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SyncInput {
    /// The connection to sync.
    pub connection_id: String,
    /// The first day of the window, `YYYY-MM-DD`.
    pub since: String,
    /// The last day of the window, `YYYY-MM-DD`.
    pub until: String,
    /// What the vendor returned (server-filled).
    #[serde(default)]
    pub fetched: Option<SyncOutput>,
    /// Why the vendor could not be read (server-filled).
    #[serde(default)]
    pub fetch_error: Option<String>,
    /// Who ran the sync (server-filled).
    #[serde(default)]
    pub created_by: String,
}

/// `connections.sync_now`: a person only. Writes the run, the invoices as
/// draft bills (idempotent by vendor id) and the usage snapshot, in one batch.
pub struct ConnectionsSyncNow;

#[async_trait::async_trait]
impl Command for ConnectionsSyncNow {
    const NAME: &'static str = "connections.sync_now";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;

    type Input = SyncInput;
    type Output = RunView;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        live_connection(db, ctx, &input.connection_id).await?;
        connector::month_windows(&input.since, &input.until).map_err(invalid)?;
        if input.fetched.is_none() && input.fetch_error.is_none() {
            return Err(invalid("the sync has no vendor answer to record"));
        }
        Ok(JournalPreview::empty())
    }

    #[allow(clippy::too_many_lines)]
    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let connection = live_connection(db, ctx, &input.connection_id).await?;
        let run_id = ctx.event_id.clone();
        let finished_at = ctx.now.clone();
        let Some(fetched) = &input.fetched else {
            let message = input.fetch_error.clone().unwrap_or_default();
            let run = run_row(
                &run_id,
                ctx,
                input,
                RunOutcome {
                    status: "failed",
                    invoices_found: 0,
                    bills_created: 0,
                    usage_rows: 0,
                    error_message: Some(sanitize_vendor_text(&message)),
                    notes: &[],
                },
            );
            return Ok(Plan {
                statements: vec![insert_run(ctx, &connection.id, input, &run)],
                output: run_view(run),
                objects: vec![run_id],
            });
        };
        let mut statements = Vec::new();
        let mut notes = fetched
            .notes
            .iter()
            .map(|n| sanitize_vendor_text(n))
            .collect::<Vec<_>>();
        let rules = store::rules(db, &ctx.company_id, &connection.id).await?;
        validate_rules(&rules).map_err(invalid)?;
        let config: Value = serde_json::from_str(&connection.config_json).unwrap_or(Value::Null);
        let expense_account = config
            .get("expense_account")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let vat_rate = config
            .get("vat_rate_bp")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        for rule in &rules {
            require_unit(db, &ctx.company_id, &rule.unit_code).await?;
        }
        require_unit(db, &ctx.company_id, &connection.default_unit).await?;

        // Usage first: replace the snapshot for every period the vendor answered.
        let mut usage_by_period: BTreeMap<String, Vec<UsageRow>> = BTreeMap::new();
        for row in &fetched.usage {
            if !is_month(&row.period) {
                return Err(invalid(format!(
                    "usage period {} is not YYYY-MM",
                    row.period
                )));
            }
            if row.quantity < 0 {
                return Err(invalid("a usage quantity is negative"));
            }
            usage_by_period
                .entry(row.period.clone())
                .or_default()
                .push(UsageRow {
                    period: row.period.clone(),
                    resource_kind: row.resource_kind.clone(),
                    resource_id: row.resource_id.clone(),
                    resource_name: sanitize_vendor_text(&row.resource_name),
                    quantity: row.quantity,
                });
        }
        for (period, rows) in &usage_by_period {
            statements.push(Statement::with_values(
                "DELETE FROM connector_usage WHERE company_id = ? AND connection_id = ? AND period = ?".to_owned(),
                vec![
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(connection.id.as_str()),
                    SeaValue::from(period.as_str()),
                ],
            ));
            let mut merged: BTreeMap<(String, String), UsageRow> = BTreeMap::new();
            for row in rows {
                let key = (row.resource_kind.clone(), row.resource_id.clone());
                let entry = merged.entry(key).or_insert_with(|| UsageRow {
                    quantity: 0,
                    ..row.clone()
                });
                entry.quantity = entry.quantity.saturating_add(row.quantity);
            }
            for row in merged.values() {
                statements.push(Statement::with_values(
                    "INSERT INTO connector_usage (id, company_id, connection_id, period, resource_kind, resource_id, resource_name, quantity, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)".to_owned(),
                    vec![
                        SeaValue::from(derive_id(&run_id, &format!("usage/{period}/{}/{}", row.resource_kind, row.resource_id))),
                        SeaValue::from(ctx.company_id.as_str()),
                        SeaValue::from(connection.id.as_str()),
                        SeaValue::from(period.as_str()),
                        SeaValue::from(row.resource_kind.as_str()),
                        SeaValue::from(row.resource_id.as_str()),
                        SeaValue::from(row.resource_name.as_str()),
                        SeaValue::from(row.quantity),
                        SeaValue::from(ctx.now.as_str()),
                    ],
                ));
            }
        }
        let usage_rows = i64::try_from(usage_by_period.values().map(Vec::len).sum::<usize>())
            .unwrap_or(i64::MAX);

        // The usage each invoice's period is split by: this sync's answer, or
        // the stored snapshot when this sync did not read that period.
        let mut invoices_found = 0_i64;
        let mut bills_created = 0_i64;
        let known: Vec<String> = db
            .query(&Statement::with_values(
                "SELECT external_id FROM connector_invoice WHERE company_id = ? AND connection_id = ?".to_owned(),
                vec![SeaValue::from(ctx.company_id.as_str()), SeaValue::from(connection.id.as_str())],
            ))
            .await?
            .rows
            .iter()
            .filter_map(|row| row.get::<String>("external_id"))
            .collect();
        let mut unmapped_seen: Vec<String> = Vec::new();
        // Vendors and documents created earlier in this batch, so two invoices
        // of one new vendor (or one file) share the row instead of colliding.
        let mut new_vendors: BTreeMap<String, String> = BTreeMap::new();
        let mut new_documents: BTreeMap<String, String> = BTreeMap::new();
        let mut seen_external: Vec<String> = Vec::new();
        for (index, invoice) in fetched.invoices.iter().enumerate() {
            invoices_found += 1;
            let checked = check_invoice(invoice)?;
            if known.contains(&checked.external_id) || seen_external.contains(&checked.external_id)
            {
                continue;
            }
            seen_external.push(checked.external_id.clone());
            let usage_for = match usage_by_period.get(&checked.period) {
                Some(rows) => rows.clone(),
                None => {
                    store::usage(db, &ctx.company_id, &connection.id, Some(&checked.period)).await?
                }
            };
            let net_total = net_of_gross(checked.total_minor, vat_rate);
            let split_result = split(net_total, &usage_for, &rules, &connection.default_unit);
            for label in &split_result.unmapped {
                if !unmapped_seen.contains(label) {
                    unmapped_seen.push(label.clone());
                }
            }
            if split_result.no_usage {
                notes.push(format!(
                    "no usage for {} on {}: booked to the default unit",
                    checked.period, connection.name
                ));
            }
            let vendor_key = checked.vendor_name.to_lowercase();
            let vendor_id = if let Some(id) =
                store::vendor_by_name(db, &ctx.company_id, &checked.vendor_name).await?
            {
                id
            } else if let Some(id) = new_vendors.get(&vendor_key) {
                id.clone()
            } else {
                let id = derive_id(&run_id, &format!("vendor/{index}"));
                statements.push(vendor_insert(&id, ctx, &checked.vendor_name));
                new_vendors.insert(vendor_key, id.clone());
                id
            };
            let (sha, bytes) = invoice_source(&checked);
            let document_id =
                if let Some(id) = store::document_by_sha(db, &ctx.company_id, &sha).await? {
                    id
                } else if let Some(id) = new_documents.get(&sha) {
                    id.clone()
                } else {
                    let id = derive_id(&run_id, &format!("document/{index}"));
                    statements.extend(document_inserts(&id, &sha, bytes.len(), ctx, &checked));
                    new_documents.insert(sha.clone(), id.clone());
                    id
                };
            let bill_id = derive_id(&run_id, &format!("bill/{index}"));
            let lines = bill_lines(
                &split_result.parts,
                &checked.description,
                &expense_account,
                (vat_rate > 0).then_some(vat_rate),
            );
            statements.push(bill_insert(
                &bill_id,
                ctx,
                &document_id,
                &vendor_id,
                &checked,
                net_total,
                &lines,
            )?);
            let invoice_id = derive_id(&run_id, &format!("invoice/{index}"));
            statements.push(invoice_insert(
                &invoice_id,
                ctx,
                &connection.id,
                &vendor_id,
                &checked,
                &bill_id,
            ));
            for (unit, amount) in &split_result.parts {
                statements.push(split_insert(
                    &derive_id(&run_id, &format!("split/{index}/{unit}")),
                    ctx,
                    &connection.id,
                    &invoice_id,
                    &checked,
                    unit,
                    *amount,
                ));
            }
            bills_created += 1;
        }
        if !unmapped_seen.is_empty() {
            notes.push(format!(
                "{} resource(s) have no allocation rule and booked to the default unit: {}",
                unmapped_seen.len(),
                unmapped_seen.join(", ")
            ));
        }
        statements.push(Statement::with_values(
            "UPDATE connector_connection SET last_sync_at = ?, updated_at = ? WHERE company_id = ? AND id = ?".to_owned(),
            vec![
                SeaValue::from(finished_at.as_str()),
                SeaValue::from(finished_at.as_str()),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(connection.id.as_str()),
            ],
        ));
        let run = run_row(
            &run_id,
            ctx,
            input,
            RunOutcome {
                status: "succeeded",
                invoices_found,
                bills_created,
                usage_rows,
                error_message: None,
                notes: &notes,
            },
        );
        statements.push(insert_run(ctx, &connection.id, input, &run));
        Ok(Plan {
            statements,
            output: run_view(run),
            objects: vec![run_id],
        })
    }
}

/// Checks a vendor invoice before anything is written. The server fills it
/// from the connector; the checks keep a bad answer from reaching the books.
fn check_invoice(invoice: &InvoiceRecord) -> Result<InvoiceRecord, CommandError> {
    if invoice.external_id.is_empty() || invoice.external_id.len() > 200 {
        return Err(invalid("an invoice has no usable external id"));
    }
    if !is_month(&invoice.period) || !connector::is_date(&invoice.issued_on) {
        return Err(invalid(format!(
            "invoice {} has a bad date",
            invoice.external_id
        )));
    }
    let currency = Currency::from_code(&invoice.currency).map_err(|_| {
        invalid(format!(
            "invoice {} has an unknown currency",
            invoice.external_id
        ))
    })?;
    if invoice.total_minor <= 0 {
        return Err(invalid(format!(
            "invoice {} must have a positive total",
            invoice.external_id
        )));
    }
    Ok(InvoiceRecord {
        external_id: invoice.external_id.clone(),
        vendor_name: sanitize_vendor_text(&invoice.vendor_name),
        invoice_number: invoice.invoice_number.as_deref().map(sanitize_vendor_text),
        issued_on: invoice.issued_on.clone(),
        period: invoice.period.clone(),
        currency: currency.code().to_owned(),
        total_minor: invoice.total_minor,
        description: sanitize_vendor_text(&invoice.description),
        hosted_url: invoice
            .hosted_url
            .clone()
            .filter(|url| url.starts_with("https://")),
    })
}

fn vendor_insert(id: &str, ctx: &CommandContext, name: &str) -> Statement {
    Statement::with_values(
        "INSERT INTO vendor (id, company_id, name, vat_id, iban, source, needs_review, created_at, updated_at, roles, personal_data) VALUES (?, ?, ?, NULL, NULL, 'manual', 1, ?, ?, 'vendor', 0)".to_owned(),
        vec![
            SeaValue::from(id),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(name),
            SeaValue::from(ctx.now.as_str()),
            SeaValue::from(ctx.now.as_str()),
        ],
    )
}

fn document_inserts(
    id: &str,
    sha: &str,
    size: usize,
    ctx: &CommandContext,
    invoice: &InvoiceRecord,
) -> Vec<Statement> {
    let original = format!("{}-{}.json", invoice.vendor_name, invoice.external_id);
    let original = original.replace(['/', '\\', ' ', ':'], "-");
    vec![
        Statement::with_values(
            "INSERT INTO document (id, company_id, sha256, size_bytes, mime, original_name, uploaded_by, retention_class, text_layer, created_at) VALUES (?, ?, ?, ?, 'application/json', ?, ?, 'ten-years', NULL, ?)".to_owned(),
            vec![
                SeaValue::from(id),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(sha),
                SeaValue::from(i64::try_from(size).unwrap_or(0)),
                SeaValue::from(original.as_str()),
                SeaValue::from(ctx.request_id.as_str()),
                SeaValue::from(ctx.now.as_str()),
            ],
        ),
        Statement::with_values(
            "INSERT INTO document_upload_event (id, company_id, document_id, uploaded_by, original_name, uploaded_at, source) VALUES (?, ?, ?, ?, ?, ?, 'api')".to_owned(),
            vec![
                SeaValue::from(derive_id(id, "upload")),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(id),
                SeaValue::from(ctx.request_id.as_str()),
                SeaValue::from(original.as_str()),
                SeaValue::from(ctx.now.as_str()),
            ],
        ),
    ]
}

fn bill_insert(
    id: &str,
    ctx: &CommandContext,
    document_id: &str,
    vendor_id: &str,
    invoice: &InvoiceRecord,
    net_total: i64,
    lines: &[ledgers_bills::model::BillLine],
) -> Result<Statement, CommandError> {
    let lines_json = serde_json::to_string(lines).map_err(|err| invalid(err.to_string()))?;
    let vat = invoice.total_minor - net_total;
    Ok(Statement::with_values(
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, invoice_date, due_date, currency, net_minor, vat_minor, gross_minor, lines_json, confidence_json, checks_json, instruction_flags, iban_changed, source, reader, unit_code, posted_entry_id, discarded_at, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, '{}', '[]', '[]', 0, 'manual', NULL, NULL, NULL, NULL, ?, ?, ?)".to_owned(),
        vec![
            SeaValue::from(id),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(document_id),
            SeaValue::from(vendor_id),
            SeaValue::from(invoice.invoice_number.clone()),
            SeaValue::from(invoice.issued_on.as_str()),
            SeaValue::from(invoice.currency.as_str()),
            SeaValue::from(net_total),
            SeaValue::from(vat),
            SeaValue::from(invoice.total_minor),
            SeaValue::from(lines_json),
            SeaValue::from(ctx.request_id.as_str()),
            SeaValue::from(ctx.now.as_str()),
            SeaValue::from(ctx.now.as_str()),
        ],
    ))
}

fn invoice_insert(
    id: &str,
    ctx: &CommandContext,
    connection_id: &str,
    vendor_id: &str,
    invoice: &InvoiceRecord,
    bill_id: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO connector_invoice (id, company_id, connection_id, external_id, period, issued_on, vendor_id, vendor_name, invoice_number, currency, total_minor, description, bill_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".to_owned(),
        vec![
            SeaValue::from(id),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(connection_id),
            SeaValue::from(invoice.external_id.as_str()),
            SeaValue::from(invoice.period.as_str()),
            SeaValue::from(invoice.issued_on.as_str()),
            SeaValue::from(vendor_id),
            SeaValue::from(invoice.vendor_name.as_str()),
            SeaValue::from(invoice.invoice_number.clone()),
            SeaValue::from(invoice.currency.as_str()),
            SeaValue::from(invoice.total_minor),
            SeaValue::from(invoice.description.as_str()),
            SeaValue::from(bill_id),
            SeaValue::from(ctx.now.as_str()),
        ],
    )
}

fn split_insert(
    id: &str,
    ctx: &CommandContext,
    connection_id: &str,
    invoice_id: &str,
    invoice: &InvoiceRecord,
    unit: &str,
    amount: i64,
) -> Statement {
    Statement::with_values(
        "INSERT INTO connector_split (id, company_id, connection_id, invoice_id, period, unit_code, currency, amount_minor) VALUES (?, ?, ?, ?, ?, ?, ?, ?)".to_owned(),
        vec![
            SeaValue::from(id),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(connection_id),
            SeaValue::from(invoice_id),
            SeaValue::from(invoice.period.as_str()),
            SeaValue::from(unit),
            SeaValue::from(invoice.currency.as_str()),
            SeaValue::from(amount),
        ],
    )
}

/// The numbers a finished sync records.
struct RunOutcome<'a> {
    status: &'a str,
    invoices_found: i64,
    bills_created: i64,
    usage_rows: i64,
    error_message: Option<String>,
    notes: &'a [String],
}

fn run_row(
    id: &str,
    ctx: &CommandContext,
    input: &SyncInput,
    outcome: RunOutcome<'_>,
) -> store::RunRow {
    store::RunRow {
        id: id.to_owned(),
        window_since: input.since.clone(),
        window_until: input.until.clone(),
        status: outcome.status.to_owned(),
        invoices_found: outcome.invoices_found,
        bills_created: outcome.bills_created,
        usage_rows: outcome.usage_rows,
        error_message: outcome.error_message,
        notes_json: serde_json::to_string(outcome.notes).unwrap_or_else(|_| "[]".to_owned()),
        finished_at: ctx.now.clone(),
    }
}

fn insert_run(
    ctx: &CommandContext,
    connection_id: &str,
    input: &SyncInput,
    run: &store::RunRow,
) -> Statement {
    Statement::with_values(
        "INSERT INTO connector_run (id, company_id, connection_id, window_since, window_until, status, invoices_found, bills_created, usage_rows, error_message, notes_json, created_by, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".to_owned(),
        vec![
            SeaValue::from(run.id.as_str()),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(connection_id),
            SeaValue::from(input.since.as_str()),
            SeaValue::from(input.until.as_str()),
            SeaValue::from(run.status.as_str()),
            SeaValue::from(run.invoices_found),
            SeaValue::from(run.bills_created),
            SeaValue::from(run.usage_rows),
            SeaValue::from(run.error_message.clone()),
            SeaValue::from(run.notes_json.as_str()),
            SeaValue::from(input.created_by.as_str()),
            SeaValue::from(run.finished_at.as_str()),
        ],
    )
}

/// The statements that re-split every unposted draft of a connection under
/// its current rules, default unit and config. Posted and discarded bills keep
/// their split.
async fn resplit_statements(
    db: &dyn Database,
    ctx: &CommandContext,
    connection: &ConnectionRow,
) -> Result<Vec<Statement>, CommandError> {
    let rules = store::rules(db, &ctx.company_id, &connection.id).await?;
    let config: Value = serde_json::from_str(&connection.config_json).unwrap_or(Value::Null);
    let expense_account = config
        .get("expense_account")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let vat_rate = config
        .get("vat_rate_bp")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut statements = Vec::new();
    for invoice in store::invoices(db, &ctx.company_id, &connection.id).await? {
        let Some(bill_id) = invoice.bill_id.clone() else {
            continue;
        };
        if store::bill_is_frozen(db, &ctx.company_id, &bill_id).await? {
            continue;
        }
        statements.extend(
            split_statements(
                db,
                ctx,
                connection,
                &rules,
                &expense_account,
                vat_rate,
                &invoice,
                &bill_id,
            )
            .await?,
        );
    }
    Ok(statements)
}

#[allow(clippy::too_many_arguments)]
async fn split_statements(
    db: &dyn Database,
    ctx: &CommandContext,
    connection: &ConnectionRow,
    rules: &[AllocationRule],
    expense_account: &str,
    vat_rate: i64,
    invoice: &InvoiceRow,
    bill_id: &str,
) -> Result<Vec<Statement>, CommandError> {
    let usage = store::usage(db, &ctx.company_id, &connection.id, Some(&invoice.period)).await?;
    let net_total = net_of_gross(invoice.total_minor, vat_rate);
    let result = split(net_total, &usage, rules, &connection.default_unit);
    let lines = bill_lines(
        &result.parts,
        &invoice.description,
        expense_account,
        (vat_rate > 0).then_some(vat_rate),
    );
    let lines_json = serde_json::to_string(&lines).map_err(|err| invalid(err.to_string()))?;
    let mut statements = vec![
        Statement::with_values(
            "DELETE FROM connector_split WHERE company_id = ? AND invoice_id = ?".to_owned(),
            vec![SeaValue::from(ctx.company_id.as_str()), SeaValue::from(invoice.id.as_str())],
        ),
        Statement::with_values(
            "UPDATE bill SET lines_json = ?, net_minor = ?, vat_minor = ?, updated_at = ? WHERE company_id = ? AND id = ? AND posted_entry_id IS NULL"
                .to_owned(),
            vec![
                SeaValue::from(lines_json),
                SeaValue::from(net_total),
                SeaValue::from(invoice.total_minor - net_total),
                SeaValue::from(ctx.now.as_str()),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(bill_id),
            ],
        ),
    ];
    for (unit, amount) in &result.parts {
        statements.push(Statement::with_values(
            "INSERT INTO connector_split (id, company_id, connection_id, invoice_id, period, unit_code, currency, amount_minor) VALUES (?, ?, ?, ?, ?, ?, ?, ?)".to_owned(),
            vec![
                SeaValue::from(derive_id(&invoice.id, &format!("resplit/{}/{unit}", ctx.event_id))),
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(connection.id.as_str()),
                SeaValue::from(invoice.id.as_str()),
                SeaValue::from(invoice.period.as_str()),
                SeaValue::from(unit.as_str()),
                SeaValue::from(invoice.currency.as_str()),
                SeaValue::from(*amount),
            ],
        ));
    }
    Ok(statements)
}

// ---------------------------------------------------------------- runs

/// The input of `connections.runs`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RunsInput {
    /// The connection.
    pub connection_id: String,
    /// The most runs to return (default 20, at most 100).
    pub limit: Option<u32>,
}

/// The output of `connections.runs`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct RunsOutput {
    /// The runs, newest first.
    pub runs: Vec<RunView>,
}

/// `connections.runs`: the sync history.
pub struct ConnectionsRuns;

#[async_trait::async_trait]
impl Command for ConnectionsRuns {
    const NAME: &'static str = "connections.runs";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = RunsInput;
    type Output = RunsOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        live_connection(db, ctx, &input.connection_id).await?;
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        let rows = store::runs(db, &ctx.company_id, &input.connection_id, limit).await?;
        Ok(Plan::with_output(RunsOutput {
            runs: rows.into_iter().map(run_view).collect(),
        }))
    }
}

// ---------------------------------------------------------------- allocation

/// The input of `allocation_rules.list`: all connections, or one.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct AllocationListInput {
    /// Only this connection, when given.
    pub connection_id: Option<String>,
}

/// A resource with usage and no rule: the needs-mapping queue.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnmappedResource {
    /// The resource kind.
    pub resource_kind: String,
    /// The vendor's resource id.
    pub resource_id: String,
    /// The name it was given.
    pub resource_name: String,
    /// The periods it had usage in.
    pub periods: Vec<String>,
    /// The unit whose name or code it resembles, as a suggestion only.
    pub suggested_unit: Option<String>,
}

/// One connection's rules, default and queue.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ConnectionAllocation {
    /// The connection id.
    pub connection_id: String,
    /// The connection name.
    pub name: String,
    /// The default unit.
    pub default_unit: String,
    /// The rules in order.
    pub rules: Vec<AllocationRule>,
    /// Resources waiting for a mapping.
    pub unmapped: Vec<UnmappedResource>,
}

/// A business unit the mapping can name.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitChoice {
    /// The unit code.
    pub code: String,
    /// The unit name.
    pub name: String,
}

/// The output of `allocation_rules.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AllocationListOutput {
    /// Each connection's allocation.
    pub connections: Vec<ConnectionAllocation>,
    /// The active units.
    pub units: Vec<UnitChoice>,
}

/// `allocation_rules.list`.
pub struct AllocationRulesList;

#[async_trait::async_trait]
impl Command for AllocationRulesList {
    const NAME: &'static str = "allocation_rules.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = AllocationListInput;
    type Output = AllocationListOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let units: Vec<UnitChoice> = ledgers_ledger::units::list_units(db, &ctx.company_id, false)
            .await
            .map_err(|err| invalid(err.to_string()))?
            .into_iter()
            .map(|unit| UnitChoice {
                code: unit.code,
                name: unit.name,
            })
            .collect();
        let unit_pairs: Vec<(String, String)> = units
            .iter()
            .map(|unit| (unit.code.clone(), unit.name.clone()))
            .collect();
        let mut connections = Vec::new();
        for row in store::connections(db, &ctx.company_id).await? {
            if input
                .connection_id
                .as_ref()
                .is_some_and(|wanted| wanted != &row.id)
            {
                continue;
            }
            let rules = store::rules(db, &ctx.company_id, &row.id).await?;
            let stored = store::usage(db, &ctx.company_id, &row.id, None).await?;
            let mut queue: BTreeMap<(String, String), UnmappedResource> = BTreeMap::new();
            for usage in stored {
                if unit_for(&rules, &usage.resource_id).is_some() {
                    continue;
                }
                let key = (usage.resource_kind.clone(), usage.resource_id.clone());
                let entry = queue.entry(key).or_insert_with(|| UnmappedResource {
                    resource_kind: usage.resource_kind.clone(),
                    resource_id: usage.resource_id.clone(),
                    resource_name: usage.resource_name.clone(),
                    periods: Vec::new(),
                    suggested_unit: suggest_unit(
                        &usage.resource_name,
                        &usage.resource_id,
                        &unit_pairs,
                    )
                    .map(|(code, _)| code),
                });
                if !entry.periods.contains(&usage.period) {
                    entry.periods.push(usage.period.clone());
                }
            }
            connections.push(ConnectionAllocation {
                connection_id: row.id.clone(),
                name: row.name.clone(),
                default_unit: row.default_unit.clone(),
                rules,
                unmapped: queue.into_values().collect(),
            });
        }
        Ok(Plan::with_output(AllocationListOutput {
            connections,
            units,
        }))
    }
}

/// The input of `allocation_rules.set`: the rules replace the old set, and the
/// unposted drafts of the connection are re-split under them.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct AllocationSetInput {
    /// The connection.
    pub connection_id: String,
    /// The new rules, in order.
    pub rules: Vec<AllocationRule>,
    /// A new default unit, when given.
    pub default_unit: Option<String>,
}

/// The output of `allocation_rules.set`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AllocationSetOutput {
    /// The rules now in force.
    pub rules: Vec<AllocationRule>,
    /// Drafts re-split under them.
    pub resplit: usize,
    /// Posted or discarded bills whose split stays as it was.
    pub kept_frozen: usize,
}

/// `allocation_rules.set`: a person only.
pub struct AllocationRulesSet;

#[async_trait::async_trait]
impl Command for AllocationRulesSet {
    const NAME: &'static str = "allocation_rules.set";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = AllocationSetInput;
    type Output = AllocationSetOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        live_connection(db, ctx, &input.connection_id).await?;
        validate_rules(&input.rules).map_err(invalid)?;
        for rule in &input.rules {
            require_unit(db, &ctx.company_id, &rule.unit_code).await?;
        }
        if let Some(unit) = &input.default_unit {
            require_unit(db, &ctx.company_id, unit).await?;
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let mut connection = live_connection(db, ctx, &input.connection_id).await?;
        let mut statements = vec![Statement::with_values(
            "DELETE FROM allocation_rule WHERE company_id = ? AND connection_id = ?".to_owned(),
            vec![
                SeaValue::from(ctx.company_id.as_str()),
                SeaValue::from(connection.id.as_str()),
            ],
        )];
        for (position, rule) in input.rules.iter().enumerate() {
            statements.push(Statement::with_values(
                "INSERT INTO allocation_rule (id, company_id, connection_id, pattern, unit_code, position) VALUES (?, ?, ?, ?, ?, ?)".to_owned(),
                vec![
                    SeaValue::from(derive_id(&ctx.event_id, &format!("rule/{position}"))),
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(connection.id.as_str()),
                    SeaValue::from(rule.pattern.trim()),
                    SeaValue::from(rule.unit_code.as_str()),
                    SeaValue::from(i64::try_from(position).unwrap_or(0)),
                ],
            ));
        }
        if let Some(unit) = &input.default_unit {
            connection.default_unit = unit.clone();
            statements.push(Statement::with_values(
                "UPDATE connector_connection SET default_unit = ?, updated_at = ? WHERE company_id = ? AND id = ?".to_owned(),
                vec![
                    SeaValue::from(unit.as_str()),
                    SeaValue::from(ctx.now.as_str()),
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(connection.id.as_str()),
                ],
            ));
        }
        // Count what re-splits and what stays, from the same reads the statements use.
        let mut resplit = 0_usize;
        let mut kept_frozen = 0_usize;
        for invoice in store::invoices(db, &ctx.company_id, &connection.id).await? {
            match &invoice.bill_id {
                Some(bill) if store::bill_is_frozen(db, &ctx.company_id, bill).await? => {
                    kept_frozen += 1;
                }
                Some(_) => resplit += 1,
                None => {}
            }
        }
        // Re-split with the new rules: the rules are in the batch, so the split
        // reads them from the input rather than the stored set.
        let rules: Vec<AllocationRule> = input
            .rules
            .iter()
            .map(|rule| AllocationRule {
                pattern: rule.pattern.trim().to_owned(),
                unit_code: rule.unit_code.clone(),
            })
            .collect();
        let row_for_split = connection.clone();
        let config: Value = serde_json::from_str(&connection.config_json).unwrap_or(Value::Null);
        let expense_account = config
            .get("expense_account")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let vat_rate = config
            .get("vat_rate_bp")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        for invoice in store::invoices(db, &ctx.company_id, &connection.id).await? {
            let Some(bill_id) = invoice.bill_id.clone() else {
                continue;
            };
            if store::bill_is_frozen(db, &ctx.company_id, &bill_id).await? {
                continue;
            }
            statements.extend(
                split_statements(
                    db,
                    ctx,
                    &row_for_split,
                    &rules,
                    &expense_account,
                    vat_rate,
                    &invoice,
                    &bill_id,
                )
                .await?,
            );
        }
        Ok(Plan {
            statements,
            output: AllocationSetOutput {
                rules: rules.clone(),
                resplit,
                kept_frozen,
            },
            objects: vec![connection.id.clone()],
        })
    }
}

// ---------------------------------------------------------------- reports

/// The input of `reports.costs_by_unit`: months from `from` to `to`, inclusive.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CostsInput {
    /// The first month, `YYYY-MM`.
    pub from: String,
    /// The last month, `YYYY-MM`.
    pub to: String,
}

/// One cost line: a unit, a source and a month.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CostLine {
    /// `YYYY-MM`.
    pub period: String,
    /// The business unit.
    pub unit_code: String,
    /// The connector id.
    pub connector: String,
    /// The connection id.
    pub connection_id: String,
    /// The currency.
    pub currency: String,
    /// The amount in minor units.
    pub amount_minor: i64,
}

/// The output of `reports.costs_by_unit`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CostsOutput {
    /// The lines, by month, unit and source.
    pub lines: Vec<CostLine>,
}

/// `reports.costs_by_unit`: connected costs per unit, source and month. One
/// constant query over the split table; discarded bills are left out.
pub struct ReportsCostsByUnit;

#[async_trait::async_trait]
impl Command for ReportsCostsByUnit {
    const NAME: &'static str = "reports.costs_by_unit";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = CostsInput;
    type Output = CostsOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        if !is_month(&input.from) || !is_month(&input.to) || input.from > input.to {
            return Err(invalid("from and to are YYYY-MM months, from not after to"));
        }
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        if !is_month(&input.from) || !is_month(&input.to) || input.from > input.to {
            return Err(invalid("from and to are YYYY-MM months, from not after to"));
        }
        let rows = db
            .query(&Statement::with_values(
                "SELECT s.period, s.unit_code, c.connector, c.id AS connection_id, s.currency, CAST(SUM(s.amount_minor) AS BIGINT) AS amount                  FROM connector_split s                  JOIN connector_connection c ON c.id = s.connection_id AND c.company_id = s.company_id                  JOIN connector_invoice i ON i.id = s.invoice_id AND i.company_id = s.company_id                  JOIN bill b ON b.id = i.bill_id AND b.company_id = s.company_id                  WHERE s.company_id = ? AND s.period >= ? AND s.period <= ? AND c.deleted_at IS NULL AND b.discarded_at IS NULL                  GROUP BY s.period, s.unit_code, c.connector, c.id, s.currency                  ORDER BY s.period, s.unit_code, c.connector, c.id, s.currency"
                    .to_owned(),
                vec![
                    SeaValue::from(ctx.company_id.as_str()),
                    SeaValue::from(input.from.as_str()),
                    SeaValue::from(input.to.as_str()),
                ],
            ))
            .await?;
        let lines = rows
            .rows
            .iter()
            .map(|row| CostLine {
                period: row.get::<String>("period").unwrap_or_default(),
                unit_code: row.get::<String>("unit_code").unwrap_or_default(),
                connector: row.get::<String>("connector").unwrap_or_default(),
                connection_id: row.get::<String>("connection_id").unwrap_or_default(),
                currency: row.get::<String>("currency").unwrap_or_default(),
                amount_minor: row.get::<i64>("amount").unwrap_or(0),
            })
            .collect();
        Ok(Plan::with_output(CostsOutput { lines }))
    }
}

/// The input of `connections.test`: the connection to try.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TestInput {
    /// The connection whose credential is tried.
    pub connection_id: String,
}

/// The output of `connections.test`. Nothing is written: the test reads the
/// vendor for the current day and reports whether the credential and its scope
/// work. `error` is set when they do not, with no credential in it.
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct TestOutput {
    /// Whether the vendor answered for the test window.
    pub ok: bool,
    /// What the vendor said about the test, as sentences.
    pub notes: Vec<String>,
    /// Why the test failed, when it failed.
    pub error: Option<String>,
}
