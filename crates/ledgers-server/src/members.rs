//! The company's organization: its members, roles and invitations
//! (docs/permissions.md).
//!
//! The company is one organization from `cratefield-module-orgs`, whose id is
//! the company id. Its routes are mounted at `/v1/orgs` through
//! [`MembersModule`], which is the module with two rules of Ledgers' own: no
//! organization creation (the company's organization is made by the first
//! owner's sign-in, [`resolve_role`]), and the session's Origin check for
//! state-changing calls made with a cookie.
//!
//! The person behind a session is identified for the module by
//! [`SessionIdentity`], the harness `Auth` port. It names the signed-in person
//! and nothing else: what that person may do is their membership role.

use crate::people::{check_session_origin, cookie_value, load_session};
use async_trait::async_trait;
use cratefield_core::axum::{
    self,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use cratefield_core::{
    AnyError, Auth, AuthError, BoxFuture, Caller, Clock, Config, ConfigError, Database, DbError,
    Migrations, Module, ModuleContext, PersonalDataSet, Port, Problem, ProblemDef, Statement,
    Subject, Surface,
};
use cratefield_module_orgs::Orgs;
use ledgers_auth::session::{SESSION_COOKIE, session_key_hash};
use ledgers_commands::Role;
use sea_query::Value;
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The membership roles the company uses, in the order the matrix lists them.
pub const ROLE_NAMES: [&str; 6] = [
    "owner",
    "admin",
    "accountant",
    "approver",
    "viewer",
    "auditor",
];

/// The non-owner roles that manage members and invitations.
pub const MANAGER_ROLES: [&str; 1] = ["admin"];

/// Whether a role manages members and invitations: owners always, and the
/// managers the organization was configured with ([`MANAGER_ROLES`]).
#[must_use]
pub fn may_manage(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Admin)
}

/// The name the company's organization is created with. The company's own
/// name lives in its settings; the organization's is only a label.
pub const COMPANY_ORG_NAME: &str = "Company";

/// The organization module, configured for the company: the six roles, and
/// `admin` as the role that manages people (owners always do).
#[must_use]
pub fn company_orgs() -> Orgs {
    Orgs::builder()
        .roles(ROLE_NAMES)
        .managers(MANAGER_ROLES)
        .build()
}

/// The invitation mailer: Owlpost, from `ORGS_FROM` (a `send.` address on
/// staging and production). With no `OWLPOST_API_KEY` the mailer reports
/// `NotConfigured`, and the module answers invitations with
/// `503 mail-not-configured` instead of leaving a row that was never mailed.
#[must_use]
pub fn invitation_mailer(
    cfg: &dyn Config,
    http: Arc<dyn cratefield_core::HttpClient>,
    clock: Arc<dyn Clock>,
) -> cratefield_adapter_owlpost::Owlpost {
    let from = cfg.get("ORGS_FROM").unwrap_or_default();
    let mailer = cratefield_adapter_owlpost::Owlpost::new(
        http,
        clock,
        cfg.get("OWLPOST_API_KEY"),
        from,
        cfg.get("ORGS_REPLY_TO"),
    );
    match cfg.get("OWLPOST_BASE_URL") {
        Some(base) if !base.trim().is_empty() => mailer.with_base_url(base),
        _ => mailer,
    }
}

/// The Auth and Mailer ports the organization module needs on the native
/// runtime, added to `native`: the session identity over `db`, and the
/// invitation mailer. Every native composition of the company goes through this,
/// so `ledgers serve`, `ledgers demo init` and the server agree.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn with_native_members_ports(
    native: cratefield_runtime_native::Native,
    db: Arc<dyn Database>,
    cfg: &dyn Config,
) -> cratefield_runtime_native::Native {
    let clock: Arc<dyn Clock> = Arc::new(cratefield_runtime_native::TokioClock);
    let http: Arc<dyn cratefield_core::HttpClient> =
        Arc::new(cratefield_core::BoundedHttpClient::new(
            Arc::new(cratefield_runtime_native::ReqwestClient::new()),
            Arc::clone(&clock),
        ));
    native
        .auth_arc(Arc::new(SessionIdentity::new(db, Arc::clone(&clock))))
        .mailer(invitation_mailer(cfg, http, clock))
}

