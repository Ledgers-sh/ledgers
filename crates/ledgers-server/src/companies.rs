//! The companies a signed-in person belongs to, and creating a new one
//! (docs/permissions.md, "Companies").
//!
//! One deployment serves many companies. A company is one `company` row plus
//! one organization from `cratefield-module-orgs` with the same id; a person
//! belongs to a company exactly when they hold a membership in its
//! organization. Routes:
//!
//! - `GET /v1/companies`: the companies the session's person belongs to, with
//!   their role. Cookie sessions only; agent keys are not people.
//! - `POST /v1/companies`: create a company. The creator becomes its owner.
//!   Cookie sessions only, with an allowed Origin.
//!
//! Commands pick their company with the `X-Ledgers-Company` header
//! ([`crate::routes::company_dispatch`]); this module only lists and creates.

use crate::dispatch::{Deps, chart_for_pack, seed_company_chart};
use crate::people::{check_session_origin, cookie_value, load_session};
use cratefield_core::axum::{
    self,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use cratefield_core::{
    Config, ConfigError, Database, Migrations, Module, ModuleContext, Port, Problem, ProblemDef,
    Statement,
};
use ledgers_auth::session::SESSION_COOKIE;
use ledgers_money::Currency;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The longest company name, in characters.
pub const COMPANY_NAME_MAX: usize = 200;

/// The company-setting keys a new company records at creation.
pub const SETTING_COUNTRY_PACK: &str = "country_pack";
/// The fiscal year's first month, 1 to 12.
pub const SETTING_FISCAL_YEAR_START: &str = "fiscal_year_start_month";

/// The ports the companies routes need.
pub const COMPANIES_PORTS: &[Port] = &[Port::Db, Port::Clock, Port::IdGen];

/// The companies module: `GET` and `POST` at `/v1/companies`.
pub struct CompaniesModule {
    app_origins: Vec<String>,
}

impl CompaniesModule {
    /// The module, with `app_origins` the origins a cookie-authenticated
    /// creation may come from.
    #[must_use]
    pub fn new(app_origins: Vec<String>) -> Self {
        Self { app_origins }
    }
}

impl Module for CompaniesModule {
    fn name(&self) -> &'static str {
        "companies"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        COMPANIES_PORTS
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let deps = Deps::from_ports(&ctx.ports)
            .expect("the companies ports (db, clock, id_gen) are declared in requires()");
        let state = Arc::new(CompaniesState {
            deps,
            app_origins: self.app_origins.clone(),
        });
        axum::Router::new()
            .route("/", get(list_companies).post(create_company))
            .with_state(state)
    }

    fn migrations(&self) -> Migrations {
        // The tables are the company, organization and setting tables the
        // storage and organization modules already own.
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

struct CompaniesState {
    deps: Deps,
    app_origins: Vec<String>,
}

/// 401: no valid person session on the request.
static SESSION_NEEDED: ProblemDef = ProblemDef {
    slug: "session-required",
    status: StatusCode::UNAUTHORIZED,
    title: "Sign-in required",
    description: "Creating or listing companies needs a signed-in person.",
};

/// 422: the company input does not pass its checks.
static INVALID_COMPANY: ProblemDef = ProblemDef {
    slug: "invalid-company-input",
    status: StatusCode::UNPROCESSABLE_ENTITY,
    title: "Invalid company",
    description: "The company needs a name of 1 to 200 characters, an ISO 4217 base currency, a bundled country pack and a fiscal year start month from 1 to 12.",
};

/// 500: the company was created but its chart could not be seeded. The
/// company stays; a second run of the seed would finish it.
static SEED_FAILED: ProblemDef = ProblemDef {
    slug: "company-seed-failed",
    status: StatusCode::INTERNAL_SERVER_ERROR,
    title: "Company chart not created",
    description: "The company row was stored but its chart could not be created. The details are in the server log.",
};

/// One company the person belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySummary {
    pub id: String,
    pub name: String,
    pub base_currency: String,
    pub role: String,
}

/// The body of `POST /v1/companies`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCompany {
    pub name: String,
    pub base_currency: String,
    pub country_pack: String,
    pub fiscal_year_start_month: u8,
}

impl CreateCompany {
    /// Checks the fields; the country pack must be one the server bundles.
    ///
    /// # Errors
    /// A message naming the field that failed.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > COMPANY_NAME_MAX {
            return Err(format!("name must be 1 to {COMPANY_NAME_MAX} characters"));
        }
        match Currency::from_code(&self.base_currency) {
            Ok(currency) if currency.code() == self.base_currency => {}
            _ => return Err("baseCurrency must be an uppercase ISO 4217 code".to_owned()),
        }
        if ledgers_tax::country::pack_by_id(&self.country_pack).is_err() {
            return Err("countryPack must be a bundled country pack".to_owned());
        }
        if !(1..=12).contains(&self.fiscal_year_start_month) {
            return Err("fiscalYearStartMonth must be 1 to 12".to_owned());
        }
        Ok(())
    }
}

