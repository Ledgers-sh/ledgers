//! The server's [`Dispatch`]: every registry command, run through the
//! command executor against the harness database.
//!
//! Each command here is a thin adapter: it reads the input, resolves
//! defaults, and calls the domain crate that owns the rule (ledgers-ledger
//! for entries, reversals, accounts and reports; ledgers-policy for agent
//! keys; ledgers-audit for the audit row the executor appends). The
//! executor authenticates the credential, checks the tier, asks the policy,
//! and writes the audit event in the same guard batch as the change
//! (docs/PLAN.md sections 3 and 5).

mod accounts;
mod approvals;
mod bank;
mod bills;
mod settings;
mod settings_rules;
mod units;

use crate::people::SessionAuthenticator;
use crate::routes::{Dispatch, Presented};
use accounts::AccountsGet;
use approvals::{ApprovalsGet, ApprovalsList};
use async_trait::async_trait;
use bills::PortTransport;
use cratefield_core::{
    ApiKeys, Blob, Clock, Database, DbError, HttpClient, IdGen, Port, Ports, Problem, RateLimiter,
    Statement,
};
use ledgers_approvals::{
    ApprovalEvent, ApprovalRequest, NewRequest, notice_statement, request_statement,
};
use ledgers_audit::{Actor, AuditContext};
use ledgers_auth::AgentKeys;
use ledgers_bills::commands::{
    BillsConfirm, BillsCreate, BillsDiscard, BillsGet, BillsList, BillsPost, BillsUpdate,
};
use ledgers_bills::parties::{
    HttpVies, PartiesCheckVat, PartiesCreate, PartiesList, PartiesMerge, PartiesSetBank,
    PartiesUpdate, ViesPort,
};
use ledgers_commands::auth::{Authenticator, Credential, Limit, Limits, Principal, Tier};
use ledgers_commands::{
    Command, CommandContext, CommandError, ExecToken, Executed, Executor, JournalPreview,
    PendingApproval, Plan, Registry,
};
use ledgers_documents::commands::{DocumentsGet, DocumentsList, DocumentsRegister};
use ledgers_extract::provider::ModelSettings;
use ledgers_ledger::journal::{JournalDraft, JournalKind};
use ledgers_ledger::query::{self, EntryRecord};
use ledgers_ledger::report;
use ledgers_ledger::reversal::ReverseContext;
use ledgers_ledger::surface::SerializableAmount;
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, PostEntryOutput, SerializableEntryDraft, SerializablePostedEntry,
};
use ledgers_policy::KeyPolicy;
use ledgers_policy::key::{AgentKey, CurrencyLimit};
use ledgers_policy::store::{deserialize_key, parse_keys, serialize_key};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use settings::{
    ApprovalSettingsGet, ApprovalSettingsUpdate, AuditList, CompanyGet, CompanyUpdate,
    FiscalYearsList, NumberingList, NumberingSetNext, TaxSettingsGet,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use time::format_description::well_known::Iso8601;
use time::{Date, OffsetDateTime, UtcOffset};
use units::{ReportsUnitsSummary, UnitsList, resolve_scope};

/// The company the development bootstrap creates when `LEDGERS_COMPANY_ID`
/// is unset. Invented fixture id, never a real company.
pub const DEV_COMPANY_ID: &str = "01DEVCOMPANY0000000000000A";
/// The id of the dev bootstrap's general journal (code `GN`). The web app's
/// journal field defaults to this id.
pub const DEV_JOURNAL_ID: &str = "general";
/// The owner recorded on dev bootstrap audit events and dev keys.
const DEV_OWNER: &str = "dev-owner";
/// The largest integer a D1 bind can take: 2^53 - 1, the JavaScript safe
/// integer. The Cloudflare runtime binds an `i64` through `serde_wasm_bindgen`,
/// which refuses anything outside this range ("wasm-bindgen serialization
/// error"), so a query that binds `i64::MAX` fails on Workers while the same
/// query runs under SQLite. Bind a value from this range or leave the clause out.
pub const D1_SAFE_INTEGER_MAX: i64 = (1 << 53) - 1;
/// The table the harness `ApiKeys` store keeps key hashes in.
pub const API_KEYS_TABLE: &str = "api_keys";
/// Chart tags whose accounts only a person may move money through: an
/// agent posting to one of them needs approval (docs/PLAN.md section 5).
const PROTECTED_TAGS: &[&str] = &[
    "cash",
    "bank",
    "ar",
    "ap",
    "vat-input",
    "vat-output",
    "suspense",
    "retained-earnings",
    "fx-gain",
    "fx-loss",
    "rounding",
];
/// The input of `documents.download_url`: the document id, from the path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DocumentUrlInput {
    /// The document id.
    pub id: String,
}

/// The output of `documents.download_url`: a relative link and its expiry.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DocumentUrlOutput {
    /// The link, relative to the API origin.
    pub url: String,
    /// When the link stops working (Unix seconds).
    pub expires_at: i64,
}

/// Rows a list query returns when the caller names no limit, and the cap.
const DEFAULT_LIST_LIMIT: u32 = 50;
const MAX_LIST_LIMIT: u32 = 200;

/// The harness ports the dispatch needs, taken from a module's context.
#[derive(Clone)]
pub struct Deps {
    /// The harness database every command reads and writes.
    pub db: Arc<dyn Database>,
    /// The clock: command timestamps and key expiry.
    pub clock: Arc<dyn Clock>,
    /// Ids for audit events, entries and keys.
    pub id_gen: Arc<dyn IdGen>,
    /// The rate limiter for agent keys, when the runtime provides one.
    pub rate_limiter: Option<Arc<dyn RateLimiter>>,
    /// The blob store the original documents are kept in.
    pub blob: Option<Arc<dyn Blob>>,
    /// The HTTP client the model extractor calls the Messages API with.
    pub http: Option<Arc<dyn HttpClient>>,
}

impl Deps {
    /// Takes the dependencies from a port set; `None` when the database,
    /// clock or id generator is missing.
    #[must_use]
    pub fn from_ports(ports: &Ports) -> Option<Self> {
        Some(Deps {
            db: ports.db.clone()?,
            clock: ports.clock.clone()?,
            id_gen: ports.id_gen.clone()?,
            rate_limiter: ports.rate_limiter.clone(),
            blob: ports.blob.clone(),
            http: ports.http.clone(),
        })
    }
}

/// The ports a dispatch-serving module must declare.
pub const DISPATCH_PORTS: &[Port] = &[Port::Db, Port::Clock, Port::IdGen];

/// The server's command dispatch for one company.
pub struct ServerDispatch {
    company_id: String,
    /// The deployment's own company (`LEDGERS_COMPANY_ID`): the one whose
    /// owner an `owner` sign-in claim may bootstrap.
    home_company: String,
    app_origins: Vec<String>,
    app_url: String,
    /// The key that signs document download links; empty until configured,
    /// and then download links are refused.
    document_key: Vec<u8>,
    /// The model reader the configuration selected, when one is configured.
    model: Option<ModelSettings>,
    /// Whether `parties.check_vat` may call VIES (`LEDGERS_VIES=1`). Off by
    /// default: no VAT number leaves the server unless an operator turns it on.
    vies_enabled: bool,
}

impl ServerDispatch {
    /// A dispatch serving `company_id`. Cookie sessions are refused unless
    /// [`ServerDispatch::with_app_origins`] names the origins they may come from.
    #[must_use]
    pub fn new(company_id: impl Into<String>) -> Self {
        let company_id = company_id.into();
        Self {
            home_company: company_id.clone(),
            company_id,
            app_origins: Vec::new(),
            app_url: String::new(),
            document_key: Vec::new(),
            model: None,
            vies_enabled: false,
        }
    }

    /// Whether this dispatch serves the deployment's own company.
    #[must_use]
    pub fn is_home(&self) -> bool {
        self.company_id == self.home_company
    }

    /// The key that signs document download links.
    #[must_use]
    pub fn with_document_key(mut self, key: Vec<u8>) -> Self {
        self.document_key = key;
        self
    }

    /// The model reader for documents (its key is never logged or shown).
    #[must_use]
    pub fn with_model_reader(mut self, settings: Option<ModelSettings>) -> Self {
        self.model = settings;
        self
    }

    /// Turns VIES VAT checks on or off (`LEDGERS_VIES=1` turns them on).
    #[must_use]
    pub fn with_vies_enabled(mut self, enabled: bool) -> Self {
        self.vies_enabled = enabled;
        self
    }

    /// The app's absolute URL: approval links point at `{app_url}/approvals/{id}`.
    #[must_use]
    pub fn with_app_url(mut self, app_url: impl Into<String>) -> Self {
        self.app_url = app_url.into();
        self
    }

    /// The app origins a cookie-authenticated request may come from.
    #[must_use]
    pub fn with_app_origins(mut self, origins: Vec<String>) -> Self {
        self.app_origins = origins;
        self
    }

    /// The company this dispatch serves.
    #[must_use]
    pub fn company_id(&self) -> &str {
        &self.company_id
    }

