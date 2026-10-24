//! The Google Cloud connector (issue #142, cloud providers).
//!
//! Cost: the Cloud Billing API cannot return costs, so the cost comes from the
//! Cloud Billing **export to `BigQuery`** (the standard usage cost table). The
//! connection names the table (`project`, `dataset`, `table`); the query is run
//! with the `BigQuery` REST `jobs.query`, and its rows are paged with
//! `getQueryResults`. Source: <https://cloud.google.com/bigquery/docs/reference/rest/v2/jobs/query>
//!
//! The query groups the export by month, currency and project. A row's net cost
//! is `cost` plus its `credits`, summed as NUMERIC (the export's `cost` is a
//! FLOAT64, so each row is converted to NUMERIC before it is summed).
//!
//! Auth: a service account. Ledgers signs an RS256 JWT with the account's key
//! (`ring`, which builds for wasm32 with `wasm32_unknown_unknown_js`) and
//! exchanges it at `https://oauth2.googleapis.com/token` for an access token
//! with the `bigquery.readonly` scope. The account needs `BigQuery` Data Viewer on
//! the export dataset and `BigQuery` Job User on the project that runs the query.
//!
//! Allocation: by project id. A row with no project books to `unattributed`.

use crate::connector::{
    Connector, CredentialField, SyncContext, credential_text, is_date, next_day,
};
use crate::cost::{CostLine, epoch_seconds, micro_of_value, month_of, output};
use crate::http::{VendorError, form_body, percent_encode};
use crate::model::SyncOutput;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::{Value, json};

const BIGQUERY_HOST: &str = "bigquery.googleapis.com";
const TOKEN_HOST: &str = "oauth2.googleapis.com";
const BIGQUERY_HOSTS: &[&str] = &[BIGQUERY_HOST];
const TOKEN_HOSTS: &[&str] = &[TOKEN_HOST];
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/bigquery.readonly";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// Rows per page.
const PAGE: u32 = 10_000;
/// Waits (each up to `POLL_MS`) for a query job that has not finished.
const MAX_POLLS: u32 = 20;
const POLL_MS: u32 = 10_000;
/// Pages read per query.
const MAX_PAGES: u32 = 100;

/// The Google Cloud connector.
pub struct GoogleCloud;

#[async_trait]
impl Connector for GoogleCloud {
    fn id(&self) -> &'static str {
        "gcp"
    }

    fn display_name(&self) -> &'static str {
        "Google Cloud"
    }

    fn credential_fields(&self) -> &'static [CredentialField] {
        &[CredentialField {
            name: "service_account_json",
            label: "Service account key (JSON)",
        }]
    }

    fn allowed_hosts(&self) -> &'static [&'static str] {
        &[BIGQUERY_HOST, TOKEN_HOST]
    }

    fn validate_config(&self, config: &Value) -> Result<(), String> {
        let project = text(config, "project")?;
        if !valid_project(project) {
            return Err("project is the Google Cloud project id that runs the query".to_owned());
        }
        if !valid_identifier(text(config, "dataset")?, true) {
            return Err("dataset is the BigQuery dataset of the billing export".to_owned());
        }
        if !valid_identifier(text(config, "table")?, false) {
            return Err("table is the billing export table name".to_owned());
        }
        Ok(())
    }

    fn completed_periods_only(&self) -> bool {
        true
    }

    async fn sync(&self, ctx: &SyncContext<'_>) -> Result<SyncOutput, VendorError> {
        let key_json = credential_text(ctx.credential, "service_account_json")
            .map_err(|detail| malformed(&detail))?;
        let account: Value = serde_json::from_str(&key_json)
            .map_err(|_| malformed("the service account key is not JSON"))?;
        let email = account
            .get("client_email")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("the service account key has no client_email"))?;
        let pem = account
            .get("private_key")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("the service account key has no private_key"))?;
        let now = epoch_seconds(ctx.now).ok_or_else(|| malformed("the clock is not RFC 3339"))?;
        let assertion = signed_assertion(email, pem, now).map_err(|detail| malformed(&detail))?;
        let token = access_token(ctx, &assertion).await?;

        let project = text(ctx.config, "project").map_err(|detail| malformed(&detail))?;
        let dataset = text(ctx.config, "dataset").map_err(|detail| malformed(&detail))?;
        let table = text(ctx.config, "table").map_err(|detail| malformed(&detail))?;
        if !valid_project(project)
            || !valid_identifier(dataset, true)
            || !valid_identifier(table, false)
        {
            return Err(malformed(
                "the connection's project, dataset or table is not a valid name",
            ));
        }
        if !is_date(ctx.since) || !is_date(ctx.until) {
            return Err(malformed("the window is not two dates"));
        }
        let end = next_day(ctx.until).ok_or_else(|| malformed("until is not a date"))?;
        let sql = cost_sql(project, dataset, table, ctx.since, &end);

        let rows = run_query(ctx, &token, project, &sql).await?;
        let mut lines = Vec::new();
        let mut skipped = 0_usize;
        for row in &rows {
            match cost_line(row) {
                Ok(line) => lines.push(line),
                Err(()) => skipped += 1,
            }
        }
        let today = ctx.now.get(0..10).unwrap_or_default();
        let mut notes = Vec::new();
        let mut out = output("gcp", "Google Cloud", &lines, today, &mut notes);
        if skipped > 0 {
            notes.push(format!(
                "{skipped} export row(s) had no readable period or amount and were skipped"
            ));
        }
        notes.push(
            "costs come from the billing export table; the export lags the usage by hours"
                .to_owned(),
        );
        out.notes = notes;
        Ok(out)
    }
}

