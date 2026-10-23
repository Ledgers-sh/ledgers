//! The Factory Zero logos (group and venture logos): the marks of the
//! Factory Zero ventures are stored as PNG documents and attached to the
//! operating company's business units, to both companies and to the Factory
//! Zero group. The PNGs are rendered from the Factory Zero SVGs under
//! `fixtures/demo/logos` when the fixtures are vendored: an SVG is never
//! stored or served, because an SVG can carry script.
//!
//! The steps are idempotent: a document registers once per content (a second
//! registration returns the first), and each attachment rewrites the same
//! value. [`seed_step`] runs one bounded step, so the Worker can run the seed
//! one request at a time (`POST /admin/seed-demo?demo=factory-zero-logos`).

use crate::demo::DEMO_OWNER_ID;
use crate::demo_group::{GROUP_NAME, output_as_owner};
use crate::dispatch::settings::{CompanyProfile, PROFILE_KEY};
use crate::dispatch::{self, Deps};
use cratefield_core::Statement;
use ledgers_documents::commands::{DocumentsRegister, RegisterInput};
use ledgers_group::store as gstore;
use ledgers_ledger::units::UNIT_KIND;
use sea_query::Value;

/// The steps of the logo seed: step 0 the operating company's marks, step 1
/// the Factory Zero IP company's mark.
pub const LOGO_STEPS: u32 = 2;
/// The last step's number.
pub const LAST_LOGO_STEP: u32 = LOGO_STEPS - 1;

/// The Factory Zero mark: the holding unit, both companies and the group.
const FZ_MARK: &[u8] = include_bytes!("../../../fixtures/demo/logos/fz-favicon.png");

/// Each business unit's logo, by unit code. The holding unit carries the
/// Factory Zero mark; each venture carries its own mark.
const UNIT_LOGOS: [(&str, &[u8]); 8] = [
    ("FZ", FZ_MARK),
    (
        "LDG",
        include_bytes!("../../../fixtures/demo/logos/ledgers.png"),
    ),
    (
        "SQZ",
        include_bytes!("../../../fixtures/demo/logos/squeezin.png"),
    ),
    (
        "LB",
        include_bytes!("../../../fixtures/demo/logos/living-brain.png"),
    ),
    (
        "CF",
        include_bytes!("../../../fixtures/demo/logos/cratefield.png"),
    ),
    (
        "OWL",
        include_bytes!("../../../fixtures/demo/logos/owlpost.png"),
    ),
    (
        "COL",
        include_bytes!("../../../fixtures/demo/logos/colonizer.png"),
    ),
    (
        "SEAL",
        include_bytes!("../../../fixtures/demo/logos/sealbin.png"),
    ),
];

/// What one step did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogoStepReport {
    /// The step that ran.
    pub step: u32,
    /// Whether this was the last step.
    pub done: bool,
    /// The step to run next, `None` once the seed is complete.
    pub next: Option<u32>,
    /// Logos attached by this step (companies, group and units).
    pub attached: usize,
}

/// Reads the `step` query parameter of the logo seed route: absent means step 0.
///
/// # Errors
/// A message when the step is not a whole number from 0 to [`LAST_LOGO_STEP`].
pub fn parse_step(raw: Option<&str>) -> Result<u32, String> {
    match raw {
        None => Ok(0),
        Some(text) => text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|step| *step < LOGO_STEPS)
            .ok_or_else(|| format!("step must be a whole number from 0 to {LAST_LOGO_STEP}")),
    }
}

/// Runs the whole logo seed in one go: every step in order. The native path and
/// the tests use this; the Worker runs the steps one request at a time.
///
/// # Errors
/// As [`seed_step`].
pub async fn seed(deps: &Deps, ops: &str, ip: &str) -> Result<usize, String> {
    let mut attached = 0;
    for step in 0..LOGO_STEPS {
        attached += seed_step(deps, ops, ip, step).await?.attached;
    }
    Ok(attached)
}

/// Runs one step of the logo seed. The group demo must already be loaded: the
/// group logo is set on the group it created.
///
/// # Errors
/// A message naming the step that failed, or a step past [`LAST_LOGO_STEP`].
pub async fn seed_step(
    deps: &Deps,
    ops: &str,
    ip: &str,
    step: u32,
) -> Result<LogoStepReport, String> {
    if step >= LOGO_STEPS {
        return Err(format!(
            "step {step} is past the last step {LAST_LOGO_STEP}"
        ));
    }
    let attached = match step {
        0 => operating_logos(deps, ops).await?,
        _ => ip_logo(deps, ip).await?,
    };
    let done = step == LAST_LOGO_STEP;
    Ok(LogoStepReport {
        step,
        done,
        next: (!done).then_some(step + 1),
        attached,
    })
}

/// Step 0: registers the marks in the operating company, attaches each unit's
/// logo, the company's logo and the group's logo.
async fn operating_logos(deps: &Deps, ops: &str) -> Result<usize, String> {
    let fz = register_mark(deps, ops, FZ_MARK).await?;
    let mut attached = 0;
    for (code, bytes) in UNIT_LOGOS {
        let doc = register_mark(deps, ops, bytes).await?;
        attach_unit(deps, ops, code, &doc).await?;
        attached += 1;
    }
    set_company_logo(deps, ops, &fz).await?;
    attached += 1;
    let group = group_id(deps, ops).await?;
    set_group_logo(deps, &group, ops, &fz).await?;
    Ok(attached + 1)
}