/// Creating an organization is not offered: the company's organization is made
/// by its first owner's sign-in.
pub const ORG_CREATE_NOT_OFFERED: ProblemDef = ProblemDef {
    slug: "org-create-not-offered",
    status: StatusCode::NOT_FOUND,
    title: "Not offered",
    description: "Organizations are created by the first owner's sign-in; there is nothing to create here.",
};

// ---------------------------------------------------------------------------
// The identity port
// ---------------------------------------------------------------------------

/// The harness `Auth` port for people signed in with the session cookie. A
/// request with no cookie is anonymous; a bearer credential (an agent key) is
/// anonymous too, because agents are not members; an expired or unknown
/// session did not verify.
pub struct SessionIdentity {
    db: Arc<dyn Database>,
    clock: Arc<dyn Clock>,
}

impl SessionIdentity {
    /// The port over `db`, with `clock` deciding expiry.
    #[must_use]
    pub fn new(db: Arc<dyn Database>, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }
}

#[async_trait]
impl Auth for SessionIdentity {
    async fn identify(&self, headers: &HeaderMap) -> Result<Caller, AuthError> {
        if headers.get(header::AUTHORIZATION).is_some() {
            return Ok(Caller::Anonymous);
        }
        let Some(id) = cookie_value(headers, SESSION_COOKIE) else {
            return Ok(Caller::Anonymous);
        };
        let now = self.clock.now().unix_timestamp();
        let row = load_session(self.db.as_ref(), &id)
            .await
            .map_err(|err| AuthError::Unavailable(err.to_string()))?;
        match row {
            Some(row) if !row.expired_at(now) => {
                let mut subject = Subject::new(row.person_id);
                subject.email = row.email;
                subject.session = session_key_hash(&id);
                Ok(Caller::Subject(subject))
            }
            _ => Err(AuthError::NotVerified),
        }
    }
}

// ---------------------------------------------------------------------------
// The module wrapper
// ---------------------------------------------------------------------------

/// The organization module as the company mounts it: `cratefield-module-orgs`
/// at `/v1/orgs`, with the creation route refused and the Origin rule applied
/// to cookie-authenticated changes.
pub struct MembersModule {
    orgs: Orgs,
    app_origins: Vec<String>,
}

impl MembersModule {
    /// The module for the company, with `app_origins` the origins a
    /// cookie-authenticated change may come from.
    #[must_use]
    pub fn new(app_origins: Vec<String>) -> Self {
        Self {
            orgs: company_orgs(),
            app_origins,
        }
    }
}

/// What the guard middleware checks.
#[derive(Clone)]
struct Guard {
    app_origins: Vec<String>,
}

/// Refuses organization creation, and refuses a cookie-authenticated change
/// whose Origin is not an app origin. Reads and bearer calls pass through.
async fn guard(State(guard): State<Arc<Guard>>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    if request.method() == Method::POST && (path == "/" || path.is_empty()) {
        return Problem::new(&ORG_CREATE_NOT_OFFERED).into_response();
    }
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let bearer = request.headers().get(header::AUTHORIZATION).is_some();
    if !safe && !bearer && cookie_value(request.headers(), SESSION_COOKIE).is_some() {
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if let Err(problem) = check_session_origin(&guard.app_origins, origin.as_deref(), false) {
            return problem.into_response();
        }
    }
    next.run(request).await
}

impl Module for MembersModule {
    fn name(&self) -> &'static str {
        self.orgs.name()
    }

    fn version(&self) -> &'static str {
        self.orgs.version()
    }

    fn requires(&self) -> &'static [Port] {
        self.orgs.requires()
    }

    fn optional(&self) -> &'static [Port] {
        self.orgs.optional()
    }

    fn tables(&self) -> &'static [&'static str] {
        self.orgs.tables()
    }

    fn personal_data(&self) -> &'static [PersonalDataSet] {
        self.orgs.personal_data()
    }

    fn migrations(&self) -> Migrations {
        self.orgs.migrations()
    }

    fn validate_config(&self, cfg: &dyn Config) -> Result<(), ConfigError> {
        self.orgs.validate_config(cfg)
    }

    fn self_check(&self) -> Vec<String> {
        self.orgs.self_check()
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let guard_state = Arc::new(Guard {
            app_origins: self.app_origins.clone(),
        });
        self.orgs
            .router(ctx)
            .layer(middleware::from_fn_with_state(guard_state, guard))
    }

    fn surface(&self) -> Surface {
        self.orgs.surface()
    }

    fn scheduled<'a>(
        &'a self,
        ctx: &'a ModuleContext,
        cron: &'a str,
    ) -> BoxFuture<'a, Result<(), AnyError>> {
        self.orgs.scheduled(ctx, cron)
    }
}