fn text<'a>(config: &'a Value, key: &str) -> Result<&'a str, String> {
    crate::connector::config_text(config, key)
}

/// A Google Cloud project id: 6 to 30 characters, lower case letters, digits
/// and hyphens, starting with a letter and not ending with a hyphen.
fn valid_project(project: &str) -> bool {
    let bytes = project.as_bytes();
    (6..=30).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// A dataset (letters, digits, underscore) or a table (also hyphen), with no
/// dot or backtick, so it can sit in a backquoted table reference.
fn valid_identifier(name: &str, dataset: bool) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || (!dataset && b == b'-'))
}

/// The PEM body of a PKCS#8 private key, as DER.
///
/// # Errors
/// A sentence when the PEM has no key body or the body is not base64.
pub fn pem_to_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect();
    if body.is_empty() {
        return Err("the private key has no body".to_owned());
    }
    STANDARD
        .decode(body)
        .map_err(|_| "the private key is not PEM".to_owned())
}

/// The RS256 JWT that asks for a `BigQuery` read token, signed with the key.
///
/// # Errors
/// A sentence when the key is not a PKCS#8 RSA key.
pub fn signed_assertion(email: &str, pem: &str, now: i64) -> Result<String, String> {
    let der = pem_to_der(pem)?;
    let key = RsaKeyPair::from_pkcs8(&der)
        .map_err(|_| "the private key is not a PKCS#8 RSA key".to_owned())?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        json!({"iss": email, "scope": SCOPE, "aud": TOKEN_URL, "iat": now, "exp": now + 3600})
            .to_string(),
    );
    let signing_input = format!("{header}.{claims}");
    let mut signature = vec![0_u8; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signing_input.as_bytes(),
        &mut signature,
    )
    .map_err(|_| "the JWT could not be signed".to_owned())?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

async fn access_token(ctx: &SyncContext<'_>, assertion: &str) -> Result<String, VendorError> {
    let body = form_body(&[("grant_type", GRANT), ("assertion", assertion)]);
    let answer = ctx
        .http
        .post_form(TOKEN_HOSTS, TOKEN_URL, &[], &body)
        .await?;
    answer
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| malformed("the token answer has no access_token"))
}

/// The billing export, grouped by month, currency and project. The identifiers
/// were checked by `valid_project` and `valid_identifier`, and the dates by
/// `is_date`, so they are safe to place in the statement.
fn cost_sql(project: &str, dataset: &str, table: &str, since: &str, end_exclusive: &str) -> String {
    format!(
        "SELECT FORMAT_TIMESTAMP('%Y-%m', usage_start_time, 'UTC') AS period, \
         currency, IFNULL(project.id, '') AS project_id, IFNULL(project.name, '') AS project_name, \
         CAST(SUM(CAST(cost AS NUMERIC) + (SELECT IFNULL(SUM(CAST(c.amount AS NUMERIC)), 0) FROM UNNEST(credits) AS c)) AS STRING) AS net_cost \
         FROM `{project}.{dataset}.{table}` \
         WHERE usage_start_time >= TIMESTAMP('{since}') AND usage_start_time < TIMESTAMP('{end_exclusive}') \
         GROUP BY period, currency, project_id, project_name"
    )
}

