//! The approvals inbox on the server: the person commands that list, read,
//! approve and reject the requests a needs-approval call stored (docs/PLAN.md
//! section 5). Every command here is human-only, so an agent key gets 403.
//! Approving runs the stored command through the executor exactly once: the
//! status guard and the command's writes share one batch. A high-risk request
//! needs a sign-in within the last five minutes; a low-risk one does not.

use super::{Authn, Deps, EntriesPost, EntriesReverse, KeyStore, ServerDispatch, load_policy};
use crate::people::{SessionAuthenticator, load_session};
use crate::routes::Presented;
use async_trait::async_trait;
use cratefield_core::axum::http::StatusCode;
use cratefield_core::{Database, DbError, Problem, ProblemDef};
use ledgers_approvals::{
    ApprovalConfig, ApprovalError, ApprovalRequest, Approvals, Es256Verifier, Listing,
    NoCredentialKeys, OwnerApprovesAll, check_bulk, get_request, list_requests,
};
use ledgers_commands::auth::{Authenticator, Credential, Tier};
use ledgers_commands::{
    Command, CommandContext, CommandError, ExecToken, Executor, JournalPreview, Plan, Policy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Per-currency limits (canonical amount strings) at or below which a money
/// request is low risk: the only kind a bulk approval may include.
pub const LOW_RISK_LIMITS: &[(&str, &str)] =
    &[("EUR", "100.00"), ("GBP", "100.00"), ("USD", "100.00")];

/// The low-risk limits as the approvals crate reads them.
#[must_use]
pub fn low_risk_limits() -> BTreeMap<String, String> {
    LOW_RISK_LIMITS
        .iter()
        .map(|(code, limit)| ((*code).to_owned(), (*limit).to_owned()))
        .collect()
}

/// The link an agent shows a person to decide a request: `{app}/approvals/{id}`.
#[must_use]
pub fn approval_url(app_url: &str, approval_id: &str) -> String {
    format!("{}/approvals/{approval_id}", app_url.trim_end_matches('/'))
}

const REAUTH_REQUIRED: ProblemDef = ProblemDef {
    slug: "reauth-required",
    status: StatusCode::UNAUTHORIZED,
    title: "Sign in again to approve",
    description: "A high-risk approval needs a sign-in within the last five minutes.",
};

const NOT_PENDING: ProblemDef = ProblemDef {
    slug: "approval-not-pending",
    status: StatusCode::CONFLICT,
    title: "Already decided",
    description: "The approval request is no longer pending.",
};

const EXPIRED: ProblemDef = ProblemDef {
    slug: "approval-expired",
    status: StatusCode::CONFLICT,
    title: "Approval expired",
    description: "The approval request expired before it was decided.",
};

const NOT_FOUND: ProblemDef = ProblemDef {
    slug: "approval-not-found",
    status: StatusCode::NOT_FOUND,
    title: "No such approval",
    description: "No approval request with that id in this company.",
};

const BULK_REFUSED: ProblemDef = ProblemDef {
    slug: "bulk-selection-refused",
    status: StatusCode::UNPROCESSABLE_ENTITY,
    title: "Bulk approval refused",
    description: "The selection cannot be approved in one step.",
};

/// One approval request as the inbox shows it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ApprovalView {
    /// The approval request id.
    pub id: String,
    /// `pending`, `approved`, `rejected`, `expired` or `superseded`.
    pub status: String,
    /// The command the agent asked to run.
    pub command: String,
    /// `low` or `high`.
    pub risk: String,
    /// Per-currency amounts the request moves, as canonical strings.
    pub amount: BTreeMap<String, String>,
    /// The counterparty the command names, if any.
    pub counterparty: Option<String>,
    /// Why policy asked for approval.
    pub reason: String,
    /// The requesting agent key id.
    pub requested_by: String,
    /// When the request was created (RFC 3339 UTC).
    pub created_at: String,
    /// When the request expires (RFC 3339 UTC).
    pub expires_at: String,
    /// When the request was decided, if it was.
    pub decided_at: Option<String>,
    /// The person who decided, if decided.
    pub decided_by: Option<String>,
    /// The reason given for a rejection.
    pub decision_reason: Option<String>,
    /// The command output after an approval: the entry reference.
    pub result: Option<Value>,
    /// The audit event that recorded the decision.
    pub audit_ref: Option<String>,
    /// The hash binding the approval to exactly this request.
    pub request_hash: String,
    /// The journal preview. Only present on a single-request read.
    pub preview: Option<Value>,
}

