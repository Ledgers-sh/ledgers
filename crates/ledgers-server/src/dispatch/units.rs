//! Business units over the command surface: `units.list` (read),
//! `units.create`, `units.rename` and `units.archive` (a person only), the
//! recharge `entries.transfer`, the side-by-side `reports.units_summary`,
//! and the unit pin of an agent key.
//!
//! The unit writes run through the ledgers-ledger functions, which write
//! the change, its guards and its `unit.*` audit event in one batch
//! (docs/PLAN.md section 3, rule 6). An agent key is refused them with a
//! 403 and one `denied` audit event, as the chart writes are.
//!
//! An agent key pinned to a unit (`AgentKey::unit`) is enforced here,
//! before the command runs: a line with no unit takes the pinned unit, and
//! a line for another unit (or a recharge from another unit) is refused
//! with a `denied` audit event. The policy engine still decides the rest.

use super::{
    Deps, EntriesPost, KeyStore, Presented, ServerDispatch, domain, load_key, parse_date,
    rfc3339_utc,
};
use crate::people::SessionAuthenticator;
use async_trait::async_trait;
use cratefield_core::{Database, Problem};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome};
use ledgers_commands::auth::{Authenticator, Principal, Tier};
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_ledger::entry::EntryDraft;
use ledgers_ledger::report::{self, UnitSummaryRow, UnitsSummary};
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, PostEntryOutput, SerializableAmount, SerializableEntryDraft,
    SerializableLine,
};
use ledgers_ledger::units::{self, TransferDraft, Unit, UnitChange, UnitDraft, UnitScope};
use ledgers_money::Amount;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// One business unit as the app and the CLI see it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitView {
    /// The short code entry lines name the unit by.
    pub code: String,
    /// The name shown to people.
    pub name: String,
    /// The colour, `#RRGGBB`.
    pub colour: String,
    /// The logo document, when one is set.
    pub logo_document_id: Option<String>,
    /// Whether the unit is archived.
    pub archived: bool,
    /// When the unit was created (RFC 3339, UTC).
    pub created_at: String,
}

impl From<Unit> for UnitView {
    fn from(unit: Unit) -> Self {
        Self {
            code: unit.code,
            name: unit.name,
            colour: unit.colour,
            logo_document_id: unit.logo_document_id,
            archived: unit.archived,
            created_at: unit.created_at,
        }
    }
}

/// The output of `units.list`: every unit, archived ones included.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitsListOutput {
    /// The units, by code.
    pub units: Vec<UnitView>,
}

/// The input of `units.list`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct UnitsListInput {}

/// `units.list`: the company's business units.
pub struct UnitsList;

#[async_trait]
impl Command for UnitsList {
    const NAME: &'static str = "units.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = UnitsListInput;
    type Output = UnitsListOutput;

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
        let units = units::list_units(db, &ctx.company_id, true)
            .await
            .map_err(|err| CommandError::Domain(err.to_problem()))?;
        Ok(Plan::with_output(UnitsListOutput {
            units: units.into_iter().map(UnitView::from).collect(),
        }))
    }
}

/// The input of `units.create`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct UnitsCreateInput {
    /// The short code: 2 to 16 letters or digits. Upper-cased on save.
    #[serde(default)]
    pub code: String,
    /// The name: 1 to 80 characters.
    #[serde(default)]
    pub name: String,
    /// The colour, `#RRGGBB`; absent takes the default blue.
    #[serde(default)]
    pub colour: Option<String>,
    /// A stored document to use as the logo.
    #[serde(default)]
    pub logo_document_id: Option<String>,
}

/// The input of `units.rename`: the code (from the path) and the fields to
/// change. Absent fields keep their value; `clear_logo` removes the logo.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct UnitsRenameInput {
    /// The unit's code (from the path).
    #[serde(default)]
    pub code: String,
    /// A new name.
    #[serde(default)]
    pub name: Option<String>,
    /// A new colour, `#RRGGBB`.
    #[serde(default)]
    pub colour: Option<String>,
    /// A new logo document.
    #[serde(default)]
    pub logo_document_id: Option<String>,
    /// Remove the logo.
    #[serde(default)]
    pub clear_logo: bool,
}