// ---------------------------------------------------------------------------
// Membership and bootstrap
// ---------------------------------------------------------------------------

/// The membership role of `person_id` in the company organization, or `None`
/// when they are not a member. A stored role this build does not know is
/// treated as no membership: the answer fails closed.
///
/// # Errors
/// The database error.
pub async fn role_of(
    db: &dyn Database,
    company_id: &str,
    person_id: &str,
) -> Result<Option<Role>, DbError> {
    let stored = cratefield_module_orgs::member_role(db, company_id, person_id)
        .await
        .map_err(|err| match err {
            cratefield_module_orgs::OrgsError::Db(db) => db,
            other => DbError::Query(other.to_string()),
        })?;
    Ok(stored.and_then(|role| role.parse::<Role>().ok()))
}

/// The role a signed-in person holds in the company, applying the bootstrap:
/// when the person's sign-in carries the `owner` role and the company has no
/// owner yet, they become its owner. After that the claim is not read again:
/// roles are membership roles.
///
/// # Errors
/// The database error.
pub async fn resolve_role(
    db: &dyn Database,
    company_id: &str,
    person_id: &str,
    claim_roles: &[String],
    now: OffsetDateTime,
) -> Result<Option<Role>, DbError> {
    if let Some(role) = role_of(db, company_id, person_id).await? {
        return Ok(Some(role));
    }
    if claim_roles.iter().any(|role| role == "owner") {
        bootstrap_owner(db, company_id, person_id, now).await?;
        return role_of(db, company_id, person_id).await;
    }
    Ok(None)
}

/// Makes the company's organization, and `person_id` its owner, when the
/// organization has no owner. One batch: the organization, the promotion of an
/// existing member and the insert of a new owner are each guarded by "no owner
/// yet", so two sign-ins racing here leave exactly one owner.
///
/// # Errors
/// The database error.
pub async fn bootstrap_owner(
    db: &dyn Database,
    company_id: &str,
    person_id: &str,
    now: OffsetDateTime,
) -> Result<(), DbError> {
    let at = now
        .format(&Rfc3339)
        .map_err(|err| DbError::Query(err.to_string()))?;
    let text = |value: &str| Value::from(value.to_owned());
    let statements = vec![
        Statement::with_values(
            "INSERT INTO orgs (id, name, created_by, created_at) SELECT ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM orgs WHERE id = ?)",
            vec![
                text(company_id),
                text(COMPANY_ORG_NAME),
                text(person_id),
                text(&at),
                text(company_id),
            ],
        ),
        Statement::with_values(
            "UPDATE org_members SET role = 'owner' WHERE org_id = ? AND user_sub = ? AND NOT EXISTS (SELECT 1 FROM org_members WHERE org_id = ? AND role = 'owner')",
            vec![text(company_id), text(person_id), text(company_id)],
        ),
        Statement::with_values(
            "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) SELECT ?, ?, 'owner', NULL, ? WHERE NOT EXISTS (SELECT 1 FROM org_members WHERE org_id = ? AND role = 'owner') AND NOT EXISTS (SELECT 1 FROM org_members WHERE org_id = ? AND user_sub = ?)",
            vec![
                text(company_id),
                text(person_id),
                text(&at),
                text(company_id),
                text(company_id),
                text(person_id),
            ],
        ),
    ];
    db.batch_atomic(&statements).await
}

// ---------------------------------------------------------------------------
// People, profiles and invitations
// ---------------------------------------------------------------------------

/// One member, with what the company shows about them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRow {
    pub person_id: String,
    pub email: Option<String>,
    pub role: String,
    pub invited_by: Option<String>,
    pub joined_at: String,
    /// When the person last used the app (epoch seconds), when recorded.
    pub last_active_at: Option<i64>,
}