fn view(request: &ApprovalRequest, with_preview: bool) -> ApprovalView {
    ApprovalView {
        id: request.id.clone(),
        status: request.status.as_str().to_owned(),
        command: request.command.clone(),
        risk: request.risk.as_str().to_owned(),
        amount: request.effect.clone(),
        counterparty: request.counterparty.clone(),
        reason: request.reason.clone(),
        requested_by: request.requested_by.clone(),
        created_at: request.created_at.clone(),
        expires_at: request.expires_at.clone(),
        decided_at: request.decided_at.clone(),
        decided_by: request.decided_by.clone(),
        decision_reason: request.decision_reason.clone(),
        result: request
            .result
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok()),
        audit_ref: request.audit_ref.clone(),
        request_hash: request.request_hash.clone(),
        preview: with_preview
            .then(|| serde_json::from_str::<Value>(&request.preview).unwrap_or(Value::Null)),
    }
}

/// Maps an approval failure to its problem document.
fn approval_problem(err: ApprovalError) -> Problem {
    match err {
        ApprovalError::ReauthRequired => {
            Problem::new(&REAUTH_REQUIRED).with_detail("Sign in again, then approve the request.")
        }
        ApprovalError::NotPending { status } => {
            Problem::new(&NOT_PENDING).with_detail(format!("the request is {status}"))
        }
        ApprovalError::Expired => Problem::new(&EXPIRED),
        ApprovalError::NotFound => Problem::new(&NOT_FOUND),
        ApprovalError::BulkRefused(reason) => Problem::new(&BULK_REFUSED).with_detail(reason),
        ApprovalError::Forbidden => CommandError::HumanOnly.to_problem(),
        ApprovalError::Superseded => CommandError::Superseded.to_problem(),
        ApprovalError::Command(err) => err.to_problem(),
        ApprovalError::Database(msg) => CommandError::Database(DbError::Query(msg)).to_problem(),
        other => Problem::validation_failed(other.to_string()),
    }
}

/// The command-error form of an approval failure, for the read commands.
fn as_command_error(err: ApprovalError) -> CommandError {
    match err {
        ApprovalError::Command(err) => err,
        ApprovalError::Database(msg) => CommandError::Database(DbError::Query(msg)),
        other => CommandError::InvalidInput(other.to_string()),
    }
}

/// `approvals.list`: the company's approval requests, pending or decided.
pub struct ApprovalsList;

/// The input of `approvals.list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalsListInput {
    /// `pending` (the default), `decided` or `all`.
    #[serde(default)]
    pub status: Option<String>,
    /// How many requests to return: 50 by default, 200 at most.
    #[serde(default)]
    pub limit: Option<String>,
}

/// The output of `approvals.list`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ApprovalsListOutput {
    /// Requests awaiting a decision right now (not expired).
    pub pending_count: i64,
    /// The requests, newest first.
    pub items: Vec<ApprovalView>,
}

#[async_trait]
impl Command for ApprovalsList {
    const NAME: &'static str = "approvals.list";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = ApprovalsListInput;
    type Output = ApprovalsListOutput;

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
        let listing = match input.status.as_deref().unwrap_or("pending") {
            "pending" => Listing::Pending,
            "decided" => Listing::Decided,
            "all" => Listing::All,
            other => {
                return Err(CommandError::InvalidInput(format!(
                    "status must be pending, decided or all, not {other}"
                )));
            }
        };
        let limit = match input.limit.as_deref() {
            None => 50,
            Some(text) => text
                .parse::<u32>()
                .map_err(|_| CommandError::InvalidInput("limit must be a number".to_owned()))?
                .min(200),
        };
        let (requests, pending_count) =
            list_requests(db, &ctx.company_id, listing, &ctx.now, limit)
                .await
                .map_err(as_command_error)?;
        Ok(Plan::with_output(ApprovalsListOutput {
            pending_count,
            items: requests.iter().map(|r| view(r, false)).collect(),
        }))
    }
}