    /// Runs one command through the executor and maps its outcome to JSON.
    #[allow(clippy::too_many_lines)]
    async fn execute<C: Command>(
        &self,
        deps: &Deps,
        cmd: &C,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let presented = presented.ok_or_else(|| {
            CommandError::Unauthenticated
                .to_problem()
                .instance(request_id)
        })?;

        let now_at = deps.clock.now();
        let ctx = CommandContext {
            company_id: self.company_id.clone(),
            request_id: request_id.to_owned(),
            event_id: deps.id_gen.ulid(),
            now: rfc3339_utc(now_at),
        };
        let now = ctx.now.clone();
        // Reads never touch protected accounts (their previews are empty), so
        // only a command that can move money loads the protected-account set.
        let policy = load_policy_with(
            deps.db.as_ref(),
            &self.company_id,
            now_at,
            C::TIER != Tier::Read,
        )
        .await
        .map_err(fail)?;
        let policy = match &presented {
            Presented::Principal(principal) => internal_agent_policy(policy, principal, now_at),
            Presented::Bearer(_) | Presented::Session(_) => policy,
        };
        let (authenticator, credential) = match presented {
            Presented::Bearer(token) => (
                Authn::Key(KeyStore {
                    db: Arc::clone(&deps.db),
                    clock: Arc::clone(&deps.clock),
                    company_id: self.company_id.clone(),
                    // The policy above already read every agent key of the
                    // company, so the key is not read a second time.
                    preloaded: Some(policy.agent_keys().clone()),
                }),
                token,
            ),
            Presented::Principal(principal) => (Authn::Fixed(principal), String::new()),
            Presented::Session(session) => {
                let id = session.id.clone();
                (
                    Authn::Session(SessionAuthenticator {
                        db: Arc::clone(&deps.db),
                        clock: Arc::clone(&deps.clock),
                        app_origins: self.app_origins.clone(),
                        bootstrap: self.is_home(),
                        company_id: self.company_id.clone(),
                        presented: session,
                    }),
                    id,
                )
            }
        };
        let input: C::Input = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;

        // A needs-approval outcome stores its request in the same guarded batch
        // as its audit row (docs/PLAN.md section 5).
        let rules = settings::approval_rules_for::<C>(deps.db.as_ref(), &self.company_id)
            .await
            .map_err(|err| fail(CommandError::Database(err)))?;
        let ttl = Duration::from_hours(u64::from(rules.ttl_hours));
        let approval_id = deps.id_gen.ulid();
        let raised: Mutex<Option<ApprovalRequest>> = Mutex::new(None);
        let persist = |pending: &PendingApproval| -> Result<Vec<Statement>, CommandError> {
            let preview = serde_json::to_value(&pending.preview)
                .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
            let (statement, request) = request_statement(&NewRequest {
                id: &approval_id,
                company_id: &self.company_id,
                command: &pending.command,
                input: &pending.input,
                preview: &preview,
                reads: &pending.reads,
                request_hash: &pending.request_hash,
                reason: &pending.reason,
                requested_by: &pending.requested_by,
                counterparty: None,
                now: &now,
                ttl,
                low_risk_limits: &rules.low_risk,
            })
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
            // The Created notice is enqueued in the same batch as the request.
            let notice = notice_statement(
                &approval_id,
                ApprovalEvent::Created,
                &request,
                request.counterparty.as_deref(),
                &self.app_url,
                &now,
            )
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
            *raised.lock().unwrap_or_else(PoisonError::into_inner) = Some(request);
            Ok(vec![statement, notice])
        };

        let executor = Executor::new(deps.db.as_ref(), authenticator, policy);
        match executor
            .run_for_approval(cmd, credential, ctx, &input, &persist)
            .await
            .map_err(fail)?
        {
            Executed::Done(output) => serde_json::to_value(output)
                .map_err(|err| fail(CommandError::InvalidInput(err.to_string()))),
            Executed::NeedsApproval {
                request_hash,
                preview,
                reason,
                reads,
            } => {
                let stored = raised
                    .into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .ok_or_else(|| {
                        fail(CommandError::Database(DbError::Query(
                            "the approval request was not stored".to_owned(),
                        )))
                    })?;
                Ok(json!({
                    "status": "needs-approval",
                    "reason": reason,
                    "request_hash": request_hash,
                    "preview": preview,
                    "reads": reads,
                    "approval": {
                        "id": approval_id,
                        "url": approvals::approval_url(&self.app_url, &approval_id),
                        "status": stored.status.as_str(),
                        "risk": stored.risk.as_str(),
                        "expires_at": stored.expires_at,
                    },
                }))
            }
            Executed::DryRun(preview) => Ok(json!({ "status": "dry-run", "preview": preview })),
        }
    }
}

impl ServerDispatch {
    /// The person behind a human-only command. An agent key answers 403
    /// (`human-only`), an unknown or revoked credential 401, and a session
    /// goes through the same origin, role and expiry checks as every command.
    /// The signed-in person a route acts for. With a `command`, the company's
    /// role matrix must let the person's role run it (docs/permissions.md);
    /// without one the call only identifies the person. An agent key is refused
    /// with `human-only`; a role the matrix refuses gets `policy-denied`.
    async fn person_id(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: Option<&str>,
    ) -> Result<String, Problem> {
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
                    bootstrap: self.is_home(),
                    company_id: self.company_id.clone(),
                    presented: session,
                }
                .authenticate(&id)
                .await
                .map_err(fail)?
            }
            Presented::Principal(principal) => principal,
        };
        match principal {
            Principal::Person { person_id, role } => {
                if let Some(command) = command {
                    ledgers_policy::roles::check(role, command)
                        .map_err(|_| fail(CommandError::PolicyDenied))?;
                }
                Ok(person_id)
            }
            Principal::Agent { .. } => Err(fail(CommandError::HumanOnly)),
        }
    }

    async fn keys_list(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        self.person_id(deps, presented, request_id, Some("keys.list"))
            .await?;
        let keys = crate::agent_keys::list(deps, &self.company_id)
            .await
            .map_err(fail)?;
        serde_json::to_value(crate::agent_keys::KeysListOutput { keys })
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    async fn keys_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("keys.create"))
            .await?;
        let input: crate::agent_keys::KeysCreateInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let output = crate::agent_keys::create(deps, &self.company_id, &person, &input)
            .await
            .map_err(fail)?;
        serde_json::to_value(output)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }

    async fn keys_revoke(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: CommandError| err.to_problem().instance(request_id);
        let person = self
            .person_id(deps, presented, request_id, Some("keys.revoke"))
            .await?;
        let input: crate::agent_keys::KeysRevokeInput = serde_json::from_value(input)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))?;
        let output = crate::agent_keys::revoke(deps, &self.company_id, &person, &input.id)
            .await
            .map_err(fail)?;
        serde_json::to_value(output)
            .map_err(|err| fail(CommandError::InvalidInput(err.to_string())))
    }
}

impl ServerDispatch {
    /// `documents.register`: the server fills the id and the uploader from the
    /// credential; the caller cannot choose either.
    async fn documents_register(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        fill_server_fields(&mut input, deps, "id", &caller);
        self.execute(deps, &DocumentsRegister, presented, request_id, input)
            .await
    }

    /// `bills.create`: the server fills the id and the author from the credential.
    async fn bills_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let caller = self
            .caller_label(deps, presented.as_ref(), request_id)
            .await?;
        fill_server_fields(&mut input, deps, "id", &caller);
        self.execute(deps, &BillsCreate, presented, request_id, input)
            .await
    }

    /// `bills.confirm`: a person only. An agent gets 403 (`human-only`).
    async fn bills_confirm(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        self.person_id(deps, presented.clone(), request_id, Some("bills.confirm"))
            .await?;
        self.execute(deps, &BillsConfirm, presented, request_id, input)
            .await
    }

    /// `parties.set_bank`: a person only. An agent gets 403 (`human-only`).
    async fn parties_set_bank(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(
                deps,
                presented.clone(),
                request_id,
                Some("parties.set_bank"),
            )
            .await?;
        let cmd = PartiesSetBank::new(Some(person));
        self.execute(deps, &cmd, presented, request_id, input).await
    }

    /// `parties.check_vat`: agent allowed. The VIES port exists only when the
    /// server enabled VAT checks and the runtime has an HTTP client.
    async fn parties_check_vat(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let vies: Option<Arc<dyn ViesPort>> = match (self.vies_enabled, &deps.http) {
            (true, Some(http)) => Some(Arc::new(HttpVies::new(Arc::new(PortTransport(
                http.clone(),
            ))))),
            _ => None,
        };
        let cmd = PartiesCheckVat::new(vies);
        self.execute(deps, &cmd, presented, request_id, input).await
    }

    /// `parties.merge`: a person only. An agent gets 403 (`human-only`).
    async fn parties_merge(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, Some("parties.merge"))
            .await?;
        let cmd = PartiesMerge::new(Some(person));
        self.execute(deps, &cmd, presented, request_id, input).await
    }
}

