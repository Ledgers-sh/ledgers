//! Company-level tax settings: small-business exemption and VAT reporting method.
//!
//! Settings are human-only writes, guarded and audited like period transitions
//! in [`ledgers_ledger::period`]. Reads default when no row exists, mirroring
//! the domain default of `TaxSettings { small_business: false, vat_basis: VatBasis::Accrual }`.

use crate::error::TaxError;
use cratefield_core::{Database, DbError, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements, record};
use ledgers_db::chain;
use ledgers_db::retry::{self, RetryPolicy};
use sea_query::Value as SeaValue;
use std::time::Duration;

/// VAT reporting method for the company.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum VatBasis {
    /// Accrual method (Sollversteuerung): VAT on invoice date.
    Accrual,
    /// Cash method (Istversteuerung): VAT on payment date.
    Cash,
}

impl VatBasis {
    /// Parse from a string code ("accrual" or "cash").
    ///
    /// # Errors
    ///
    /// Returns [`TaxError::Invalid`] if the code is not recognized.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        match code.to_lowercase().as_str() {
            "accrual" => Ok(VatBasis::Accrual),
            "cash" => Ok(VatBasis::Cash),
            _ => Err(TaxError::Invalid {
                reason: format!("unknown vat basis {code:?}, expected accrual|cash"),
            }),
        }
    }

    /// The basis as a string code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            VatBasis::Accrual => "accrual",
            VatBasis::Cash => "cash",
        }
    }
}

impl std::fmt::Display for VatBasis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// Company-level tax settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxSettings {
    /// Small-business exemption (Kleinunternehmer, § 19 `UStG`).
    pub small_business: bool,
    /// VAT reporting method.
    pub vat_basis: VatBasis,
}

impl Default for TaxSettings {
    fn default() -> Self {
        TaxSettings {
            small_business: false,
            vat_basis: VatBasis::Accrual,
        }
    }
}