/// Step 1: registers the Factory Zero mark in the IP company and attaches it.
async fn ip_logo(deps: &Deps, ip: &str) -> Result<usize, String> {
    let fz = register_mark(deps, ip, FZ_MARK).await?;
    set_company_logo(deps, ip, &fz).await?;
    Ok(1)
}

/// Stores a PNG as a document of `company` and returns its id. The register
/// command answers the first copy's id again for identical bytes, so a repeat
/// stores nothing new. The bytes go to the blob store under their content hash.
async fn register_mark(deps: &Deps, company: &str, bytes: &[u8]) -> Result<String, String> {
    let sha256 = sha256_hex(bytes);
    let input = RegisterInput {
        id: deps.id_gen.ulid(),
        sha256: sha256.clone(),
        size_bytes: i64::try_from(bytes.len()).map_err(|err| err.to_string())?,
        mime: "image/png".to_owned(),
        original_name: "factory-zero-logo.png".to_owned(),
        uploaded_by: DEMO_OWNER_ID.to_owned(),
        text_layer: None,
        retention_class: None,
        source: Some("api".to_owned()),
    };
    let registered = output_as_owner(deps, company, &DocumentsRegister, &input).await?;
    let blob = deps
        .blob
        .as_ref()
        .ok_or_else(|| "the blob store is not bound".to_owned())?;
    let key = crate::documents::blob_key(&sha256);
    let stored = blob
        .get(&key)
        .await
        .map_err(|err| format!("read the logo blob: {err:?}"))?;
    if stored.is_none() {
        blob.put(&key, bytes, "image/png")
            .await
            .map_err(|err| format!("write the logo blob: {err:?}"))?;
    }
    Ok(registered.document.id)
}

/// The SHA-256 of `bytes`, lowercase hex: the document's content key.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Points a business unit's logo at a document of the same company.
async fn attach_unit(deps: &Deps, company: &str, code: &str, doc: &str) -> Result<(), String> {
    deps.db
        .execute(&Statement::with_values(
            "UPDATE dimension_value SET logo_document_id = ? WHERE company_id = ? AND kind = ? AND code = ?"
                .to_owned(),
            vec![
                Value::from(doc),
                Value::from(company),
                Value::from(UNIT_KIND),
                Value::from(code),
            ],
        ))
        .await
        .map(|_| ())
        .map_err(|err| format!("attach the {code} logo: {err}"))
}

/// Sets the company profile's logo, keeping the other profile fields.
async fn set_company_logo(deps: &Deps, company: &str, doc: &str) -> Result<(), String> {
    let db = deps.db.as_ref();
    let rows = db
        .query(&Statement::with_values(
            "SELECT value FROM company_setting WHERE company_id = ? AND key = ?".to_owned(),
            vec![Value::from(company), Value::from(PROFILE_KEY)],
        ))
        .await
        .map_err(|err| format!("read the company profile: {err}"))?;
    let mut profile: CompanyProfile =
        match rows.rows.first().and_then(|row| row.get::<String>("value")) {
            Some(text) => serde_json::from_str(&text)
                .map_err(|err| format!("the company profile is not valid JSON: {err}"))?,
            None => CompanyProfile::default(),
        };
    profile.logo_document_id = doc.to_owned();
    let stored = serde_json::to_string(&profile).map_err(|err| err.to_string())?;
    let now = dispatch::rfc3339_utc(deps.clock.now());
    db.execute(&Statement::with_values(
        "DELETE FROM company_setting WHERE company_id = ? AND key = ?".to_owned(),
        vec![Value::from(company), Value::from(PROFILE_KEY)],
    ))
    .await
    .map_err(|err| format!("replace the company profile: {err}"))?;
    db.execute(&Statement::with_values(
        "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, ?, ?, ?)"
            .to_owned(),
        vec![
            Value::from(company),
            Value::from(PROFILE_KEY),
            Value::from(stored),
            Value::from(now),
        ],
    ))
    .await
    .map(|_| ())
    .map_err(|err| format!("write the company profile: {err}"))
}

/// The id of the Factory Zero group, read from the operating company's groups.
async fn group_id(deps: &Deps, ops: &str) -> Result<String, String> {
    gstore::groups_of_company(deps.db.as_ref(), ops)
        .await
        .map_err(|err| format!("read the groups: {err}"))?
        .into_iter()
        .find(|group| group.name == GROUP_NAME)
        .map(|group| group.id)
        .ok_or_else(|| "the group demo has not run; run demo=factory-zero-group first".to_owned())
}

/// Sets the group's logo to a document of `company`.
async fn set_group_logo(deps: &Deps, group: &str, company: &str, doc: &str) -> Result<(), String> {
    deps.db
        .execute(&Statement::with_values(
            "UPDATE company_group SET logo_document_id = ?, logo_company_id = ? WHERE id = ?"
                .to_owned(),
            vec![Value::from(doc), Value::from(company), Value::from(group)],
        ))
        .await
        .map(|_| ())
        .map_err(|err| format!("attach the group logo: {err}"))
}