/// Sets the id (when the caller left it empty) and the caller label on a
/// command input. Values the caller sent for these are overwritten.
fn fill_server_fields(input: &mut Value, deps: &Deps, id_field: &str, caller: &str) {
    if let Value::Object(map) = input {
        let empty = map
            .get(id_field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if empty {
            map.insert(id_field.to_owned(), Value::String(deps.id_gen.ulid()));
        }
        let label_field = if map.contains_key("uploaded_by") {
            "uploaded_by"
        } else {
            "created_by"
        };
        map.insert(label_field.to_owned(), Value::String(caller.to_owned()));
    }
}

#[async_trait]
impl Dispatch for ServerDispatch {
    async fn key_company(&self, deps: &Deps, credential: &str) -> Option<String> {
        key_company(deps, credential).await
    }

    fn scoped(self: Arc<Self>, company_id: &str) -> Arc<dyn Dispatch> {
        Arc::new(ServerDispatch {
            company_id: company_id.to_owned(),
            home_company: self.home_company.clone(),
            app_origins: self.app_origins.clone(),
            app_url: self.app_url.clone(),
            document_key: self.document_key.clone(),
            model: self.model.clone(),
            vies_enabled: self.vies_enabled,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(
        &self,
        deps: &Deps,
        command: &str,
        input: Value,
        request_id: &str,
        presented: Option<Presented>,
    ) -> Result<Value, Problem> {
        match command {
            "entries.post" => self.entries_post(deps, presented, request_id, input).await,
            "entries.transfer" => {
                self.entries_transfer(deps, presented, request_id, input)
                    .await
            }
            "units.list" => {
                self.execute(deps, &UnitsList, presented, request_id, input)
                    .await
            }
            "units.create" | "units.rename" | "units.archive" => {
                self.units_write(deps, presented, request_id, command, input)
                    .await
            }
            "reports.units_summary" => {
                self.execute(deps, &ReportsUnitsSummary, presented, request_id, input)
                    .await
            }
            "entries.reverse" => {
                self.execute(deps, &EntriesReverse, presented, request_id, input)
                    .await
            }
            "entries.list" => {
                self.execute(deps, &EntriesList, presented, request_id, input)
                    .await
            }
            "approvals.list" => {
                self.execute(deps, &ApprovalsList, presented, request_id, input)
                    .await
            }
            "approvals.get" => {
                self.execute(deps, &ApprovalsGet, presented, request_id, input)
                    .await
            }
            "approvals.decide" => {
                self.approvals_decide(deps, presented, request_id, input)
                    .await
            }
            "approvals.decide_bulk" => {
                self.approvals_decide_bulk(deps, presented, request_id, input)
                    .await
            }
            "entries.get" => {
                self.execute(deps, &EntriesGet, presented, request_id, input)
                    .await
            }
            "accounts.list" => {
                self.execute(deps, &AccountsList, presented, request_id, input)
                    .await
            }
            "accounts.get" => {
                self.execute(deps, &AccountsGet, presented, request_id, input)
                    .await
            }
            "accounts.create" | "accounts.update" | "accounts.archive" | "accounts.restore" => {
                self.accounts_write(deps, presented, request_id, command, input)
                    .await
            }
            "reports.trial_balance" => {
                self.execute(deps, &ReportsTrialBalance, presented, request_id, input)
                    .await
            }
            "reports.trends" => {
                self.execute(deps, &ReportsTrends, presented, request_id, input)
                    .await
            }
            "reports.profit_and_loss" => {
                self.execute(deps, &ReportsProfitAndLoss, presented, request_id, input)
                    .await
            }
            "reports.balance_sheet" => {
                self.execute(deps, &ReportsBalanceSheet, presented, request_id, input)
                    .await
            }
            "reports.account_balance_by_dimension" => {
                self.execute(
                    deps,
                    &ReportsAccountBalanceByDimension,
                    presented,
                    request_id,
                    input,
                )
                .await
            }
            "documents.register" => {
                self.documents_register(deps, presented, request_id, input)
                    .await
            }
            "documents.list" => {
                self.execute(deps, &DocumentsList, presented, request_id, input)
                    .await
            }
            "documents.get" => {
                self.execute(deps, &DocumentsGet, presented, request_id, input)
                    .await
            }
            "documents.download_url" => {
                self.documents_download_url(deps, presented, request_id, input)
                    .await
            }
            "bills.extract" => self.bills_extract(deps, presented, request_id, input).await,
            "bills.create" => self.bills_create(deps, presented, request_id, input).await,
            "bills.list" => {
                self.execute(deps, &BillsList, presented, request_id, input)
                    .await
            }
            "bills.get" => {
                self.execute(deps, &BillsGet, presented, request_id, input)
                    .await
            }
            "bills.update" => {
                self.execute(deps, &BillsUpdate, presented, request_id, input)
                    .await
            }
            "bills.post" => {
                self.execute(deps, &BillsPost, presented, request_id, input)
                    .await
            }
            "bills.confirm" => self.bills_confirm(deps, presented, request_id, input).await,
            "bills.discard" => {
                self.execute(deps, &BillsDiscard, presented, request_id, input)
                    .await
            }
            "parties.list" => {
                self.execute(deps, &PartiesList, presented, request_id, input)
                    .await
            }
            "parties.create" => {
                self.execute(deps, &PartiesCreate, presented, request_id, input)
                    .await
            }
            "parties.update" => {
                self.execute(deps, &PartiesUpdate, presented, request_id, input)
                    .await
            }
            "parties.set_bank" => {
                self.parties_set_bank(deps, presented, request_id, input)
                    .await
            }
            "parties.check_vat" => {
                self.parties_check_vat(deps, presented, request_id, input)
                    .await
            }
            "parties.merge" => self.parties_merge(deps, presented, request_id, input).await,
            "bank.accounts.list"
            | "bank.accounts.get"
            | "bank.accounts.reconcile"
            | "bank.accounts.create"
            | "bank.accounts.update"
            | "bank.accounts.archive"
            | "bank.statements.import"
            | "bank.transactions.list"
            | "bank.transactions.post"
            | "bank.transactions.link"
            | "bank.transactions.ignore"
            | "bank.rules.list"
            | "bank.rules.create"
            | "bank.rules.enable" => {
                self.bank_command(deps, presented, request_id, command, input)
                    .await
            }
            "keys.list" => self.keys_list(deps, presented, request_id).await,
            "keys.create" => self.keys_create(deps, presented, request_id, input).await,
            "keys.revoke" => self.keys_revoke(deps, presented, request_id, input).await,
            "company.get" => {
                self.execute(deps, &CompanyGet, presented, request_id, input)
                    .await
            }
            "company.update" => {
                self.execute(deps, &CompanyUpdate, presented, request_id, input)
                    .await
            }
            "fiscal_years.list" => {
                self.execute(deps, &FiscalYearsList, presented, request_id, input)
                    .await
            }
            "fiscal_years.create" => {
                self.fiscal_years_create(deps, presented, request_id, input)
                    .await
            }
            "fiscal_years.close" => {
                self.fiscal_years_close(deps, presented, request_id, input)
                    .await
            }
            "periods.close" => self.periods_close(deps, presented, request_id, input).await,
            "periods.reopen" => {
                self.periods_reopen(deps, presented, request_id, input)
                    .await
            }
            "numbering.list" => {
                self.execute(deps, &NumberingList, presented, request_id, input)
                    .await
            }
            "numbering.set_next" => {
                self.execute(deps, &NumberingSetNext, presented, request_id, input)
                    .await
            }
            "tax_settings.get" => {
                self.execute(deps, &TaxSettingsGet, presented, request_id, input)
                    .await
            }
            "tax_settings.update" => {
                self.tax_settings_update(deps, presented, request_id, input)
                    .await
            }
            "approval_settings.get" => {
                self.execute(deps, &ApprovalSettingsGet, presented, request_id, input)
                    .await
            }
            "approval_settings.update" => {
                self.execute(deps, &ApprovalSettingsUpdate, presented, request_id, input)
                    .await
            }
            "audit.list" => {
                self.execute(deps, &AuditList, presented, request_id, input)
                    .await
            }
            other => Err(
                Problem::validation_failed(format!("unknown command {other:?}"))
                    .instance(request_id),
            ),
        }
    }

    async fn verified_key_id(&self, deps: &Deps, credential: &str) -> Option<String> {
        // The same verify the command path runs, without the policy read:
        // a revoked or unknown credential verifies to nothing.
        api_keys(&deps.db, &deps.clock)
            .verify(credential)
            .await
            .ok()
            .flatten()
            .map(|verified| verified.subject)
    }

    async fn principal(&self, deps: &Deps, credential: &str) -> Result<Principal, Problem> {
        let authenticator = KeyStore {
            db: Arc::clone(&deps.db),
            clock: Arc::clone(&deps.clock),
            company_id: self.company_id.clone(),
            preloaded: None,
        };
        authenticator
            .authenticate(&credential.to_owned())
            .await
            .map_err(|err| err.to_problem())
    }
}

/// Registers the policy row of an in-process agent principal. The principal is
/// not in the `agent_key` table, so without this its entries would be denied as
/// an unknown key. The row takes the principal's own tier and per-entry
/// ceilings; the day's ceiling is the same figure, so the daily meter never
/// binds before the per-entry one. A person principal needs no row.
fn internal_agent_policy(
    policy: KeyPolicy,
    principal: &Principal,
    now: OffsetDateTime,
) -> KeyPolicy {
    let Principal::Agent {
        key_id,
        tier,
        limits,
    } = principal
    else {
        return policy;
    };
    let limits = limits
        .iter()
        .map(|(code, limit)| {
            (
                code.clone(),
                CurrencyLimit {
                    per_entry: limit.base_ceiling,
                    per_day: limit.base_ceiling,
                },
            )
        })
        .collect();
    policy.with_key(AgentKey {
        id: key_id.clone(),
        label: "in-process".to_owned(),
        owner: "server".to_owned(),
        tier: *tier,
        limits,
        allowed_journals: None,
        allowed_accounts: None,
        expires_at: None,
        created_at: now,
        last_used_at: None,
        revoked_at: None,
        chart_changes: false,
        unit: None,
    })
}

/// The authenticator a command runs under: an agent key, a person session, or
/// a principal the server vouches for in-process (see [`Presented::Principal`]).
enum Authn {
    Key(KeyStore),
    Session(SessionAuthenticator),
    Fixed(Principal),
}

#[async_trait]
impl Authenticator for Authn {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        match self {
            Authn::Key(store) => store.authenticate(credential).await,
            Authn::Session(session) => session.authenticate(credential).await,
            Authn::Fixed(principal) => Ok(principal.clone()),
        }
    }
}

/// Authenticates a bearer credential as an agent key: the harness `ApiKeys`
/// verifies the secret (no caching, so a revoked key fails at once), and the
/// key's policy row in `agent_key` gives its tier and limits.
pub(crate) struct KeyStore {
    pub(crate) db: Arc<dyn Database>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) company_id: String,
    /// The company's agent keys when the caller already loaded them (the
    /// command path loads the policy first); `None` reads the key row.
    pub(crate) preloaded: Option<BTreeMap<String, AgentKey>>,
}

#[async_trait]
impl Authenticator for KeyStore {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        let verified = api_keys(&self.db, &self.clock)
            .verify(credential)
            .await?
            .ok_or(CommandError::Unauthenticated)?;
        let key = match &self.preloaded {
            Some(keys) => keys.get(&verified.subject).cloned(),
            None => load_key(self.db.as_ref(), &self.company_id, &verified.subject).await?,
        }
        .ok_or(CommandError::Unauthenticated)?;
        let now = self.clock.now();
        if key.revoked_at.is_some_and(|at| at <= now) || key.expires_at.is_some_and(|at| at <= now)
        {
            return Err(CommandError::Unauthenticated);
        }
        Ok(Principal::Agent {
            key_id: verified.subject,
            tier: key.tier,
            limits: principal_limits(&key)?,
        })
    }
}

/// The company an agent key belongs to, found by the key's id. The key's secret
/// is checked against the deployment-wide key table; the company whose
/// `agent_key` row carries the verified id is the one the key acts for. `None`
/// for a credential that verifies to no key of any company.
pub(crate) async fn key_company(deps: &Deps, credential: &str) -> Option<String> {
    let verified = api_keys(&deps.db, &deps.clock)
        .verify(credential)
        .await
        .ok()??;
    let rows = deps
        .db
        .query(&Statement::with_values(
            "SELECT company_id FROM agent_key WHERE id = ?".to_owned(),
            vec![sea_query::Value::from(verified.subject)],
        ))
        .await
        .ok()?;
    rows.rows.first()?.get("company_id")
}

/// The harness API-key store on the Ledgers table, with the OS entropy
/// source.
pub(crate) fn api_keys(db: &Arc<dyn Database>, clock: &Arc<dyn Clock>) -> ApiKeys {
    ApiKeys::new(
        Arc::clone(db),
        Arc::clone(clock),
        Arc::new(OsRandom),
        API_KEYS_TABLE,
    )
}

/// Entropy for key ids and secrets: the operating system's source.
struct OsRandom;

impl cratefield_core::RandomBytes for OsRandom {
    fn fill(&self, dest: &mut [u8]) -> Result<(), cratefield_core::RandomError> {
        // The OS source is the only one the key format allows; a host without
        // one cannot mint or verify keys safely, so it stops here.
        getrandom::fill(dest).expect("the operating system entropy source is available");
        Ok(())
    }
}

pub(crate) fn principal_limits(key: &AgentKey) -> Result<Limits, CommandError> {
    key.limits
        .iter()
        .try_fold(Limits::new(), |limits, (code, limit)| {
            limits
                .insert(
                    code,
                    Limit {
                        base_ceiling: limit.per_entry,
                        transaction_ceiling: limit.per_entry,
                    },
                )
                .map_err(CommandError::InvalidInput)
        })
}

/// Loads one agent key's policy row for `company_id`.
pub(crate) async fn load_key(
    db: &dyn Database,
    company_id: &str,
    key_id: &str,
) -> Result<Option<AgentKey>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT policy FROM agent_key WHERE company_id = ? AND id = ?".to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(key_id),
            ],
        ))
        .await?;
    let Some(json) = rows.first().and_then(|row| row.get::<String>("policy")) else {
        return Ok(None);
    };
    deserialize_key(&json)
        .map(Some)
        .map_err(|err| CommandError::InvalidInput(format!("agent key policy: {err}")))
}

/// The policy for one request: every agent key of the company, the chart's
/// protected accounts, and the clock for expiry and rolling days.
pub(crate) async fn load_policy(
    db: &dyn Database,
    company_id: &str,
    now: OffsetDateTime,
) -> Result<KeyPolicy, CommandError> {
    load_policy_with(db, company_id, now, true).await
}

/// [`load_policy`], with the chart's protected-account set read only when
/// `with_protected` is set. An empty set makes the protected-account rule a
/// no-op, so the caller must pass `false` only for a command whose preview
/// touches no accounts (every read-tier command's preview is empty).
pub(crate) async fn load_policy_with(
    db: &dyn Database,
    company_id: &str,
    now: OffsetDateTime,
    with_protected: bool,
) -> Result<KeyPolicy, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, policy FROM agent_key WHERE company_id = ?".to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await?;
    let pairs = rows
        .rows
        .iter()
        .filter_map(|row| Some((row.get::<String>("id")?, row.get::<String>("policy")?)))
        .collect::<Vec<_>>();
    let keys = parse_keys(pairs)
        .map_err(|err| CommandError::InvalidInput(format!("agent key policy: {err}")))?;

    let protected: BTreeSet<String> = if with_protected {
        let accounts = ledgers_ledger::commands::list_accounts(db, company_id, true)
            .await
            .map_err(|err| CommandError::Domain(err.to_problem()))?;
        accounts
            .iter()
            .filter(|entry| {
                entry
                    .account
                    .tags
                    .iter()
                    .any(|tag| PROTECTED_TAGS.contains(&tag.as_str()))
            })
            .map(|entry| entry.account.number.clone())
            .collect()
    } else {
        BTreeSet::new()
    };

    Ok(KeyPolicy::new(keys, protected, now, UtcOffset::UTC))
}