/// The input of `units.archive`: the code (from the path).
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct UnitsCodeInput {
    /// The unit's code (from the path).
    #[serde(default)]
    pub code: String,
}

/// The output of the three unit writes: the unit after the change.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitsChangedOutput {
    /// The unit.
    pub unit: UnitView,
}

/// The input of `entries.transfer`: a shared-cost recharge from one unit to
/// another. Amounts are in the company's base currency.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntriesTransferInput {
    /// The journal; defaults to the general journal `GN`.
    #[serde(default)]
    pub journal_id: Option<String>,
    /// The unit that paid the shared cost and recovers it.
    pub from_unit: String,
    /// The unit the cost is recharged to.
    pub to_unit: String,
    /// The amount, positive, in the base currency.
    pub amount: SerializableAmount,
    /// The document date, `YYYY-MM-DD`.
    pub document_date: String,
    /// The posting date, `YYYY-MM-DD`.
    pub posting_date: String,
    /// The tax point date; defaults to the posting date.
    #[serde(default)]
    pub tax_point_date: Option<String>,
    /// The description.
    pub description: String,
    /// The income account on the paying unit that books the recovery.
    pub recovery_account: String,
    /// The expense account on the receiving unit that books the cost.
    pub cost_account: String,
}

/// `entries.transfer`: posts a recharge between two units as one balanced
/// intercompany entry (`POST /v1/transfers`).
pub struct EntriesTransfer;

#[async_trait]
impl Command for EntriesTransfer {
    const NAME: &'static str = "entries.transfer";
    const TIER: Tier = Tier::PostWithinLimits;
    const IDEMPOTENT: bool = true;

    type Input = EntriesTransferInput;
    type Output = PostEntryOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let resolved = resolve_transfer(db, ctx, input).await?;
        PostEntry.preview(db, ctx, &resolved).await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let resolved = resolve_transfer(db, ctx, input).await?;
        PostEntry.plan(token, db, ctx, &resolved).await
    }
}

/// Builds the posting input of a recharge: the planned intercompany entry,
/// in the journal and fiscal year its posting date names.
async fn resolve_transfer(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &EntriesTransferInput,
) -> Result<PostEntryInput, CommandError> {
    let journal_id = match &input.journal_id {
        Some(id) => id.clone(),
        None => ledgers_ledger::journal::find_journal(db, &ctx.company_id, "GN")
            .await
            .map_err(|err| domain(&err))?
            .map(|journal| journal.id)
            .ok_or_else(|| {
                CommandError::InvalidInput("no general journal GN exists for this company".into())
            })?,
    };
    let base = report::company_currency(db, &ctx.company_id)
        .await
        .map_err(|err| domain(&err))?;
    let currency = ledgers_money::Currency::from_code(&input.amount.currency)
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    let minor: i128 = input.amount.minor.parse().map_err(|_| {
        CommandError::InvalidInput("the amount is not a whole number of minor units".into())
    })?;
    let posting_date = parse_date(&input.posting_date)?;
    let draft = TransferDraft {
        journal_id: journal_id.clone(),
        from_unit: input.from_unit.clone(),
        to_unit: input.to_unit.clone(),
        amount: Amount::new(minor, currency),
        document_date: parse_date(&input.document_date)?,
        posting_date,
        tax_point_date: input
            .tax_point_date
            .as_deref()
            .map(parse_date)
            .transpose()?,
        description: input.description.clone(),
        recovery_account: input.recovery_account.clone(),
        cost_account: input.cost_account.clone(),
        base_currency: base,
    };
    let entry = units::plan_transfer(db, &ctx.company_id, &draft)
        .await
        .map_err(|err| CommandError::Domain(err.to_problem()))?;
    Ok(PostEntryInput {
        journal_id,
        fiscal_year: i64::from(posting_date.year()),
        entry: serializable_entry(&entry),
    })
}