/// Runs a query job and reads every row, following `getQueryResults` for a job
/// that is still running and for each page.
async fn run_query(
    ctx: &SyncContext<'_>,
    token: &str,
    project: &str,
    sql: &str,
) -> Result<Vec<Value>, VendorError> {
    let headers = vec![("Authorization", format!("Bearer {token}"))];
    let url = format!(
        "https://{BIGQUERY_HOST}/bigquery/v2/projects/{}/queries",
        percent_encode(project)
    );
    let body =
        json!({"query": sql, "useLegacySql": false, "timeoutMs": POLL_MS, "maxResults": PAGE});
    let mut answer = ctx
        .http
        .post_json(BIGQUERY_HOSTS, &url, &headers, &body)
        .await?;
    let job = answer
        .pointer("/jobReference/jobId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| malformed("the query answer has no job id"))?;
    let location = answer
        .pointer("/jobReference/location")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let base = format!(
        "https://{BIGQUERY_HOST}/bigquery/v2/projects/{}/queries/{}",
        percent_encode(project),
        percent_encode(&job)
    );
    let location_query = location
        .map(|loc| format!("&location={}", percent_encode(&loc)))
        .unwrap_or_default();

    let mut polls = 0;
    while answer.get("jobComplete").and_then(Value::as_bool) != Some(true) {
        polls += 1;
        if polls > MAX_POLLS {
            return Err(malformed("the query did not finish in time"));
        }
        let url = format!("{base}?maxResults={PAGE}&timeoutMs={POLL_MS}{location_query}");
        answer = ctx.http.get_json(BIGQUERY_HOSTS, &url, &headers).await?;
    }

    let schema: Vec<String> = answer
        .pointer("/schema/fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| field.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let mut rows = Vec::new();
    let mut pages = 0;
    loop {
        for row in answer
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rows.push(named_row(row, &schema)?);
        }
        pages += 1;
        let Some(next) = answer
            .get("pageToken")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        else {
            break;
        };
        if pages >= MAX_PAGES {
            return Err(malformed(
                "the query has more pages than Ledgers reads at once",
            ));
        }
        let url = format!(
            "{base}?maxResults={PAGE}&pageToken={}{location_query}",
            percent_encode(next)
        );
        answer = ctx.http.get_json(BIGQUERY_HOSTS, &url, &headers).await?;
    }
    Ok(rows)
}

/// One export row as a JSON object keyed by the column names.
fn named_row(row: &Value, schema: &[String]) -> Result<Value, VendorError> {
    let cells = row
        .get("f")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("a row has no cells"))?;
    if cells.len() != schema.len() {
        return Err(malformed("a row does not match the query's columns"));
    }
    let mut object = serde_json::Map::new();
    for (name, cell) in schema.iter().zip(cells) {
        object.insert(name.clone(), cell.get("v").cloned().unwrap_or(Value::Null));
    }
    Ok(Value::Object(object))
}

/// One export row as a cost line. `Err` for a row without a month or amount.
fn cost_line(row: &Value) -> Result<CostLine, ()> {
    let period = row
        .get("period")
        .and_then(Value::as_str)
        .and_then(month_of)
        .ok_or(())?;
    let currency = row
        .get("currency")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_uppercase();
    let micro = micro_of_value(row.get("net_cost").ok_or(())?).map_err(|_| ())?;
    let id = row
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or("unattributed");
    let name = row
        .get("project_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(id);
    Ok(CostLine {
        period,
        currency,
        kind: "project",
        id: id.to_owned(),
        name: name.to_owned(),
        micro,
    })
}

fn malformed(detail: &str) -> VendorError {
    VendorError::Malformed {
        host: BIGQUERY_HOST.to_owned(),
        detail: detail.to_owned(),
    }
}