/// Issues an agent key of `tier` with per-currency `limits` for the company,
/// storing its policy row and minting its secret. The secret is returned once.
///
/// # Errors
/// A message naming the failed step (database, policy encoding, key store).
pub async fn issue_key(
    deps: &Deps,
    company_id: &str,
    label: &str,
    tier: Tier,
    limits: BTreeMap<String, CurrencyLimit>,
) -> Result<String, String> {
    issue_key_as(deps, company_id, label, DEV_OWNER, tier, limits, None)
        .await
        .map(|(_, token)| token)
}

/// [`issue_key`] with the owner recorded on the key and its audit events.
/// Returns the new key's id (the policy row's id) and its secret, shown once.
pub(crate) async fn issue_key_as(
    deps: &Deps,
    company_id: &str,
    label: &str,
    owner: &str,
    tier: Tier,
    limits: BTreeMap<String, CurrencyLimit>,
    unit: Option<String>,
) -> Result<(String, String), String> {
    let key_id = deps.id_gen.ulid();
    let policy =
        agent_key_policy_statement(deps, company_id, &key_id, label, owner, tier, limits, unit)?;
    deps.db
        .execute(&policy)
        .await
        .map_err(|err| format!("store key policy: {err}"))?;
    let token = mint_key(deps, company_id, &key_id, owner).await?;
    Ok((key_id, token))
}

/// The insert of an agent key's policy row, with the key's id and the time the
/// row is stamped with. Built here so the bootstrap can run it inside its guarded
/// batch; [`issue_key_as`] runs it on its own.
#[allow(clippy::too_many_arguments)]
fn agent_key_policy_statement(
    deps: &Deps,
    company_id: &str,
    key_id: &str,
    label: &str,
    owner: &str,
    tier: Tier,
    limits: BTreeMap<String, CurrencyLimit>,
    unit: Option<String>,
) -> Result<Statement, String> {
    let now = deps.clock.now();
    let key = AgentKey {
        id: key_id.to_owned(),
        label: label.to_owned(),
        owner: owner.to_owned(),
        tier,
        limits,
        allowed_journals: None,
        allowed_accounts: None,
        expires_at: None,
        created_at: now,
        last_used_at: None,
        revoked_at: None,
        chart_changes: false,
        unit,
    };
    let policy = serialize_key(&key).map_err(|err| format!("encode key policy: {err}"))?;
    Ok(Statement::with_values(
        "INSERT INTO agent_key (company_id, id, policy, updated_at) VALUES (?, ?, ?, ?)".to_owned(),
        vec![
            sea_query::Value::from(company_id),
            sea_query::Value::from(key_id),
            sea_query::Value::from(policy),
            sea_query::Value::from(rfc3339_utc(now)),
        ],
    ))
}

/// Mints the secret for an agent key whose policy row exists, and returns it.
/// The secret is written as the `ApiKeys` row and returned once.
pub(crate) async fn mint_key(
    deps: &Deps,
    company_id: &str,
    key_id: &str,
    owner: &str,
) -> Result<String, String> {
    let now = deps.clock.now();
    let audit = AuditContext::new(
        Actor::Person {
            person_id: owner.to_owned(),
        },
        deps.id_gen.ulid(),
        deps.id_gen.ulid(),
    );
    let agent_keys = AgentKeys::new(Arc::new(api_keys(&deps.db, &deps.clock)), deps.db.clone());
    let issued = agent_keys
        .issue(key_id, company_id, &audit, &rfc3339_utc(now))
        .await
        .map_err(|err| format!("issue key: {err}"))?;
    Ok(issued.key())
}

/// Seeds `company_id` when it has no chart yet: imports the generic SME
/// chart, opens the general journal (`general`, code `GN`) and the current
/// calendar year. Idempotent: a company that already has accounts is left
/// as it is.
/// Creates the company's chart (from the generic SME template), general journal,
/// and opens the fiscal year for the current year. If the company already has a
/// chart, nothing is done (idempotent by chart existence, not by company id).
///
/// # Errors
/// A message naming the failed step.
pub async fn seed_company(deps: &Deps, company_id: &str, owner: &str) -> Result<(), String> {
    seed_company_with_journal(deps, company_id, owner, DEV_JOURNAL_ID).await
}

/// [`seed_company`] with the general journal's id given. Journal ids are
/// global keys, so a second company in one database names its own.
///
/// # Errors
/// As [`seed_company`].
pub async fn seed_company_with_journal(
    deps: &Deps,
    company_id: &str,
    owner: &str,
    journal_id: &str,
) -> Result<(), String> {
    let template =
        ledgers_ledger::generic_sme_template().map_err(|err| format!("bundled chart: {err}"))?;
    seed_company_chart(deps, company_id, owner, journal_id, &template, 1).await
}

/// The chart a company's country pack starts from: the Dutch RGS chart for
/// `nl`, the German HGB chart for `de`, the generic SME chart otherwise.
///
/// # Errors
/// A message naming the bundled chart that failed to load.
pub fn chart_for_pack(pack: &str) -> Result<ledgers_ledger::ChartTemplate, String> {
    let template = match pack {
        "nl" => ledgers_ledger::nl_rgs_bv_template(),
        "de" => ledgers_ledger::de_hgb_template(),
        _ => ledgers_ledger::generic_sme_template(),
    };
    template.map_err(|err| format!("bundled chart: {err}"))
}

/// [`seed_company_with_journal`] for a chart and a fiscal year that starts in
/// `start_month` (1 to 12). The current calendar year is the first fiscal year.
///
/// # Errors
/// A message naming the failed step.
pub async fn seed_company_chart(
    deps: &Deps,
    company_id: &str,
    owner: &str,
    journal_id: &str,
    template: &ledgers_ledger::ChartTemplate,
    start_month: u8,
) -> Result<(), String> {
    let now = deps.clock.now();
    let stamp = rfc3339_utc(now);
    let db = deps.db.as_ref();
    let audit = AuditContext::new(
        Actor::Person {
            person_id: owner.to_owned(),
        },
        deps.id_gen.ulid(),
        deps.id_gen.ulid(),
    );

    let existing = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|err| format!("read chart: {err}"))?;
    if existing.is_empty() {
        ledgers_ledger::commands::import_template(db, company_id, template, stamp.clone(), &audit)
            .await
            .map_err(|err| format!("import chart: {err}"))?;
        ledgers_ledger::journal::create_journal(
            db,
            company_id,
            JournalDraft {
                code: "GN".to_owned(),
                kind: JournalKind::General,
                name: "General journal".to_owned(),
            },
            journal_id,
            &stamp,
            &audit,
        )
        .await
        .map_err(|err| format!("create journal: {err}"))?;
        ledgers_ledger::period::open_fiscal_year(
            db,
            company_id,
            ledgers_ledger::FiscalYearSpec {
                label: i64::from(now.year()),
                start_month: u32::from(start_month),
                has_adjustment_period: false,
            },
            stamp.clone(),
            &audit,
        )
        .await
        .map_err(|err| format!("open fiscal year: {err}"))?;
    }
    Ok(())
}

/// Per-currency limits of the bootstrap keys: the same generous ceilings the
/// development key uses, in minor units.
fn bootstrap_limits(currency: &str) -> BTreeMap<String, CurrencyLimit> {
    let mut limits = BTreeMap::new();
    limits.insert(
        currency.to_owned(),
        CurrencyLimit {
            per_entry: 100_000_000_000_000,
            per_day: 1_000_000_000_000_000,
        },
    );
    limits
}

/// The display name of the development company.
const DEV_COMPANY_NAME: &str = "Development company";

/// The base currency of the development company (and of the demo book).
pub const DEV_BASE_CURRENCY: &str = "EUR";

/// Creates the company row with its base currency when it does not exist yet.
/// An existing row is left as it is: the base currency is set once, at
/// creation, and never rewritten here.
///
/// # Errors
/// A message naming the failed step.
pub async fn ensure_company(
    deps: &Deps,
    company_id: &str,
    name: &str,
    currency: &str,
) -> Result<(), String> {
    let stamp = rfc3339_utc(deps.clock.now());
    deps.db
        .execute(&Statement::with_values(
            "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
             SELECT ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM company WHERE id = ?)"
                .to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(name),
                sea_query::Value::from(currency),
                sea_query::Value::from(stamp.as_str()),
                sea_query::Value::from(stamp.as_str()),
                sea_query::Value::from(company_id),
            ],
        ))
        .await
        .map(|_| ())
        .map_err(|err| format!("create company: {err}"))
}

/// Development bootstrap for `company_id`: when the company has no chart,
/// imports the generic SME chart, opens the general journal `GN` and the
/// current calendar year; then issues one local key with full tier
/// (post within limits) and returns its secret.
///
/// # Errors
/// A message naming the failed step.
pub async fn bootstrap_dev(deps: &Deps, company_id: &str) -> Result<String, String> {
    ensure_company(deps, company_id, DEV_COMPANY_NAME, DEV_BASE_CURRENCY).await?;
    seed_company(deps, company_id, DEV_OWNER).await?;
    issue_key(
        deps,
        company_id,
        "dev local key",
        Tier::PostWithinLimits,
        bootstrap_limits(DEV_BASE_CURRENCY),
    )
    .await
}

/// The owner recorded on the keys and audit events of a deployed bootstrap.
const BOOTSTRAP_OWNER: &str = "bootstrap";
/// The longest company name a bootstrap accepts, in characters.
pub const BOOTSTRAP_NAME_MAX: usize = 200;

/// The input of a deployed-environment bootstrap: the company's display
/// name and its base currency (an ISO 4217 code such as `EUR`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CompanyBootstrap {
    /// The company's name, shown in the app and on reports.
    pub company: String,
    /// The base currency: three ASCII uppercase letters.
    pub currency: String,
}

impl CompanyBootstrap {
    /// Checks the name and the currency code.
    ///
    /// # Errors
    /// A message naming the field that is out of range.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.company.trim();
        if name.is_empty() || name.chars().count() > BOOTSTRAP_NAME_MAX {
            return Err(format!(
                "company must be 1 to {BOOTSTRAP_NAME_MAX} characters"
            ));
        }
        // The code must be the table's own spelling of an ISO 4217 currency
        // (uppercase, a known code), so a company is never stored in a code
        // the amount types cannot represent.
        match ledgers_money::Currency::from_code(&self.currency) {
            Ok(currency) if currency.code() == self.currency => Ok(()),
            _ => Err("currency must be an uppercase ISO 4217 code, such as EUR".to_owned()),
        }
    }
}

/// Why a deployed bootstrap did not run.
#[derive(Debug, PartialEq, Eq)]
pub enum BootstrapError {
    /// The input failed [`CompanyBootstrap::validate`].
    Invalid(String),
    /// An agent key already exists: the bootstrap is one-shot.
    AlreadyBootstrapped,
    /// A storage or configuration step failed; the message names the step.
    Failed(String),
}