/// `approvals.get`: one approval request, with its journal preview.
pub struct ApprovalsGet;

/// The input of `approvals.get`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalsGetInput {
    /// The approval request id.
    pub id: String,
}

#[async_trait]
impl Command for ApprovalsGet {
    const NAME: &'static str = "approvals.get";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = ApprovalsGetInput;
    type Output = ApprovalView;

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
        let request = get_request(db, &ctx.company_id, &input.id)
            .await
            .map_err(as_command_error)?;
        Ok(Plan::with_output(view(&request, true)))
    }
}

/// The body of `approvals.decide`. The request id arrives from the path.
#[derive(Debug, Deserialize)]
pub struct DecideInput {
    /// The approval request id.
    pub id: String,
    /// `approve` or `reject`.
    pub decision: String,
    /// Why the request is rejected (recorded on the request).
    #[serde(default)]
    pub reason: Option<String>,
}

/// The body of `approvals.decide_bulk`.
#[derive(Debug, Deserialize)]
pub struct BulkDecideInput {
    /// The approval request ids to approve.
    pub ids: Vec<String>,
    /// Must be `approve`: bulk rejection is not offered.
    pub decision: String,
}

/// The credential a caller presented, as the executor takes it.
fn credential_of(presented: &Presented) -> Credential {
    match presented {
        Presented::Bearer(token) => token.clone(),
        Presented::Session(session) => session.id.clone(),
        Presented::Principal(_) => String::new(),
    }
}

/// An approver may not decide a request their own agent key raised.
pub const SEGREGATION_OF_DUTIES: ProblemDef = ProblemDef {
    slug: "segregation-of-duties",
    status: StatusCode::FORBIDDEN,
    title: "You cannot approve this request",
    description: "This request was raised through an agent key you created. Another approver decides it.",
};