/// The entry draft as the posting command's input shape carries it.
fn serializable_entry(entry: &EntryDraft) -> SerializableEntryDraft {
    SerializableEntryDraft {
        journal_id: entry.journal_id.clone(),
        document_date: super::ymd(entry.document_date),
        posting_date: super::ymd(entry.posting_date),
        tax_point_date: entry.tax_point_date.map(super::ymd),
        base_currency: entry.base_currency.code().to_owned(),
        description: entry.description.clone(),
        source_documents: entry.source_documents.clone(),
        external_id: entry.external_id.clone(),
        source_app: entry.source_app.clone(),
        reverses: entry.reverses.clone(),
        lines: entry
            .lines
            .iter()
            .map(|line| SerializableLine {
                account_number: line.account_number.clone(),
                side: match line.side {
                    ledgers_ledger::entry::Side::Debit => "debit".to_owned(),
                    ledgers_ledger::entry::Side::Credit => "credit".to_owned(),
                },
                base: SerializableAmount::from(line.base),
                fx: None,
                tax_code: line.tax_code.clone(),
                dimensions: line.dimensions.clone(),
                memo: line.memo.clone(),
            })
            .collect(),
    }
}

/// One row of the units comparison, as the API returns it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitSummaryRowView {
    /// The unit code; absent on the consolidated and elimination rows.
    pub unit: Option<String>,
    /// Income for the period.
    pub revenue: SerializableAmount,
    /// Expenses for the period.
    pub expenses: SerializableAmount,
    /// Revenue minus expenses.
    pub result: SerializableAmount,
    /// Cash and bank balance through the period (debit positive).
    pub cash: SerializableAmount,
}

impl From<UnitSummaryRow> for UnitSummaryRowView {
    fn from(row: UnitSummaryRow) -> Self {
        Self {
            unit: row.unit,
            revenue: SerializableAmount::from(row.revenue),
            expenses: SerializableAmount::from(row.expenses),
            result: SerializableAmount::from(row.result),
            cash: SerializableAmount::from(row.cash),
        }
    }
}

/// The output of `reports.units_summary`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct UnitsSummaryOutput {
    /// The period, `YYYY-MM`.
    pub period: String,
    /// One row per unit, by code.
    pub units: Vec<UnitSummaryRowView>,
    /// Company totals with intercompany entries eliminated.
    pub consolidated: UnitSummaryRowView,
    /// What the intercompany entries add to the units and take away again.
    pub eliminations: UnitSummaryRowView,
}

impl From<UnitsSummary> for UnitsSummaryOutput {
    fn from(summary: UnitsSummary) -> Self {
        Self {
            period: summary.period,
            units: summary
                .units
                .into_iter()
                .map(UnitSummaryRowView::from)
                .collect(),
            consolidated: UnitSummaryRowView::from(summary.consolidated),
            eliminations: UnitSummaryRowView::from(summary.eliminations),
        }
    }
}

/// The input of `reports.units_summary`: the period.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UnitsSummaryInput {
    /// The period, `YYYY-MM`.
    pub period: String,
}

/// `reports.units_summary`: every business unit side by side for a period.
pub struct ReportsUnitsSummary;

#[async_trait]
impl Command for ReportsUnitsSummary {
    const NAME: &'static str = "reports.units_summary";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = UnitsSummaryInput;
    type Output = UnitsSummaryOutput;

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
        super::check_period(&input.period)?;
        let summary = report::units_summary(db, &ctx.company_id, &input.period)
            .await
            .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(UnitsSummaryOutput::from(summary)))
    }
}

/// Resolves a report's `unit` parameter: absent or `all` is the
/// consolidated company; a code must name one of the company's units.
///
/// # Errors
/// [`CommandError::InvalidInput`] / unit problems for a malformed or unknown code.
pub(crate) async fn resolve_scope(
    db: &dyn Database,
    company_id: &str,
    unit: Option<&str>,
) -> Result<UnitScope, CommandError> {
    let scope = UnitScope::parse(unit).map_err(|err| CommandError::Domain(err.to_problem()))?;
    if let UnitScope::Unit(code) = &scope {
        let known = units::find_unit(db, company_id, code)
            .await
            .map_err(|err| CommandError::Domain(err.to_problem()))?;
        if known.is_none() {
            return Err(CommandError::Domain(
                ledgers_ledger::AccountsError::UnitNotFound { code: code.clone() }.to_problem(),
            ));
        }
    }
    Ok(scope)
}