/// Creates the company (when absent), seeds its chart, journal and fiscal year,
/// and issues one full-tier (post within limits) key. Returns the key secret,
/// which the caller shows once.
///
/// "No key exists yet" is decided inside the database, not by a read before the
/// write: the first batch inserts the company and the bootstrap's agent-key policy
/// row under a guard statement that refuses while any agent key exists for the
/// company (`docs/adr/0004-guard-statements.md`). Two bootstraps racing on the
/// same database cannot both pass that guard. The secret row itself is written by
/// the `ApiKeys` port after the claim; if anything after the claim fails, the claim
/// is released so a retry can run.
///
/// # Errors
/// [`BootstrapError`] for invalid input, an already-bootstrapped database, or
/// a failed step.
pub async fn bootstrap_company(
    deps: &Deps,
    company_id: &str,
    input: &CompanyBootstrap,
) -> Result<String, BootstrapError> {
    input.validate().map_err(BootstrapError::Invalid)?;
    if company_id.trim().is_empty() {
        return Err(BootstrapError::Failed(
            "LEDGERS_COMPANY_ID is not set".to_owned(),
        ));
    }
    let db = deps.db.as_ref();
    let stamp = rfc3339_utc(deps.clock.now());
    let key_id = deps.id_gen.ulid();
    let policy = agent_key_policy_statement(
        deps,
        company_id,
        &key_id,
        "bootstrap key",
        BOOTSTRAP_OWNER,
        Tier::PostWithinLimits,
        bootstrap_limits(&input.currency),
        None,
    )
    .map_err(BootstrapError::Failed)?;

    // The claim: one batch, all or nothing. The guard refuses while the
    // company has any agent key; the release removes the probe row again.
    let claim = ledgers_db::guard::guard_key(&["bootstrap-claim", company_id]);
    let batch = [
        ledgers_db::guard::assert_statement(
            claim.clone(),
            "(SELECT COUNT(*) FROM agent_key WHERE company_id = ?) = 0",
            vec![sea_query::Value::from(company_id)],
        ),
        Statement::with_values(
            "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
             SELECT ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM company WHERE id = ?)"
                .to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(input.company.trim()),
                sea_query::Value::from(input.currency.as_str()),
                sea_query::Value::from(stamp.as_str()),
                sea_query::Value::from(stamp.as_str()),
                sea_query::Value::from(company_id),
            ],
        ),
        policy,
        ledgers_db::guard::release_statement(claim),
    ];
    if let Err(err) = db.batch_atomic(&batch).await {
        return Err(if is_guard_refusal(&err) {
            BootstrapError::AlreadyBootstrapped
        } else {
            BootstrapError::Failed(format!("claim company: {err}"))
        });
    }

    // The claim stands. A failure from here on releases it, so the operator can
    // fix the cause and run the bootstrap again.
    let outcome = async {
        seed_company(deps, company_id, BOOTSTRAP_OWNER).await?;
        mint_key(deps, company_id, &key_id, BOOTSTRAP_OWNER).await
    }
    .await;
    match outcome {
        Ok(key) => Ok(key),
        Err(step) => {
            let release = db
                .execute(&Statement::with_values(
                    "DELETE FROM agent_key WHERE company_id = ? AND id = ?".to_owned(),
                    vec![
                        sea_query::Value::from(company_id),
                        sea_query::Value::from(key_id.as_str()),
                    ],
                ))
                .await;
            Err(BootstrapError::Failed(match release {
                Ok(_) => step,
                Err(err) => format!("{step}; releasing the claim also failed: {err}"),
            }))
        }
    }
}

/// Whether a batch failed because a guard refused it (the guard's `ok` column
/// is `NOT NULL`, so a refused probe reports that constraint).
fn is_guard_refusal(err: &cratefield_core::DbError) -> bool {
    matches!(err, cratefield_core::DbError::Batch(message)
        if message.contains("NOT NULL constraint failed: ledger_guard.ok"))
}

/// The registry of every command the server serves: the descriptors of
/// `ledgers_commands::default_commands()` with the input and output schemas
/// of the typed commands filled in.
///
/// # Panics
/// Never in practice: the command names are fixed and unique.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn registry() -> Registry {
    let schemas: BTreeMap<&str, (Value, Value)> = BTreeMap::from([
        (
            "bank.accounts.list",
            (
                schema_of::<ledgers_bank::model::AccountsListInput>(),
                schema_of::<ledgers_bank::model::AccountsListOutput>(),
            ),
        ),
        (
            "bank.accounts.get",
            (
                schema_of::<ledgers_bank::model::AccountsGetInput>(),
                schema_of::<ledgers_bank::model::AccountView>(),
            ),
        ),
        (
            "bank.accounts.reconcile",
            (
                schema_of::<ledgers_bank::model::ReconcileInput>(),
                schema_of::<ledgers_bank::model::ReconciliationView>(),
            ),
        ),
        (
            "bank.accounts.create",
            (
                schema_of::<ledgers_bank::model::AccountsCreateInput>(),
                schema_of::<ledgers_bank::model::AccountView>(),
            ),
        ),
        (
            "bank.accounts.update",
            (
                schema_of::<ledgers_bank::model::AccountsUpdateInput>(),
                schema_of::<ledgers_bank::model::AccountView>(),
            ),
        ),
        (
            "bank.accounts.archive",
            (
                schema_of::<ledgers_bank::model::AccountsArchiveInput>(),
                schema_of::<ledgers_bank::model::AccountView>(),
            ),
        ),
        (
            "bank.statements.import",
            (
                schema_of::<ledgers_bank::model::StatementsImportRequest>(),
                schema_of::<ledgers_bank::model::StatementsImportOutput>(),
            ),
        ),
        (
            "bank.transactions.list",
            (
                schema_of::<ledgers_bank::model::TransactionsListInput>(),
                schema_of::<ledgers_bank::model::TransactionsListOutput>(),
            ),
        ),
        (
            "bank.transactions.post",
            (
                schema_of::<ledgers_bank::model::TransactionsPostInput>(),
                schema_of::<ledgers_bank::model::TransactionsPostOutput>(),
            ),
        ),
        (
            "bank.transactions.link",
            (
                schema_of::<ledgers_bank::model::TransactionsLinkInput>(),
                schema_of::<ledgers_bank::model::TransactionStatusOutput>(),
            ),
        ),
        (
            "bank.transactions.ignore",
            (
                schema_of::<ledgers_bank::model::TransactionsIgnoreInput>(),
                schema_of::<ledgers_bank::model::TransactionStatusOutput>(),
            ),
        ),
        (
            "bank.rules.list",
            (
                schema_of::<ledgers_bank::model::RulesListInput>(),
                schema_of::<ledgers_bank::model::RulesListOutput>(),
            ),
        ),
        (
            "bank.rules.create",
            (
                schema_of::<ledgers_bank::model::RulesCreateRequest>(),
                schema_of::<ledgers_bank::model::RuleView>(),
            ),
        ),
        (
            "bank.rules.enable",
            (
                schema_of::<ledgers_bank::model::RulesEnableInput>(),
                schema_of::<ledgers_bank::model::RulesEnableOutput>(),
            ),
        ),
        (
            "accounts.list",
            (
                schema_of::<AccountsListInput>(),
                schema_of::<AccountsListOutput>(),
            ),
        ),
        (
            "accounts.create",
            (
                schema_of::<accounts::AccountsCreateInput>(),
                schema_of::<accounts::AccountsChangedOutput>(),
            ),
        ),
        (
            "accounts.update",
            (
                schema_of::<accounts::AccountsUpdateInput>(),
                schema_of::<accounts::AccountsChangedOutput>(),
            ),
        ),
        (
            "accounts.archive",
            (
                schema_of::<accounts::AccountsNumberInput>(),
                schema_of::<accounts::AccountsChangedOutput>(),
            ),
        ),
        (
            "accounts.restore",
            (
                schema_of::<accounts::AccountsNumberInput>(),
                schema_of::<accounts::AccountsChangedOutput>(),
            ),
        ),
        (
            "accounts.get",
            (
                schema_of::<accounts::AccountsNumberInput>(),
                schema_of::<accounts::AccountsGetOutput>(),
            ),
        ),
        (
            "entries.get",
            (
                schema_of::<EntriesGetInput>(),
                schema_of::<EntriesGetOutput>(),
            ),
        ),
        (
            "entries.list",
            (
                schema_of::<EntriesListInput>(),
                schema_of::<EntriesListOutput>(),
            ),
        ),
        (
            "entries.transfer",
            (
                schema_of::<units::EntriesTransferInput>(),
                schema_of::<PostEntryOutput>(),
            ),
        ),
        (
            "units.list",
            (
                schema_of::<units::UnitsListInput>(),
                schema_of::<units::UnitsListOutput>(),
            ),
        ),
        (
            "units.create",
            (
                schema_of::<units::UnitsCreateInput>(),
                schema_of::<units::UnitsChangedOutput>(),
            ),
        ),
        (
            "units.rename",
            (
                schema_of::<units::UnitsRenameInput>(),
                schema_of::<units::UnitsChangedOutput>(),
            ),
        ),
        (
            "units.archive",
            (
                schema_of::<units::UnitsCodeInput>(),
                schema_of::<units::UnitsChangedOutput>(),
            ),
        ),
        (
            "reports.units_summary",
            (
                schema_of::<units::UnitsSummaryInput>(),
                schema_of::<units::UnitsSummaryOutput>(),
            ),
        ),
        (
            "entries.post",
            (
                schema_of::<EntriesPostInput>(),
                schema_of::<PostEntryOutput>(),
            ),
        ),
        (
            "entries.reverse",
            (
                schema_of::<EntriesReverseInput>(),
                schema_of::<PostEntryOutput>(),
            ),
        ),
        (
            "reports.account_balance_by_dimension",
            (
                schema_of::<DimensionBalanceInput>(),
                schema_of::<DimensionBalanceOutput>(),
            ),
        ),
        (
            "reports.balance_sheet",
            (
                schema_of::<BalanceSheetInput>(),
                schema_of::<BalanceSheetOutput>(),
            ),
        ),
        (
            "reports.profit_and_loss",
            (
                schema_of::<PeriodInput>(),
                schema_of::<ProfitAndLossOutput>(),
            ),
        ),
        (
            "reports.trial_balance",
            (
                schema_of::<PeriodInput>(),
                schema_of::<TrialBalanceOutput>(),
            ),
        ),
        (
            "reports.trends",
            (schema_of::<TrendsInput>(), schema_of::<TrendsOutput>()),
        ),
        (
            "keys.list",
            (
                schema_of::<crate::agent_keys::KeysListInput>(),
                schema_of::<crate::agent_keys::KeysListOutput>(),
            ),
        ),
        (
            "keys.create",
            (
                schema_of::<crate::agent_keys::KeysCreateInput>(),
                schema_of::<crate::agent_keys::KeysCreateOutput>(),
            ),
        ),
        (
            "keys.revoke",
            (
                schema_of::<crate::agent_keys::KeysRevokeInput>(),
                schema_of::<crate::agent_keys::KeysRevokeOutput>(),
            ),
        ),
    ]);
    let mut schemas = schemas;
    schemas.extend([
        (
            "documents.list",
            (
                schema_of::<ledgers_documents::commands::ListInput>(),
                schema_of::<ledgers_documents::commands::ListOutput>(),
            ),
        ),
        (
            "documents.get",
            (
                schema_of::<ledgers_documents::commands::GetInput>(),
                schema_of::<ledgers_documents::commands::GetOutput>(),
            ),
        ),
        (
            "documents.download_url",
            (
                schema_of::<DocumentUrlInput>(),
                schema_of::<DocumentUrlOutput>(),
            ),
        ),
        (
            "bills.extract",
            (
                schema_of::<bills::ExtractInput>(),
                schema_of::<bills::ExtractResponse>(),
            ),
        ),
        (
            "bills.create",
            (
                schema_of::<ledgers_bills::model::BillInput>(),
                schema_of::<ledgers_bills::commands::CreateOutput>(),
            ),
        ),
        (
            "bills.update",
            (
                schema_of::<ledgers_bills::commands::UpdateInput>(),
                schema_of::<ledgers_bills::commands::CreateOutput>(),
            ),
        ),
        (
            "bills.get",
            (
                schema_of::<ledgers_bills::commands::GetInput>(),
                schema_of::<ledgers_bills::commands::GetOutput>(),
            ),
        ),
        (
            "bills.list",
            (
                schema_of::<ledgers_bills::commands::ListInput>(),
                schema_of::<ledgers_bills::commands::ListOutput>(),
            ),
        ),
        (
            "bills.post",
            (
                schema_of::<ledgers_bills::commands::PostInput>(),
                schema_of::<ledgers_bills::commands::PostOutput>(),
            ),
        ),
        (
            "bills.discard",
            (
                schema_of::<ledgers_bills::commands::DiscardInput>(),
                schema_of::<ledgers_bills::commands::GetOutput>(),
            ),
        ),
        (
            "bills.confirm",
            (
                schema_of::<ledgers_bills::commands::ConfirmInput>(),
                schema_of::<ledgers_bills::commands::GetOutput>(),
            ),
        ),
        (
            "parties.list",
            (
                schema_of::<ledgers_bills::parties::ListInput>(),
                schema_of::<ledgers_bills::parties::ListOutput>(),
            ),
        ),
        (
            "parties.create",
            (
                schema_of::<ledgers_bills::parties::CreateInput>(),
                schema_of::<ledgers_bills::parties::CreateOutput>(),
            ),
        ),
        (
            "parties.update",
            (
                schema_of::<ledgers_bills::parties::UpdateInput>(),
                schema_of::<ledgers_bills::parties::UpdateOutput>(),
            ),
        ),
        (
            "parties.set_bank",
            (
                schema_of::<ledgers_bills::parties::SetBankInput>(),
                schema_of::<ledgers_bills::parties::SetBankOutput>(),
            ),
        ),
        (
            "parties.check_vat",
            (
                schema_of::<ledgers_bills::parties::CheckVatInput>(),
                schema_of::<ledgers_bills::parties::CheckVatOutput>(),
            ),
        ),
        (
            "parties.merge",
            (
                schema_of::<ledgers_bills::parties::MergeInput>(),
                schema_of::<ledgers_bills::parties::MergeOutput>(),
            ),
        ),
        (
            "company.get",
            (
                schema_of::<Value>(),
                schema_of::<settings::CompanyGetOutput>(),
            ),
        ),
        (
            "company.update",
            (
                schema_of::<settings::CompanyUpdateInput>(),
                schema_of::<settings::CompanyGetOutput>(),
            ),
        ),
        (
            "fiscal_years.list",
            (
                schema_of::<Value>(),
                schema_of::<settings::FiscalYearsListOutput>(),
            ),
        ),
        (
            "fiscal_years.create",
            (
                schema_of::<settings::FiscalYearCreateInput>(),
                schema_of::<settings::FiscalYearView>(),
            ),
        ),
        (
            "fiscal_years.close",
            (
                schema_of::<settings::FiscalYearCloseInput>(),
                schema_of::<settings::FiscalYearCloseOutput>(),
            ),
        ),
        (
            "periods.close",
            (
                schema_of::<settings::PeriodCloseInput>(),
                schema_of::<settings::PeriodView>(),
            ),
        ),
        (
            "periods.reopen",
            (
                schema_of::<settings::PeriodReopenInput>(),
                schema_of::<settings::PeriodView>(),
            ),
        ),
        (
            "numbering.list",
            (
                schema_of::<Value>(),
                schema_of::<settings::NumberingListOutput>(),
            ),
        ),
        (
            "numbering.set_next",
            (
                schema_of::<settings::NumberingSetNextInput>(),
                schema_of::<settings::NumberingSetNextInput>(),
            ),
        ),
        (
            "tax_settings.get",
            (
                schema_of::<Value>(),
                schema_of::<settings::TaxSettingsOutput>(),
            ),
        ),
        (
            "tax_settings.update",
            (
                schema_of::<settings::TaxSettingsInput>(),
                schema_of::<settings::TaxSettingsOutput>(),
            ),
        ),
        (
            "approval_settings.get",
            (
                schema_of::<Value>(),
                schema_of::<settings::ApprovalSettingsView>(),
            ),
        ),
        (
            "approval_settings.update",
            (
                schema_of::<settings::ApprovalSettingsInput>(),
                schema_of::<settings::ApprovalSettingsView>(),
            ),
        ),
        (
            "audit.list",
            (
                schema_of::<settings::AuditListInput>(),
                schema_of::<settings::AuditListOutput>(),
            ),
        ),
    ]);
    let mut registry = Registry::new();
    for mut descriptor in ledgers_commands::default_commands() {
        if let Some((input, output)) = schemas.get(descriptor.name.as_str()) {
            descriptor.input_schema = input.clone();
            descriptor.output_schema = output.clone();
        }
        registry
            .register(descriptor)
            .expect("command names are unique");
    }
    registry
}