/// Every member of the company, in one query.
///
/// # Errors
/// The database error.
pub async fn members(db: &dyn Database, company_id: &str) -> Result<Vec<MemberRow>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT m.user_sub AS person_id, m.role AS role, m.invited_by AS invited_by, m.created_at AS joined_at, p.email AS email, p.last_seen_at AS last_seen_at FROM org_members m LEFT JOIN people_profile p ON p.person_id = m.user_sub WHERE m.org_id = ? ORDER BY m.created_at ASC, m.user_sub ASC",
            vec![Value::from(company_id.to_owned())],
        ))
        .await?;
    Ok(rows
        .rows
        .iter()
        .map(|row| MemberRow {
            person_id: row.get::<String>("person_id").unwrap_or_default(),
            email: row.get::<String>("email"),
            role: row.get::<String>("role").unwrap_or_default(),
            invited_by: row.get::<String>("invited_by"),
            joined_at: row.get::<String>("joined_at").unwrap_or_default(),
            last_active_at: row.get::<i64>("last_seen_at"),
        })
        .collect())
}

/// One invitation that has not been accepted and has not lapsed. The address
/// is not here: the organization module keeps only its hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInvitation {
    pub id: String,
    pub role: String,
    pub invited_by: String,
    pub created_at: String,
    pub expires_at: String,
}

/// The invitations still open at `now`.
///
/// # Errors
/// The database error.
pub async fn pending_invitations(
    db: &dyn Database,
    company_id: &str,
    now: OffsetDateTime,
) -> Result<Vec<PendingInvitation>, DbError> {
    let at = now
        .format(&Rfc3339)
        .map_err(|err| DbError::Query(err.to_string()))?;
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, role, invited_by, created_at, expires_at FROM org_invitations WHERE org_id = ? AND accepted_at IS NULL AND expires_at > ? ORDER BY created_at DESC, id DESC",
            vec![Value::from(company_id.to_owned()), Value::from(at)],
        ))
        .await?;
    Ok(rows
        .rows
        .iter()
        .map(|row| PendingInvitation {
            id: row.get::<String>("id").unwrap_or_default(),
            role: row.get::<String>("role").unwrap_or_default(),
            invited_by: row.get::<String>("invited_by").unwrap_or_default(),
            created_at: row.get::<String>("created_at").unwrap_or_default(),
            expires_at: row.get::<String>("expires_at").unwrap_or_default(),
        })
        .collect())
}

/// Revokes an open invitation. `false` when there is no such open invitation,
/// so a revoked or accepted one answers the same as an unknown one.
///
/// # Errors
/// The database error.
pub async fn revoke_invitation(
    db: &dyn Database,
    company_id: &str,
    invitation_id: &str,
) -> Result<bool, DbError> {
    let changed = db
        .execute(&Statement::with_values(
            "DELETE FROM org_invitations WHERE org_id = ? AND id = ? AND accepted_at IS NULL",
            vec![
                Value::from(company_id.to_owned()),
                Value::from(invitation_id.to_owned()),
            ],
        ))
        .await?;
    Ok(changed > 0)
}

/// Records that `person_id` was seen at `now` (epoch seconds) with `email`, so
/// the member list can show it. One batch, so the row is inserted once.
///
/// # Errors
/// The database error.
pub async fn touch_profile(
    db: &dyn Database,
    person_id: &str,
    email: Option<&str>,
    now: i64,
) -> Result<(), DbError> {
    let email_value = Value::String(email.map(|e| Box::new(e.to_owned())));
    let statements = vec![
        Statement::with_values(
            "UPDATE people_profile SET email = ?, last_seen_at = ? WHERE person_id = ?",
            vec![
                email_value.clone(),
                Value::from(now),
                Value::from(person_id.to_owned()),
            ],
        ),
        Statement::with_values(
            "INSERT INTO people_profile (person_id, email, last_seen_at) SELECT ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM people_profile WHERE person_id = ?)",
            vec![
                Value::from(person_id.to_owned()),
                email_value,
                Value::from(now),
                Value::from(person_id.to_owned()),
            ],
        ),
    ];
    db.batch_atomic(&statements).await
}