impl ServerDispatch {
    /// The three unit writes: a person only. The caller is authenticated
    /// first; an agent key is refused with a 403 and one `denied` event.
    pub(super) async fn units_write(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let db = deps.db.as_ref();
        let company = self.company_id.as_str();
        let at = rfc3339_utc(deps.clock.now());
        let actor = self
            .person_actor(deps, presented, request_id, command, &input, &at)
            .await?;
        let audit = AuditContext::new(actor, request_id.to_owned(), deps.id_gen.ulid());

        let unit = match command {
            "units.create" => {
                let body: UnitsCreateInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                units::create_unit(
                    db,
                    company,
                    UnitDraft {
                        code: body.code,
                        name: body.name,
                        colour: body.colour,
                        logo_document_id: body.logo_document_id,
                    },
                    at,
                    &audit,
                )
                .await
                .map_err(|err| fail(CommandError::Domain(err.to_problem())))?
            }
            "units.rename" => {
                let body: UnitsRenameInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                let logo = if body.clear_logo {
                    Some(None)
                } else {
                    body.logo_document_id.map(Some)
                };
                units::rename_unit(
                    db,
                    company,
                    &body.code,
                    UnitChange {
                        name: body.name,
                        colour: body.colour,
                        logo_document_id: logo,
                    },
                    at,
                    &audit,
                )
                .await
                .map_err(|err| fail(CommandError::Domain(err.to_problem())))?
            }
            "units.archive" => {
                let body: UnitsCodeInput = serde_json::from_value(input)
                    .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
                units::archive_unit(db, company, &body.code, at, &audit)
                    .await
                    .map_err(|err| fail(CommandError::Domain(err.to_problem())))?
            }
            other => {
                return Err(
                    Problem::validation_failed(format!("unknown unit command {other:?}"))
                        .instance(request_id),
                );
            }
        };
        serde_json::to_value(UnitsChangedOutput {
            unit: UnitView::from(unit),
        })
        .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    /// `entries.post` and `entries.transfer` with the unit pin of an agent
    /// key applied first: a missing unit on an entry line takes the pinned
    /// unit, and a line for another unit is refused.
    pub(super) async fn entries_post(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        if let Some((key_id, unit)) = self
            .pinned_unit(deps, presented.as_ref(), request_id)
            .await?
        {
            let other = input
                .pointer("/entry/lines")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|line| line.pointer("/dimensions/unit").and_then(Value::as_str))
                .find(|code| *code != unit)
                .map(str::to_owned);
            if let Some(other) = other {
                let reason =
                    format!("agent key {key_id} is pinned to unit {unit}; {other} is not allowed");
                return Err(self
                    .pinned_refusal(deps, request_id, &key_id, "entries.post", &input, reason)
                    .await);
            }
            if let Some(lines) = input
                .pointer_mut("/entry/lines")
                .and_then(Value::as_array_mut)
            {
                for line in lines {
                    if let Some(map) = line.as_object_mut() {
                        let dims = map.entry("dimensions").or_insert_with(|| json!({}));
                        if let Some(dims) = dims.as_object_mut() {
                            dims.entry("unit").or_insert_with(|| json!(unit));
                        }
                    }
                }
            }
        }
        self.execute(deps, &EntriesPost, presented, request_id, input)
            .await
    }

    /// `entries.transfer` with the unit pin applied: a pinned key recovers
    /// costs only for its own unit, so the paying unit must be the pinned one.
    pub(super) async fn entries_transfer(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        if let Some((key_id, unit)) = self
            .pinned_unit(deps, presented.as_ref(), request_id)
            .await?
        {
            let from = input
                .get("from_unit")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if from != unit {
                let reason = format!(
                    "agent key {key_id} is pinned to unit {unit}; it may recover costs only for {unit}"
                );
                return Err(self
                    .pinned_refusal(
                        deps,
                        request_id,
                        &key_id,
                        "entries.transfer",
                        &input,
                        reason,
                    )
                    .await);
            }
        }
        self.execute(deps, &EntriesTransfer, presented, request_id, input)
            .await
    }