fn schema_of<T: JsonSchema>() -> Value {
    serde_json::to_value(
        schemars::generate::SchemaSettings::default()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap_or(Value::Null)
}

/// `YYYY-MM-DDTHH:MM:SSZ` in UTC: fixed width, so text order is time order.
#[must_use]
pub fn rfc3339_utc(now: OffsetDateTime) -> String {
    let u = now.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        u.year(),
        u8::from(u.month()),
        u.day(),
        u.hour(),
        u.minute(),
        u.second()
    )
}

fn ymd(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn parse_date(text: &str) -> Result<Date, CommandError> {
    Date::parse(text, &Iso8601::DEFAULT)
        .map_err(|_| CommandError::InvalidInput(format!("{text:?} is not a YYYY-MM-DD date")))
}

fn check_period(period: &str) -> Result<(), CommandError> {
    let bytes = period.as_bytes();
    let valid = bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit);
    if valid {
        Ok(())
    } else {
        Err(CommandError::InvalidInput(format!(
            "{period:?} is not a YYYY-MM period"
        )))
    }
}

fn domain(err: &ledgers_ledger::JournalError) -> CommandError {
    CommandError::Domain(err.to_problem())
}

/// The preview of a validated entry: one line per entry line.
fn preview_of(entry: &ledgers_ledger::Entry) -> Result<JournalPreview, CommandError> {
    let mut preview = JournalPreview::empty();
    for line in &entry.lines {
        let side = line.side.to_string().to_lowercase();
        let txn = line.fx.as_ref().map(|fx| fx.transaction);
        preview
            .add_line(line.account_number.clone(), &side, line.base, txn)
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    }
    Ok(preview)
}

/// Deserializes a numeric query or body field sent as a number or a string.
fn de_opt_u32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    use serde::de::Error;
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| D::Error::custom("expected a non-negative 32-bit integer")),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .parse::<u32>()
            .map(Some)
            .map_err(|_| D::Error::custom("expected a non-negative integer")),
        Some(_) => Err(D::Error::custom("expected an integer")),
    }
}

// ----------------------------------------------------------------- entries

/// The input of `entries.post`: a balanced entry and, optionally, the
/// journal and fiscal year to post it to (defaults: journal `GN`, and the
/// fiscal year of the posting date).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntriesPostInput {
    /// The journal to post into; defaults to the general journal `GN`.
    pub journal_id: Option<String>,
    /// The fiscal year the entry number is scoped to; defaults to the
    /// posting date's year.
    pub fiscal_year: Option<i64>,
    /// The entry: header, dates and lines.
    pub entry: SerializableEntryDraft,
}

/// `entries.post`: posts one balanced entry (`POST /v1/entries`).
pub struct EntriesPost;

#[async_trait]
impl Command for EntriesPost {
    const NAME: &'static str = "entries.post";
    const TIER: Tier = Tier::PostWithinLimits;
    const IDEMPOTENT: bool = true;

    type Input = EntriesPostInput;
    type Output = PostEntryOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let resolved = resolve_post(db, ctx, input).await?;
        PostEntry.preview(db, ctx, &resolved).await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let resolved = resolve_post(db, ctx, input).await?;
        PostEntry.plan(token, db, ctx, &resolved).await
    }
}

/// Fills the journal and fiscal year defaults of an `entries.post` input.
async fn resolve_post(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &EntriesPostInput,
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
    let fiscal_year = if let Some(year) = input.fiscal_year {
        year
    } else {
        let posting = &input.entry.posting_date;
        posting
            .get(..4)
            .and_then(|year| year.parse::<i64>().ok())
            .ok_or_else(|| {
                CommandError::InvalidInput(format!("{posting:?} is not a YYYY-MM-DD date"))
            })?
    };
    let mut entry = input.entry.clone();
    entry.journal_id.clone_from(&journal_id);
    Ok(PostEntryInput {
        journal_id,
        fiscal_year,
        entry,
    })
}

/// The input of `entries.reverse`: the external id of the entry to reverse
/// (from the path) and the posting date of the reversal (default: today).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntriesReverseInput {
    /// The `external_id` the reversed entry was posted with.
    pub external_id: String,
    /// The reversal's posting date, `YYYY-MM-DD`; defaults to today (UTC).
    pub posting_date: Option<String>,
}

/// `entries.reverse`: posts the mirror of a posted entry. Posted entries are
/// never edited; a correction is a reversal plus new entries (`POST
/// /v1/entries/{external_id}/reverse`).
pub struct EntriesReverse;

/// Resolves the entry a reversal targets and builds its mirror.
async fn reversal_of(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &EntriesReverseInput,
) -> Result<(String, ReverseContext), CommandError> {
    let original = query::entry_by_external_id(db, &ctx.company_id, &input.external_id)
        .await
        .map_err(|err| domain(&err))?
        .ok_or_else(|| {
            CommandError::InvalidInput(format!("no entry with external_id {:?}", input.external_id))
        })?;
    let requested = match &input.posting_date {
        Some(text) => parse_date(text)?,
        None => parse_date(&ctx.now[..10])?,
    };
    let reverse_ctx = ReverseContext {
        company_id: ctx.company_id.clone(),
        entry_id: original.id.clone(),
        requested_posting_date: requested,
        human: false,
    };
    Ok((original.id, reverse_ctx))
}