/// Error type for settings operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// The actor is not a human (agent keys are forbidden).
    HumanOnly,
    /// A database error occurred.
    Database(String),
    /// The settings data is invalid.
    Invalid(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::HumanOnly => f.write_str("settings changes are human-only"),
            SettingsError::Database(e) => write!(f, "database error: {e}"),
            SettingsError::Invalid(e) => write!(f, "invalid settings: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Reads the tax settings for a company, defaulting when no row exists.
///
/// # Errors
///
/// [`SettingsError::Database`] when the query fails or a row cannot be read back.
pub async fn tax_settings(
    db: &dyn Database,
    company_id: &str,
) -> Result<TaxSettings, SettingsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT small_business, vat_basis FROM tax_settings WHERE company_id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .map_err(|e| SettingsError::Database(e.to_string()))?;

    if let Some(row) = rows.first() {
        let small_business = row
            .get::<i64>("small_business")
            .map(|v| v != 0)
            .ok_or_else(|| SettingsError::Invalid("small_business column missing".to_owned()))?;

        let vat_basis_code = row
            .get::<String>("vat_basis")
            .ok_or_else(|| SettingsError::Invalid("vat_basis column missing".to_owned()))?;

        let vat_basis = VatBasis::from_code(&vat_basis_code)
            .map_err(|e| SettingsError::Invalid(e.to_string()))?;

        Ok(TaxSettings {
            small_business,
            vat_basis,
        })
    } else {
        Ok(TaxSettings::default())
    }
}

/// Sets the tax settings for a company: human-only, guarded, audited.
///
/// The write is one [`batch_atomic`](cratefield_core::Database::batch_atomic):
/// - Guard: actor must be a person (checked before touching the DB)
/// - Upsert: the `tax_settings` row
/// - Audit: one `"tax.set_settings"` event with the settings hash, `company_id`,
///   and the audit chain verified
///
/// A denied attempt writes a `Denied` audit event before returning the error.
///
/// # Errors
///
/// [`SettingsError::HumanOnly`] if the actor is an agent (with a denial event written).
/// [`SettingsError::Database`] when the write fails.
pub async fn set_tax_settings(
    db: &dyn Database,
    company_id: &str,
    settings: &TaxSettings,
    at: &str,
    audit: &AuditContext,
    audit_config: &AuditConfig,
) -> Result<(), SettingsError> {
    // Guard 1: actor must be a person.
    if !audit.actor.is_person() {
        // Write a denial event before returning the error.
        let input = serde_json::json!({
            "small_business": settings.small_business,
            "vat_basis": settings.vat_basis.code(),
        });
        record(
            db,
            audit_config,
            company_id,
            audit,
            &AuditRecord::from_input(
                "tax.set_settings",
                &input,
                Outcome::Denied,
                vec![company_id.to_owned()],
                None,
            )
            .with_reason("human-only action"),
            at,
            RetryPolicy::default(),
            no_sleep,
        )
        .await
        .map_err(|e| SettingsError::Database(e.to_string()))?;

        return Err(SettingsError::HumanOnly);
    }

    // Perform the upsert and audit in one batch.
    let input = serde_json::json!({
        "small_business": settings.small_business,
        "vat_basis": settings.vat_basis.code(),
    });
    let audited = AuditRecord::from_input(
        "tax.set_settings",
        &input,
        Outcome::Allowed,
        vec![company_id.to_owned()],
        None,
    );

    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let audit = audit.clone();
            let audited = audited.clone();
            let settings = settings.clone();
            let company_id = company_id.to_owned();
            let at = at.to_owned();

            async move {
                let head = chain::read_head(db, &company_id).await?;
                let appended =
                    append_statements(&company_id, &head, &audit, &audited, &at, audit_config)
                        .map_err(|e| DbError::Query(e.to_string()))?;

                let upsert = Statement::with_values(
                    "INSERT INTO tax_settings (company_id, small_business, vat_basis, updated_at) \
                     VALUES (?, ?, ?, ?) \
                     ON CONFLICT(company_id) DO UPDATE SET \
                       small_business = excluded.small_business, \
                       vat_basis = excluded.vat_basis, \
                       updated_at = excluded.updated_at"
                        .to_owned(),
                    vec![
                        SeaValue::from(company_id.as_str()),
                        SeaValue::BigInt(Some(i64::from(settings.small_business))),
                        SeaValue::from(settings.vat_basis.code()),
                        SeaValue::from(at.as_str()),
                    ],
                );

                let mut statements = vec![upsert];
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        no_sleep,
    )
    .await
    .map_err(|e| SettingsError::Database(e.to_string()))?;

    Ok(())
}

// Dummy sleep function for retry compatibility.
fn no_sleep(_: Duration) {}

/// Stores the company's own tax rates (JSON, under `tax.user_rates`): human-only,
/// guarded and audited like [`set_tax_settings`]. The caller has validated the
/// rates against the pack; this only persists them.
///
/// # Errors
///
/// [`SettingsError::HumanOnly`] for an agent actor, or [`SettingsError::Database`].
pub async fn set_user_rates(
    db: &dyn Database,
    company_id: &str,
    rates_json: &str,
    at: &str,
    audit: &AuditContext,
    audit_config: &AuditConfig,
) -> Result<(), SettingsError> {
    let input = serde_json::json!({ "rates": rates_json });
    if !audit.actor.is_person() {
        record(
            db,
            audit_config,
            company_id,
            audit,
            &AuditRecord::from_input(
                "tax.set_rates",
                &input,
                Outcome::Denied,
                vec![company_id.to_owned()],
                None,
            )
            .with_reason("human-only action"),
            at,
            RetryPolicy::default(),
            no_sleep,
        )
        .await
        .map_err(|e| SettingsError::Database(e.to_string()))?;
        return Err(SettingsError::HumanOnly);
    }
    let audited = AuditRecord::from_input(
        "tax.set_rates",
        &input,
        Outcome::Allowed,
        vec![company_id.to_owned()],
        None,
    );
    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        || {
            let audit = audit.clone();
            let audited = audited.clone();
            let company_id = company_id.to_owned();
            let at = at.to_owned();
            let rates_json = rates_json.to_owned();

            async move {
                let head = chain::read_head(db, &company_id).await?;
                let appended =
                    append_statements(&company_id, &head, &audit, &audited, &at, audit_config)
                        .map_err(|e| DbError::Query(e.to_string()))?;
                let upsert = Statement::with_values(
                    "INSERT INTO company_setting (company_id, key, value, updated_at) \
                     VALUES (?, ?, ?, ?) \
                     ON CONFLICT (company_id, key) DO UPDATE SET \
                       value = excluded.value, updated_at = excluded.updated_at"
                        .to_owned(),
                    vec![
                        SeaValue::from(company_id.as_str()),
                        SeaValue::from("tax.user_rates"),
                        SeaValue::from(rates_json.as_str()),
                        SeaValue::from(at.as_str()),
                    ],
                );
                let mut statements = vec![upsert];
                statements.extend(appended.statements);
                Ok(statements)
            }
        },
        no_sleep,
    )
    .await
    .map_err(|e| SettingsError::Database(e.to_string()))?;
    Ok(())
}