    /// The unit an agent key is pinned to, when the caller is such a key.
    async fn pinned_unit(
        &self,
        deps: &Deps,
        presented: Option<&Presented>,
        request_id: &str,
    ) -> Result<Option<(String, String)>, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let principal = match presented {
            Some(Presented::Bearer(token)) => KeyStore {
                db: Arc::clone(&deps.db),
                clock: Arc::clone(&deps.clock),
                company_id: self.company_id.clone(),
                preloaded: None,
            }
            .authenticate(token)
            .await
            .map_err(fail)?,
            Some(Presented::Principal(principal)) => principal.clone(),
            Some(Presented::Session(_)) | None => return Ok(None),
        };
        let Principal::Agent { key_id, .. } = principal else {
            return Ok(None);
        };
        let key = load_key(deps.db.as_ref(), &self.company_id, &key_id)
            .await
            .map_err(fail)?;
        Ok(key.and_then(|key| key.unit).map(|unit| (key_id, unit)))
    }

    /// Audits a pin refusal as `denied` and answers 403.
    async fn pinned_refusal(
        &self,
        deps: &Deps,
        request_id: &str,
        key_id: &str,
        command: &str,
        input: &Value,
        reason: String,
    ) -> Problem {
        let at = rfc3339_utc(deps.clock.now());
        let actor = ledgers_audit::Actor::Agent {
            key_id: key_id.to_owned(),
            tier: Tier::PostWithinLimits.to_string(),
        };
        let audit = AuditContext::new(actor, request_id.to_owned(), deps.id_gen.ulid());
        let record =
            AuditRecord::from_input(command, input, Outcome::Denied, Vec::new(), Some(reason));
        let recorded = ledgers_audit::record(
            deps.db.as_ref(),
            &AuditConfig::default(),
            &self.company_id,
            &audit,
            &record,
            &at,
            ledgers_db::RetryPolicy::default(),
            |_| {},
        )
        .await;
        match recorded {
            Ok(_) => CommandError::PolicyDenied.to_problem().instance(request_id),
            Err(err) => CommandError::Database(cratefield_core::DbError::Query(err.to_string()))
                .to_problem()
                .instance(request_id),
        }
    }

    /// Authenticates the caller for a person-only unit write. An agent key
    /// is audited as `denied` and answered with 403 (`human-only`).
    async fn person_actor(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: &Value,
        at: &str,
    ) -> Result<ledgers_audit::Actor, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let principal = match presented
            .ok_or(CommandError::Unauthenticated)
            .map_err(fail)?
        {
            Presented::Bearer(token) => KeyStore {
                db: Arc::clone(&deps.db),
                clock: Arc::clone(&deps.clock),
                company_id: self.company_id.clone(),
                preloaded: None,
            }
            .authenticate(&token)
            .await
            .map_err(fail)?,
            Presented::Session(session) => {
                let id = session.id.clone();
                SessionAuthenticator {
                    db: Arc::clone(&deps.db),
                    clock: Arc::clone(&deps.clock),
                    app_origins: self.app_origins.clone(),
                    presented: session,
                }
                .authenticate(&id)
                .await
                .map_err(fail)?
            }
            Presented::Principal(principal) => principal,
        };
        match principal {
            Principal::Person { person_id } => Ok(ledgers_audit::Actor::Person { person_id }),
            Principal::Agent { key_id, tier, .. } => {
                let actor = ledgers_audit::Actor::Agent {
                    key_id: key_id.clone(),
                    tier: tier.to_string(),
                };
                let reason = format!("{command} changes business units; only a person may");
                let audit = AuditContext::new(actor, request_id.to_owned(), deps.id_gen.ulid());
                let record = AuditRecord::from_input(
                    command,
                    input,
                    Outcome::Denied,
                    Vec::new(),
                    Some(reason),
                );
                ledgers_audit::record(
                    deps.db.as_ref(),
                    &AuditConfig::default(),
                    &self.company_id,
                    &audit,
                    &record,
                    at,
                    ledgers_db::RetryPolicy::default(),
                    |_| {},
                )
                .await
                .map_err(|err| {
                    fail(CommandError::Database(cratefield_core::DbError::Query(
                        err.to_string(),
                    )))
                })?;
                Err(fail(CommandError::HumanOnly))
            }
        }
    }
}