#[async_trait]
impl Command for EntriesReverse {
    const NAME: &'static str = "entries.reverse";
    const TIER: Tier = Tier::PostWithinLimits;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("external_id");

    type Input = EntriesReverseInput;
    type Output = PostEntryOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let (_, reverse_ctx) = reversal_of(db, ctx, input).await?;
        let mirror = ledgers_ledger::reversal_entry(db, &reverse_ctx)
            .await
            .map_err(|err| domain(&err))?;
        preview_of(&mirror)
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let (original_id, reverse_ctx) = reversal_of(db, ctx, input).await?;
        let (statements, posted) =
            ledgers_ledger::plan_reverse_entry(token, db, &reverse_ctx, &ctx.now, &ctx.request_id)
                .await
                .map_err(|err| domain(&err))?;
        let objects = vec![posted.entry_id.clone(), original_id.clone()];
        Ok(Plan {
            statements,
            output: PostEntryOutput {
                entry: SerializablePostedEntry::from(posted),
            },
            objects,
        })
    }
}

/// One entry line, as the app's `EntryLineRecord` types it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EntryLineRecordView {
    /// The account number.
    pub account_number: String,
    /// `debit` or `credit`.
    pub side: String,
    /// The base amount, in minor units of the entry's base currency.
    pub base: SerializableAmount,
    /// The memo, if any.
    pub memo: Option<String>,
}

/// One posted entry, as the app's `EntryRecord` types it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EntryRecordView {
    /// The source application's idempotency key, if it gave one.
    pub external_id: Option<String>,
    /// The entry id.
    pub entry_id: String,
    /// The journal it posted into.
    pub journal_id: String,
    /// The fiscal year its number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number within the journal and fiscal year.
    pub number: i64,
    /// The document date, `YYYY-MM-DD`.
    pub document_date: String,
    /// The posting date, `YYYY-MM-DD`.
    pub posting_date: String,
    /// The base currency code.
    pub base_currency: String,
    /// The description.
    pub description: String,
    /// Who posted it: `agent` (an agent key) or `person`.
    pub posted_by: String,
    /// The entry this one reverses, when it is a reversal.
    pub reverses: Option<String>,
    /// The entry that reverses this one, if any.
    pub reversed_by: Option<String>,
    /// When it was recorded (UTC).
    pub recorded_at: String,
    /// The lines.
    pub lines: Vec<EntryLineRecordView>,
}

impl From<EntryRecord> for EntryRecordView {
    fn from(record: EntryRecord) -> Self {
        let entry = record.entry;
        EntryRecordView {
            entry_id: record.id,
            external_id: entry.external_id.clone(),
            journal_id: entry.journal_id.clone(),
            fiscal_year: record.fiscal_year,
            number: record.number,
            document_date: ymd(entry.document_date),
            posting_date: ymd(entry.posting_date),
            base_currency: entry.base_currency.code().to_string(),
            description: entry.description.clone(),
            posted_by: if record.actor == "human" {
                "person".to_owned()
            } else {
                "agent".to_owned()
            },
            reverses: entry.reverses.clone(),
            reversed_by: record.reversed_by.first().cloned(),
            recorded_at: record.recorded_at,
            lines: record
                .lines
                .into_iter()
                .map(|line| EntryLineRecordView {
                    account_number: line.account_number,
                    side: line.side.to_string().to_lowercase(),
                    base: SerializableAmount::from(line.base),
                    memo: line.memo,
                })
                .collect(),
        }
    }
}

/// The input of `entries.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntriesListInput {
    /// Only entries of this VAT period (`YYYY-MM`); all periods when absent.
    pub period: Option<String>,
    /// The id of the last entry of the previous page: the page starts after
    /// it. The first page when absent. An id the company does not hold is a 404.
    pub after: Option<String>,
    /// At most this many entries, newest first (default 50, cap 200).
    #[serde(default, deserialize_with = "de_opt_u32")]
    #[schemars(with = "Option<u32>")]
    pub limit: Option<u32>,
}

/// The output of `entries.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EntriesListOutput {
    /// The entries, newest posting first.
    pub entries: Vec<EntryRecordView>,
}

/// `entries.list`: lists posted entries (`GET /v1/entries`).
pub struct EntriesList;

#[async_trait]
impl Command for EntriesList {
    const NAME: &'static str = "entries.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = EntriesListInput;
    type Output = EntriesListOutput;

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
        if let Some(period) = &input.period {
            check_period(period)?;
        }
        let limit = input
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let records = query::list_entries(
            db,
            &ctx.company_id,
            input.period.as_deref(),
            input.after.as_deref(),
            limit,
        )
        .await
        .map_err(|err| domain(&err))?
        .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        Ok(Plan::with_output(EntriesListOutput {
            entries: records.into_iter().map(EntryRecordView::from).collect(),
        }))
    }
}

/// The input of `entries.get`: the entry's external id (from the path).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntriesGetInput {
    /// The `external_id` the entry was posted with.
    pub external_id: String,
}

/// The output of `entries.get`: the entry record itself.
pub type EntriesGetOutput = EntryRecordView;

/// `entries.get`: one posted entry by its external id (`GET /v1/entries/{external_id}`).
pub struct EntriesGet;

#[async_trait]
impl Command for EntriesGet {
    const NAME: &'static str = "entries.get";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("external_id");

    type Input = EntriesGetInput;
    type Output = EntriesGetOutput;

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
        let record = query::entry_by_external_id(db, &ctx.company_id, &input.external_id)
            .await
            .map_err(|err| domain(&err))?
            .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        Ok(Plan::with_output(EntryRecordView::from(record)))
    }
}

// ---------------------------------------------------------------- accounts

/// The input of `accounts.list`: none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccountsListInput {}

/// One account of the chart, as the app's `Account` types it, plus its balances.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AccountView {
    /// The account number.
    pub number: String,
    /// The translatable name key.
    pub name_key: String,
    /// The custom name a person gave the account, or `null` to show the
    /// translated `name_key`.
    pub display_name: Option<String>,
    /// The account class: asset, liability, equity, income, expense.
    pub account_type: String,
    /// The normal balance side.
    pub normal_balance: String,
    /// The account currency, or `null` for the company base currency.
    pub currency: Option<String>,
    /// The parent account number, if any.
    pub parent: Option<String>,
    /// The chart tags of the account.
    pub tags: Vec<String>,
    /// Whether the account accepts postings (a leaf of the chart).
    pub is_postable: bool,
    /// Whether the account is archived: kept for history, no new postings.
    pub archived: bool,
    /// The balance per base currency, debit-positive (empty when no postings).
    pub balances: Vec<SerializableAmount>,
}

/// The output of `accounts.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AccountsListOutput {
    /// The accounts, by number.
    pub accounts: Vec<AccountView>,
}

/// `accounts.list`: the chart with each account's balance (`GET /v1/accounts`).
pub struct AccountsList;

#[async_trait]
impl Command for AccountsList {
    const NAME: &'static str = "accounts.list";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = AccountsListInput;
    type Output = AccountsListOutput;

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
        let accounts = ledgers_ledger::commands::list_accounts(db, &ctx.company_id, true)
            .await
            .map_err(|err| CommandError::Domain(err.to_problem()))?;
        let balances = query::account_balances(db, &ctx.company_id)
            .await
            .map_err(|err| domain(&err))?;
        let mut by_account: BTreeMap<String, Vec<SerializableAmount>> = BTreeMap::new();
        for balance in balances {
            by_account
                .entry(balance.account_number)
                .or_default()
                .push(SerializableAmount::from(balance.balance));
        }
        let accounts = accounts
            .into_iter()
            .map(|entry| {
                let is_postable = entry.is_postable();
                let account = entry.account;
                AccountView {
                    balances: by_account.remove(&account.number).unwrap_or_default(),
                    number: account.number,
                    name_key: account.name_key,
                    display_name: account.display_name,
                    account_type: account.account_type.to_string(),
                    normal_balance: account.normal_balance.to_string(),
                    currency: account.currency.map(|c| c.code().to_string()),
                    parent: account.parent,
                    tags: account.tags,
                    is_postable,
                    archived: account.archived,
                }
            })
            .collect();
        Ok(Plan::with_output(AccountsListOutput { accounts }))
    }
}

// ----------------------------------------------------------------- reports

/// The input of the period reports: `YYYY-MM`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeriodInput {
    /// The VAT period, `YYYY-MM`.
    pub period: String,
    /// The reporting scope: a unit code, or `all` (the default: consolidated).
    #[serde(default)]
    pub unit: Option<String>,
}

/// One trial-balance row.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TrialBalanceRowView {
    /// The account number.
    pub account_code: String,
    /// The debit total of the period.
    pub debit: SerializableAmount,
    /// The credit total of the period.
    pub credit: SerializableAmount,
    /// The balance (debit positive).
    pub balance: SerializableAmount,
}

/// The output of `reports.trial_balance`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TrialBalanceOutput {
    /// The period reported.
    pub period: String,
    /// The rows, by account.
    pub rows: Vec<TrialBalanceRowView>,
}

/// `reports.trial_balance`: debit and credit totals per account for a
/// period (`GET /v1/reports/trial-balance?period=YYYY-MM`).
pub struct ReportsTrialBalance;

#[async_trait]
impl Command for ReportsTrialBalance {
    const NAME: &'static str = "reports.trial_balance";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = PeriodInput;
    type Output = TrialBalanceOutput;

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
        check_period(&input.period)?;
        let scope = resolve_scope(db, &ctx.company_id, input.unit.as_deref()).await?;
        let rows = report::trial_balance_scoped(db, &ctx.company_id, &input.period, &scope)
            .await
            .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(TrialBalanceOutput {
            period: input.period.clone(),
            rows: rows
                .into_iter()
                .map(|row| TrialBalanceRowView {
                    account_code: row.account_code,
                    debit: SerializableAmount::from(row.debit),
                    credit: SerializableAmount::from(row.credit),
                    balance: SerializableAmount::from(row.balance),
                })
                .collect(),
        }))
    }
}

/// The output of `reports.profit_and_loss`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ProfitAndLossOutput {
    /// The period reported.
    pub period: String,
    /// Total income.
    pub revenue: SerializableAmount,
    /// Total expenses.
    pub expenses: SerializableAmount,
    /// Revenue minus expenses.
    pub net_income: SerializableAmount,
}

/// `reports.profit_and_loss`: income, expenses and net income for a period
/// (`GET /v1/reports/p-and-l?period=YYYY-MM`).
pub struct ReportsProfitAndLoss;

#[async_trait]
impl Command for ReportsProfitAndLoss {
    const NAME: &'static str = "reports.profit_and_loss";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = PeriodInput;
    type Output = ProfitAndLossOutput;

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
        check_period(&input.period)?;
        let scope = resolve_scope(db, &ctx.company_id, input.unit.as_deref()).await?;
        let pnl = report::profit_and_loss_scoped(db, &ctx.company_id, &input.period, &scope)
            .await
            .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(ProfitAndLossOutput {
            period: input.period.clone(),
            revenue: SerializableAmount::from(pnl.revenue),
            expenses: SerializableAmount::from(pnl.expenses),
            net_income: SerializableAmount::from(pnl.net_income),
        }))
    }
}