impl ServerDispatch {
    fn approvals_service<'db>(
        &self,
        db: &'db dyn Database,
    ) -> Approvals<'db, OwnerApprovesAll, Es256Verifier, NoCredentialKeys> {
        // Passkey step-up is not used by the server: no credential keys are
        // registered, so that path refuses every assertion.
        Approvals::new(
            db,
            OwnerApprovesAll,
            Es256Verifier,
            NoCredentialKeys,
            ApprovalConfig::new(String::new(), self.app_url.clone(), self.app_url.clone()),
        )
    }

    fn caller_authn(&self, deps: &Deps, presented: &Presented) -> Authn {
        match presented {
            Presented::Principal(principal) => Authn::Fixed(principal.clone()),
            Presented::Bearer(_) => Authn::Key(KeyStore {
                db: Arc::clone(&deps.db),
                clock: Arc::clone(&deps.clock),
                company_id: self.company_id.clone(),
                preloaded: None,
            }),
            Presented::Session(session) => Authn::Session(SessionAuthenticator {
                db: Arc::clone(&deps.db),
                clock: Arc::clone(&deps.clock),
                app_origins: self.app_origins.clone(),
                bootstrap: self.is_home(),
                company_id: self.company_id.clone(),
                presented: session.clone(),
            }),
        }
    }

    /// When the caller last signed in, for a session; `None` for a key.
    async fn signed_in_at(
        &self,
        deps: &Deps,
        presented: &Presented,
    ) -> Result<Option<i64>, CommandError> {
        match presented {
            Presented::Bearer(_) | Presented::Principal(_) => Ok(None),
            Presented::Session(session) => Ok(load_session(deps.db.as_ref(), &session.id)
                .await
                .map_err(CommandError::Database)?
                .and_then(|row| row.auth_time)),
        }
    }

    /// Refuses any caller that is not a signed-in person, and a person whose
    /// company role may not decide approvals (docs/permissions.md).
    async fn require_person(&self, deps: &Deps, presented: &Presented) -> Result<(), CommandError> {
        let authn = self.caller_authn(deps, presented);
        match authn.authenticate(&credential_of(presented)).await? {
            ledgers_commands::Principal::Person { role, .. } => {
                ledgers_policy::roles::check(role, "approvals.decide")
                    .map_err(|_| CommandError::PolicyDenied)
            }
            ledgers_commands::Principal::Agent { .. } => Err(CommandError::HumanOnly),
        }
    }

    fn inbox_context(&self, deps: &Deps, request_id: &str) -> CommandContext {
        let now_at = deps.clock.now();
        CommandContext {
            company_id: self.company_id.clone(),
            request_id: request_id.to_owned(),
            event_id: deps.id_gen.ulid(),
            now: super::rfc3339_utc(now_at),
            is_person: true, // Approvals are always from person (owner)
        }
    }

    /// Approves one stored request, returning its updated view and the entry
    /// reference the command produced.
    async fn approve_stored(
        &self,
        deps: &Deps,
        presented: &Presented,
        request_id: &str,
        approval_id: &str,
    ) -> Result<(ApprovalView, Value), ApprovalError> {
        let db = deps.db.as_ref();
        let request = get_request(db, &self.company_id, approval_id).await?;
        let policy = load_policy(db, &self.company_id, deps.clock.now())
            .await
            .map_err(ApprovalError::Command)?;
        let signed_in_at = self
            .signed_in_at(deps, presented)
            .await
            .map_err(ApprovalError::Command)?;
        let authn = self.caller_authn(deps, presented);
        // Segregation of duties: a person may not approve a request that their
        // own agent key raised. The key's owner is the person who created it.
        let approver = authn
            .authenticate(&credential_of(presented))
            .await
            .map_err(ApprovalError::Command)?;
        if let ledgers_commands::Principal::Person { person_id, .. } = &approver
            && policy
                .agent_keys()
                .get(&request.requested_by)
                .is_some_and(|key| key.owner == *person_id)
        {
            return Err(ApprovalError::Command(CommandError::Domain(Problem::new(
                &SEGREGATION_OF_DUTIES,
            ))));
        }
        // The approval authorizes the command it was raised for, so the
        // approver's role is not checked against it again.
        let policy = policy.for_approval();
        let executor = Executor::new(db, self.caller_authn(deps, presented), policy);
        let approvals = self.approvals_service(db);
        let credential = credential_of(presented);
        let ctx = self.inbox_context(deps, request_id);

        let result = match request.command.as_str() {
            "entries.post" => {
                run_stored(
                    &approvals,
                    &executor,
                    &authn,
                    &EntriesPost,
                    credential,
                    ctx,
                    approval_id,
                    signed_in_at,
                )
                .await?
            }
            "entries.reverse" => {
                run_stored(
                    &approvals,
                    &executor,
                    &authn,
                    &EntriesReverse,
                    credential,
                    ctx,
                    approval_id,
                    signed_in_at,
                )
                .await?
            }
            other => {
                return Err(ApprovalError::Other(format!(
                    "{other} has no approval path"
                )));
            }
        };
        let updated = get_request(db, &self.company_id, approval_id).await?;
        Ok((view(&updated, false), result))
    }

    async fn reject_stored(
        &self,
        deps: &Deps,
        presented: &Presented,
        request_id: &str,
        approval_id: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalView, ApprovalError> {
        let db = deps.db.as_ref();
        let policy = load_policy(db, &self.company_id, deps.clock.now())
            .await
            .map_err(ApprovalError::Command)?;
        let authn = self.caller_authn(deps, presented);
        let executor = Executor::new(db, self.caller_authn(deps, presented), policy);
        let ctx = self.inbox_context(deps, request_id);
        self.approvals_service(db)
            .reject_signed_in(
                &executor,
                &authn,
                credential_of(presented),
                approval_id,
                &ctx,
                reason,
            )
            .await?;
        let updated = get_request(db, &self.company_id, approval_id).await?;
        Ok(view(&updated, false))
    }

    /// `approvals.decide`: approve or reject one request.
    pub(super) async fn approvals_decide(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: Problem| err.instance(request_id);
        let presented = presented
            .ok_or_else(|| CommandError::Unauthenticated.to_problem())
            .map_err(fail)?;
        let input: DecideInput = serde_json::from_value(input)
            .map_err(|err| fail(Problem::validation_failed(err.to_string())))?;
        self.require_person(deps, &presented)
            .await
            .map_err(|err| fail(err.to_problem()))?;
        match input.decision.as_str() {
            "approve" => {
                let (approval, result) = self
                    .approve_stored(deps, &presented, request_id, &input.id)
                    .await
                    .map_err(|err| fail(approval_problem(err)))?;
                Ok(json!({ "approval": approval, "result": result }))
            }
            "reject" => {
                let approval = self
                    .reject_stored(
                        deps,
                        &presented,
                        request_id,
                        &input.id,
                        input.reason.as_deref(),
                    )
                    .await
                    .map_err(|err| fail(approval_problem(err)))?;
                Ok(json!({ "approval": approval }))
            }
            other => Err(fail(Problem::validation_failed(format!(
                "decision must be approve or reject, not {other}"
            )))),
        }
    }

    /// `approvals.decide_bulk`: approve several low-risk requests in one currency.
    /// Every selected request is checked before any is approved, so a mixed
    /// selection is refused with nothing written.
    pub(super) async fn approvals_decide_bulk(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let fail = |err: Problem| err.instance(request_id);
        let presented = presented
            .ok_or_else(|| CommandError::Unauthenticated.to_problem())
            .map_err(fail)?;
        let input: BulkDecideInput = serde_json::from_value(input)
            .map_err(|err| fail(Problem::validation_failed(err.to_string())))?;
        self.require_person(deps, &presented)
            .await
            .map_err(|err| fail(err.to_problem()))?;
        if input.decision != "approve" {
            return Err(fail(Problem::validation_failed(
                "bulk decisions are approvals only",
            )));
        }
        let mut selected = Vec::with_capacity(input.ids.len());
        for id in &input.ids {
            let request = get_request(deps.db.as_ref(), &self.company_id, id)
                .await
                .map_err(|err| fail(approval_problem(err)))?;
            selected.push(request);
        }
        check_bulk(&selected).map_err(|err| fail(approval_problem(err)))?;

        let mut results = Vec::with_capacity(input.ids.len());
        for id in &input.ids {
            match self.approve_stored(deps, &presented, request_id, id).await {
                Ok((approval, result)) => {
                    results.push(json!({ "id": id, "approval": approval, "result": result }));
                }
                Err(err) => {
                    let problem = approval_problem(err);
                    results.push(json!({ "id": id, "error": problem.slug }));
                }
            }
        }
        Ok(json!({ "results": results }))
    }
}

/// Approves a stored request with the command it was raised for.
#[allow(clippy::too_many_arguments)]
async fn run_stored<C: Command>(
    approvals: &Approvals<'_, OwnerApprovesAll, Es256Verifier, NoCredentialKeys>,
    executor: &Executor<'_, Authn, impl Policy>,
    authn: &Authn,
    cmd: &C,
    credential: Credential,
    ctx: CommandContext,
    approval_id: &str,
    signed_in_at: Option<i64>,
) -> Result<Value, ApprovalError> {
    let output = approvals
        .approve_signed_in(
            executor,
            authn,
            cmd,
            credential,
            ctx,
            approval_id,
            signed_in_at,
        )
        .await?;
    serde_json::to_value(output).map_err(ApprovalError::from)
}