/// The signed-in person behind a cookie session, or the 401.
async fn session_person(deps: &Deps, headers: &HeaderMap) -> Result<String, Problem> {
    let Some(id) = cookie_value(headers, SESSION_COOKIE) else {
        return Err(Problem::new(&SESSION_NEEDED));
    };
    let now = deps.clock.now().unix_timestamp();
    load_session(deps.db.as_ref(), &id)
        .await
        .map_err(|err| Problem::new(&SESSION_NEEDED).with_detail(err.to_string()))?
        .filter(|row| !row.expired_at(now))
        .map(|row| row.person_id)
        .ok_or_else(|| Problem::new(&SESSION_NEEDED))
}

/// The companies `person_id` holds a membership in, by name.
///
/// # Errors
/// The database error.
pub async fn companies_of(
    db: &dyn Database,
    person_id: &str,
) -> Result<Vec<CompanySummary>, cratefield_core::DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT c.id, c.name, c.base_currency, m.role FROM company c \
             JOIN org_members m ON m.org_id = c.id \
             WHERE m.user_sub = ? ORDER BY c.name, c.id"
                .to_owned(),
            vec![Value::from(person_id.to_owned())],
        ))
        .await?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| {
            Some(CompanySummary {
                id: row.get("id")?,
                name: row.get("name")?,
                base_currency: row.get("base_currency")?,
                role: row.get("role")?,
            })
        })
        .collect())
}

async fn list_companies(State(state): State<Arc<CompaniesState>>, headers: HeaderMap) -> Response {
    let person = match session_person(&state.deps, &headers).await {
        Ok(person) => person,
        Err(problem) => return problem.into_response(),
    };
    match companies_of(state.deps.db.as_ref(), &person).await {
        Ok(companies) => (
            StatusCode::OK,
            axum::Json(json!({ "companies": companies })),
        )
            .into_response(),
        Err(err) => Problem::new(&SESSION_NEEDED)
            .with_detail(err.to_string())
            .into_response(),
    }
}

async fn create_company(
    State(state): State<Arc<CompaniesState>>,
    headers: HeaderMap,
    body: Result<axum::Json<CreateCompany>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Err(problem) = check_session_origin(&state.app_origins, origin.as_deref(), false) {
        return problem.into_response();
    }
    let person = match session_person(&state.deps, &headers).await {
        Ok(person) => person,
        Err(problem) => return problem.into_response(),
    };
    let axum::Json(input) = match body {
        Ok(body) => body,
        Err(err) => {
            return Problem::new(&INVALID_COMPANY)
                .with_detail(err.to_string())
                .into_response();
        }
    };
    if let Err(detail) = input.validate() {
        return Problem::new(&INVALID_COMPANY)
            .with_detail(detail)
            .into_response();
    }
    match create(&state.deps, &person, &input).await {
        Ok(summary) => (StatusCode::CREATED, axum::Json(summary)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

/// Creates a company for `creator`: the company row, its organization with
/// the creator as owner, its country and fiscal settings, and its chart and
/// general journal. The membership rows commit in one batch; the chart is
/// seeded after it, and the seed is idempotent by the chart's existence.
///
/// # Errors
/// A problem for a database failure or a chart that could not be seeded.
pub async fn create(
    deps: &Deps,
    creator: &str,
    input: &CreateCompany,
) -> Result<CompanySummary, Problem> {
    let company_id = deps.id_gen.ulid();
    let journal_id = deps.id_gen.ulid();
    let name = input.name.trim().to_owned();
    let stamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|err| Problem::new(&SEED_FAILED).with_detail(err.to_string()))?;
    let text = |value: &str| Value::from(value.to_owned());
    let statements = vec![
        Statement::with_values(
            "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            vec![
                text(&company_id),
                text(&name),
                text(&input.base_currency),
                text(&stamp),
                text(&stamp),
            ],
        ),
        Statement::with_values(
            "INSERT INTO orgs (id, name, created_by, created_at) VALUES (?, ?, ?, ?)",
            vec![text(&company_id), text(&name), text(creator), text(&stamp)],
        ),
        Statement::with_values(
            "INSERT INTO org_members (org_id, user_sub, role, invited_by, created_at) VALUES (?, ?, 'owner', NULL, ?)",
            vec![text(&company_id), text(creator), text(&stamp)],
        ),
        Statement::with_values(
            "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, ?, ?, ?)",
            vec![
                text(&company_id),
                text(SETTING_COUNTRY_PACK),
                text(&input.country_pack),
                text(&stamp),
            ],
        ),
        Statement::with_values(
            "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, ?, ?, ?)",
            vec![
                text(&company_id),
                text(SETTING_FISCAL_YEAR_START),
                text(&input.fiscal_year_start_month.to_string()),
                text(&stamp),
            ],
        ),
    ];
    deps.db
        .batch_atomic(&statements)
        .await
        .map_err(|err| Problem::new(&SEED_FAILED).with_detail(err.to_string()))?;
    let chart = chart_for_pack(&input.country_pack)
        .map_err(|err| Problem::new(&SEED_FAILED).with_detail(err))?;
    seed_company_chart(
        deps,
        &company_id,
        creator,
        &journal_id,
        &chart,
        input.fiscal_year_start_month,
    )
    .await
    .map_err(|err| Problem::new(&SEED_FAILED).with_detail(err))?;
    Ok(CompanySummary {
        id: company_id,
        name,
        base_currency: input.base_currency.clone(),
        role: "owner".to_owned(),
    })
}