/// The input of `reports.balance_sheet`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BalanceSheetInput {
    /// The date the balance sheet is as of, `YYYY-MM-DD`.
    pub as_of: String,
    /// The reporting scope: a unit code, or `all` (the default: consolidated).
    #[serde(default)]
    pub unit: Option<String>,
}

/// The output of `reports.balance_sheet`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct BalanceSheetOutput {
    /// The date reported.
    pub as_of: String,
    /// Total assets.
    pub assets: SerializableAmount,
    /// Total liabilities.
    pub liabilities: SerializableAmount,
    /// Total equity as presented: `equity_accounts` plus `current_year_result`.
    pub equity: SerializableAmount,
    /// The equity accounts alone (capital, reserves, retained earnings).
    pub equity_accounts: SerializableAmount,
    /// Income minus expenses from the fiscal year start up to `as_of`
    /// (Jahresüberschuss, or Jahresfehlbetrag when negative).
    pub current_year_result: SerializableAmount,
    /// Assets minus liabilities minus equity. Zero when `balanced`.
    pub difference: SerializableAmount,
    /// Whether the sheet balances on its own: assets = liabilities + equity.
    pub balanced: bool,
}

/// `reports.balance_sheet`: assets, liabilities and equity as of a date
/// (`GET /v1/reports/balance-sheet?as_of=YYYY-MM-DD`).
pub struct ReportsBalanceSheet;

#[async_trait]
impl Command for ReportsBalanceSheet {
    const NAME: &'static str = "reports.balance_sheet";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = BalanceSheetInput;
    type Output = BalanceSheetOutput;

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
        parse_date(&input.as_of)?;
        let scope = resolve_scope(db, &ctx.company_id, input.unit.as_deref()).await?;
        let sheet = report::balance_sheet_scoped(db, &ctx.company_id, &input.as_of, &scope)
            .await
            .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(BalanceSheetOutput {
            as_of: input.as_of.clone(),
            assets: SerializableAmount::from(sheet.assets),
            liabilities: SerializableAmount::from(sheet.liabilities),
            equity: SerializableAmount::from(sheet.equity),
            equity_accounts: SerializableAmount::from(sheet.equity_accounts),
            current_year_result: SerializableAmount::from(sheet.current_year_result),
            difference: SerializableAmount::from(sheet.difference),
            balanced: sheet.balanced,
        }))
    }
}

/// The input of `reports.account_balance_by_dimension`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DimensionBalanceInput {
    /// The account number.
    pub account: String,
    /// The dimension key, for example `cost-centre`.
    pub dimension: String,
    /// The VAT period, `YYYY-MM`.
    pub period: String,
    /// The reporting scope: a unit code, or `all` (the default: consolidated).
    #[serde(default)]
    pub unit: Option<String>,
}

/// One dimension value's balance.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DimensionValueView {
    /// The dimension value.
    pub dimension_value: String,
    /// The balance (debit positive).
    pub balance: SerializableAmount,
}

/// The output of `reports.account_balance_by_dimension`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DimensionBalanceOutput {
    /// The account reported.
    pub account: String,
    /// The dimension key.
    pub dimension: String,
    /// The period reported.
    pub period: String,
    /// One row per dimension value.
    pub rows: Vec<DimensionValueView>,
}

/// `reports.account_balance_by_dimension`: an account's balance split by a
/// dimension, for a period.
pub struct ReportsAccountBalanceByDimension;

#[async_trait]
impl Command for ReportsAccountBalanceByDimension {
    const NAME: &'static str = "reports.account_balance_by_dimension";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = DimensionBalanceInput;
    type Output = DimensionBalanceOutput;

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
        check_period(&input.period)?;
        let scope = resolve_scope(db, &ctx.company_id, input.unit.as_deref()).await?;
        let rows = report::account_balance_by_dimension_scoped(
            db,
            &ctx.company_id,
            &input.account,
            &input.dimension,
            &input.period,
            &scope,
        )
        .await
        .map_err(|err| domain(&err))?;
        Ok(Plan::with_output(DimensionBalanceOutput {
            account: input.account.clone(),
            dimension: input.dimension.clone(),
            period: input.period.clone(),
            rows: rows
                .into_iter()
                .map(|row| DimensionValueView {
                    dimension_value: row.dimension_value,
                    balance: SerializableAmount::from(row.balance),
                })
                .collect(),
        }))
    }
}

/// The input of `reports.trends`: a month range and an optional dimension
/// kind to split revenue and expenses by.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TrendsInput {
    /// The first month, `YYYY-MM`.
    pub from: String,
    /// The last month, `YYYY-MM`, inclusive. At most 120 months after `from`.
    pub to: String,
    /// The dimension kind to split by: `cost_centre`, `project` or `unit`.
    #[serde(default)]
    pub dimension: Option<String>,
    /// The reporting scope: a unit code, or `all` (the default: consolidated).
    #[serde(default)]
    pub unit: Option<String>,
}

/// One month of `reports.trends`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TrendMonthView {
    /// The month, `YYYY-MM`.
    pub period: String,
    /// Income for the month.
    pub revenue: SerializableAmount,
    /// Expenses for the month.
    pub expenses: SerializableAmount,
    /// Revenue minus expenses.
    pub net_result: SerializableAmount,
    /// Cash and bank balance at month end.
    pub cash: SerializableAmount,
    /// Receivables balance at month end.
    pub receivables: SerializableAmount,
    /// Payables balance at month end, as a positive amount owed.
    pub payables: SerializableAmount,
}

/// One ranked expense account of `reports.trends`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ExpenseAccountView {
    /// The account number.
    pub account: String,
    /// The account's name key.
    pub name_key: String,
    /// The account's expenses in the range.
    pub amount: SerializableAmount,
    /// Share of the range's expenses in basis points (1/100 of a percent).
    pub share_bps: i64,
}

/// Revenue and expenses of one dimension value in `reports.trends`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DimensionShareView {
    /// The dimension value's code.
    pub code: String,
    /// The dimension value's name.
    pub name: String,
    /// Income tagged with the value.
    pub revenue: SerializableAmount,
    /// Expenses tagged with the value.
    pub expenses: SerializableAmount,
}

/// One month of entries posted, by actor, in `reports.trends`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ActivityMonthView {
    /// The month, `YYYY-MM`.
    pub period: String,
    /// Entries posted by an agent key.
    pub agent: i64,
    /// Entries posted by a person.
    pub human: i64,
}

/// The output of `reports.trends`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TrendsOutput {
    /// The company's base currency code.
    pub currency: String,
    /// The first month reported.
    pub from: String,
    /// The last month reported.
    pub to: String,
    /// One row per month, in order.
    pub months: Vec<TrendMonthView>,
    /// The largest expense accounts in the range.
    pub top_expense_accounts: Vec<ExpenseAccountView>,
    /// The dimension kind the split is by, when one was asked for.
    pub dimension: Option<String>,
    /// Revenue and expenses per dimension value; empty without a dimension.
    pub by_dimension: Vec<DimensionShareView>,
    /// Entries posted per month by actor; months with none are left out.
    pub activity: Vec<ActivityMonthView>,
}

/// The longest range `reports.trends` answers, in months.
pub const TRENDS_MAX_MONTHS: usize = 120;

/// The dimension kinds a trend report can split by.
const TREND_DIMENSIONS: [&str; 3] = ["cost_centre", "project", "unit"];

/// `reports.trends`: monthly revenue, expenses, net result, cash, receivables
/// and payables over a range, the top expense accounts, and an optional split
/// by dimension (`GET /v1/reports/trends?from=YYYY-MM&to=YYYY-MM`).
pub struct ReportsTrends;

#[async_trait]
impl Command for ReportsTrends {
    const NAME: &'static str = "reports.trends";
    const TIER: Tier = Tier::Read;
    const IDEMPOTENT: bool = true;

    type Input = TrendsInput;
    type Output = TrendsOutput;

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
        check_period(&input.from)?;
        check_period(&input.to)?;
        let months = report::month_range(&input.from, &input.to).ok_or_else(|| {
            CommandError::InvalidInput(format!(
                "from {:?} must not be after to {:?}",
                input.from, input.to
            ))
        })?;
        if months.len() > TRENDS_MAX_MONTHS {
            return Err(CommandError::InvalidInput(format!(
                "a trend range covers at most {TRENDS_MAX_MONTHS} months"
            )));
        }
        let dimension = input.dimension.as_deref().filter(|kind| !kind.is_empty());
        if let Some(kind) = dimension
            && !TREND_DIMENSIONS.contains(&kind)
        {
            return Err(CommandError::InvalidInput(format!(
                "{kind:?} is not a dimension kind (cost_centre, project or unit)"
            )));
        }
        let scope = resolve_scope(db, &ctx.company_id, input.unit.as_deref()).await?;
        let trends = report::trends_scoped(
            db,
            &ctx.company_id,
            &input.from,
            &input.to,
            dimension,
            &scope,
        )
        .await
        .map_err(|err| domain(&err))?;
        let currency = trends
            .months
            .first()
            .map(|month| SerializableAmount::from(month.revenue).currency)
            .unwrap_or_default();
        Ok(Plan::with_output(TrendsOutput {
            currency,
            from: trends.from,
            to: trends.to,
            months: trends
                .months
                .into_iter()
                .map(|month| TrendMonthView {
                    period: month.period,
                    revenue: SerializableAmount::from(month.revenue),
                    expenses: SerializableAmount::from(month.expenses),
                    net_result: SerializableAmount::from(month.net_result),
                    cash: SerializableAmount::from(month.cash),
                    receivables: SerializableAmount::from(month.receivables),
                    payables: SerializableAmount::from(month.payables),
                })
                .collect(),
            top_expense_accounts: trends
                .top_expense_accounts
                .into_iter()
                .map(|account| ExpenseAccountView {
                    account: account.account_code,
                    name_key: account.name_key,
                    amount: SerializableAmount::from(account.amount),
                    share_bps: account.share_bps,
                })
                .collect(),
            dimension: trends.dimension,
            by_dimension: trends
                .by_dimension
                .into_iter()
                .map(|share| DimensionShareView {
                    code: share.code,
                    name: share.name,
                    revenue: SerializableAmount::from(share.revenue),
                    expenses: SerializableAmount::from(share.expenses),
                })
                .collect(),
            activity: trends
                .activity
                .into_iter()
                .map(|month| ActivityMonthView {
                    period: month.period,
                    agent: month.agent,
                    human: month.human,
                })
                .collect(),
        }))
    }
}

/// Migrations of the harness API-key table, shipped with the server.
pub const API_KEY_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "api_keys",
        include_str!("../migrations/sqlite/0001_api_keys.sql"),
    )];

/// The module that declares the API-key table to the harness.
pub struct ApiKeysModule;

impl cratefield_core::Module for ApiKeysModule {
    fn name(&self) -> &'static str {
        "ledgers-api-keys"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn router(&self, _ctx: cratefield_core::ModuleContext) -> cratefield_core::axum::Router {
        cratefield_core::axum::Router::new()
    }

    fn migrations(&self) -> cratefield_core::Migrations {
        cratefield_core::Migrations::sqlite(API_KEY_MIGRATIONS)
    }

    fn validate_config(
        &self,
        _cfg: &dyn cratefield_core::Config,
    ) -> Result<(), cratefield_core::ConfigError> {
        Ok(())
    }
}
