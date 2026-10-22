//! Parties master data: customers and vendors (issue #44).
//!
//! Commands for managing parties (vendor table), including:
//! - parties.list: read parties with latest VIES results
//! - parties.create: create a party (master data only, agent allowed)
//! - parties.update: update a party (master data only, agent allowed)
//! - `parties.set_bank`: change IBAN/BIC with history tracking (human only)
//! - `parties.check_vat`: validate VAT number against VIES (agent allowed)
//! - `parties.merge`: merge two parties (human only)

use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard;
use ledgers_extract::llm::Transport;
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

// ===================================================================== list

/// The input of `parties.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Maximum rows to return; defaults to 50.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Number of rows to skip; defaults to 0.
    #[serde(default)]
    pub offset: Option<u32>,
}

/// A party with latest VIES result.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PartyView {
    /// The party id.
    pub id: String,
    /// The legal name.
    pub name: String,
    /// The VAT id (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vat_id: Option<String>,
    /// The IBAN (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iban: Option<String>,
    /// The BIC (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bic: Option<String>,
    /// Roles: 'customer', 'vendor', or 'both'.
    pub roles: String,
    /// Whether merged away (null if not merged).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    /// Latest VIES check result (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_vies: Option<ViesResult>,
    /// Whether this party's name/address/contacts is personal data.
    pub personal_data: bool,
    /// IDs of other unmerged parties with possible duplicates (same VAT id, IBAN, or name).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub possible_duplicates: Vec<String>,
}

/// The latest VIES check result for a party.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ViesResult {
    /// Whether the VAT number is valid.
    pub valid: bool,
    /// The VIES request identifier (consultation number).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consultation_number: Option<String>,
    /// When we checked it.
    pub checked_at: String,
}

/// The output of `parties.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListOutput {
    /// The parties.
    pub parties: Vec<PartyView>,
    /// Total number of parties (for pagination).
    pub total: u32,
}

/// `parties.list`: read parties with latest VIES results.
pub struct PartiesList;

#[async_trait::async_trait]
impl Command for PartiesList {
    const NAME: &'static str = "parties.list";
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
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let limit = input.limit.unwrap_or(50).min(200) as usize;
        let offset = input.offset.unwrap_or(0) as usize;

        // Query all parties for the company (unmerged).
        let rows = db
            .query(&Statement::with_values(
                format!("SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND merged_into IS NULL ORDER BY created_at DESC")
            , vec![Value::from(ctx.company_id.as_str())]))
            .await
            .map_err(|_| CommandError::InvalidInput("database error".to_string()))?;

        #[allow(clippy::cast_possible_truncation)]
        let total = rows.rows.len() as u32;

        // First, read all unmerged parties to detect duplicates.
        let all_parties_data: Vec<_> = rows
            .rows
            .iter()
            .map(|row| {
                let id = row.get::<String>("id").unwrap_or_default();
                let name = row.get::<String>("name").unwrap_or_default();
                let vat_id = row.get::<Option<String>>("vat_id").flatten();
                let iban = row.get::<Option<String>>("iban").flatten();
                (id, name, vat_id, iban)
            })
            .collect();

        let mut parties = Vec::new();

        for row in rows.rows.iter().skip(offset).take(limit) {
            let id = row
                .get::<String>("id")
                .ok_or_else(|| CommandError::InvalidInput("missing id".to_string()))?;
            let name = row
                .get::<String>("name")
                .ok_or_else(|| CommandError::InvalidInput("missing name".to_string()))?;
            let vat_id = row.get::<Option<String>>("vat_id").flatten();
            let iban = row.get::<Option<String>>("iban").flatten();
            let bic = row.get::<Option<String>>("bic").flatten();
            let roles = row
                .get::<String>("roles")
                .ok_or_else(|| CommandError::InvalidInput("missing roles".to_string()))?;
            let merged_into = row.get::<Option<String>>("merged_into").flatten();
            let personal_data = row.get::<i64>("personal_data").unwrap_or(0) != 0;

            // Get latest VIES result for this party.
            let latest_vies = db
                .query(&Statement::with_values(
                    "SELECT valid, consultation_number, checked_at FROM vies_check WHERE company_id = ? AND vendor_id = ? ORDER BY checked_at DESC LIMIT 1".to_string()
                , vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(id.as_str()),
                ]))
                .await
                .ok()
                .and_then(|vies_rows| {
                    vies_rows.rows.first().and_then(|vrow| {
                        let valid = vrow.get::<i64>("valid").unwrap_or(0) != 0;
                        let consultation_number = vrow.get::<Option<String>>("consultation_number").flatten();
                        let checked_at = vrow.get::<String>("checked_at")?;
                        Some(ViesResult {
                            valid,
                            consultation_number,
                            checked_at,
                        })
                    })
                });

            // Detect possible duplicates.
            let possible_duplicates = all_parties_data
                .iter()
                .filter_map(|(other_id, other_name, other_vat, other_iban)| {
                    if other_id == &id {
                        return None; // Skip self
                    }

                    // Check if VAT ids match (case/space-insensitive).
                    if let (Some(v), Some(ov)) = (&vat_id, other_vat)
                        && normalise_vat(v) == normalise_vat(ov)
                    {
                        return Some(other_id.clone());
                    }

                    // Check if IBANs match (whitespace-insensitive, uppercase).
                    if let (Some(i), Some(oi)) = (&iban, other_iban)
                        && normalise_iban(i) == normalise_iban(oi)
                    {
                        return Some(other_id.clone());
                    }

                    // Check if names match (normalized).
                    let normalized_name = normalise_name(&name);
                    if !normalized_name.is_empty() && normalized_name == normalise_name(other_name)
                    {
                        return Some(other_id.clone());
                    }

                    None
                })
                .collect();

            parties.push(PartyView {
                id,
                name,
                vat_id,
                iban,
                bic,
                roles,
                merged_into,
                latest_vies,
                personal_data,
                possible_duplicates,
            });
        }

        Ok(Plan::with_output(ListOutput { parties, total }))
    }
}

// ===================================================================== create

/// The input of `parties.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The party id (generated by server if empty).
    #[serde(default)]
    pub id: Option<String>,
    /// The legal name.
    pub name: String,
    /// The VAT id (optional).
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Roles: 'customer', 'vendor', or 'both'; defaults to 'vendor'.
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// Language code (optional).
    #[serde(default)]
    pub language: Option<String>,
    /// Currency code (optional).
    #[serde(default)]
    pub currency: Option<String>,
    /// Whether this party's name/addresses/contacts is personal data (default false).
    #[serde(default)]
    pub personal_data: Option<bool>,
    /// Payment terms in days (optional, >= 0).
    #[serde(default)]
    pub payment_terms_days: Option<i32>,
    /// Default account number (optional).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Default tax code (optional).
    #[serde(default)]
    pub default_tax_code: Option<String>,
}

/// The output of `parties.create`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateOutput {
    /// The created party.
    pub party: PartyView,
}

/// `parties.create`: create a party (master data only, agent allowed).
pub struct PartiesCreate;

#[async_trait::async_trait]
impl Command for PartiesCreate {
    const NAME: &'static str = "parties.create";
    const TIER: Tier = Tier::Draft;

    type Input = CreateInput;
    type Output = CreateOutput;

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
        _db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let id = input.id.clone().ok_or_else(|| {
            CommandError::InvalidInput("id must be provided or filled by server".to_string())
        })?;
        let roles = input.roles.clone().unwrap_or_else(|| "vendor".to_string());
        let personal_data = input.personal_data.unwrap_or(false);

        // Validate roles.
        if !["customer", "vendor", "both"].contains(&roles.as_str()) {
            return Err(CommandError::InvalidInput(
                "roles must be 'customer', 'vendor', or 'both'".to_string(),
            ));
        }

        // Validate payment_terms_days.
        if let Some(days) = input.payment_terms_days
            && days < 0
        {
            return Err(CommandError::InvalidInput(
                "payment_terms_days must be >= 0".to_string(),
            ));
        }

        let values = vec![
            Value::from(id.as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(input.name.as_str()),
            optional_string_value(input.vat_id.as_ref()),
            Value::from(roles.as_str()),
            optional_string_value(input.tax_number.as_ref()),
            optional_string_value(input.language.as_ref()),
            optional_string_value(input.currency.as_ref()),
            Value::from(i32::from(personal_data)),
            input
                .payment_terms_days
                .map_or(Value::String(None), Value::from),
            optional_string_value(input.default_account.as_ref()),
            optional_string_value(input.default_tax_code.as_ref()),
            Value::from(ctx.now.as_str()),
            Value::from(ctx.now.as_str()),
        ];

        let statement = Statement::with_values(
            "INSERT INTO vendor (id, company_id, name, vat_id, roles, tax_number, language, currency, personal_data, payment_terms_days, default_account, default_tax_code, source, needs_review, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'manual', 0, ?, ?)".to_string(),
            values
        );

        let party = PartyView {
            id: id.clone(),
            name: input.name.clone(),
            vat_id: input.vat_id.clone(),
            iban: None,
            bic: None,
            roles,
            merged_into: None,
            latest_vies: None,
            personal_data,
            possible_duplicates: Vec::new(),
        };

        let mut plan = Plan::with_output(CreateOutput { party });
        plan.statements.push(statement);
        plan.objects.push(format!("vendor:{id}"));
        Ok(plan)
    }
}

// ===================================================================== update

/// The input of `parties.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The party id (from the path).
    pub id: String,
    /// The legal name (optional).
    #[serde(default)]
    pub name: Option<String>,
    /// The VAT id (optional).
    #[serde(default)]
    pub vat_id: Option<String>,
    /// Roles (optional).
    #[serde(default)]
    pub roles: Option<String>,
    /// Tax number (optional).
    #[serde(default)]
    pub tax_number: Option<String>,
    /// Language code (optional).
    #[serde(default)]
    pub language: Option<String>,
    /// Currency code (optional).
    #[serde(default)]
    pub currency: Option<String>,
    /// Whether personal data (optional).
    #[serde(default)]
    pub personal_data: Option<bool>,
    /// Payment terms in days (optional).
    #[serde(default)]
    pub payment_terms_days: Option<i32>,
    /// Default account number (optional).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Default tax code (optional).
    #[serde(default)]
    pub default_tax_code: Option<String>,
}

/// The output of `parties.update`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateOutput {
    /// The updated party.
    pub party: PartyView,
}

/// `parties.update`: update a party (master data only, agent allowed).
pub struct PartiesUpdate;

#[async_trait::async_trait]
#[allow(clippy::too_many_lines)]
impl Command for PartiesUpdate {
    const NAME: &'static str = "parties.update";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");

    type Input = UpdateInput;
    type Output = UpdateOutput;

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
        // Guard: party exists and is not merged.
        let existing = db
            .query(&Statement::with_values(
                format!("SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND id = ?"),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ))
            .await
            .map_err(|_| CommandError::InvalidInput("database error".to_string()))?;

        let row = existing
            .rows
            .first()
            .ok_or_else(|| CommandError::InvalidInput("party not found".to_string()))?;

        let merged_into = row.get::<Option<String>>("merged_into").flatten();
        if merged_into.is_some() {
            return Err(CommandError::InvalidInput(
                "party is merged and cannot be updated".to_string(),
            ));
        }

        // Validate payment_terms_days.
        if let Some(days) = input.payment_terms_days
            && days < 0
        {
            return Err(CommandError::InvalidInput(
                "payment_terms_days must be >= 0".to_string(),
            ));
        }

        // Build UPDATE statement only with provided fields.
        let mut updates = vec!["updated_at = ?"];
        let mut values = vec![Value::from(ctx.now.as_str())];

        if let Some(n) = &input.name {
            updates.push("name = ?");
            values.push(Value::from(n.as_str()));
        }
        if let Some(v) = &input.vat_id {
            updates.push("vat_id = ?");
            values.push(Value::from(v.as_str()));
        }
        if let Some(r) = &input.roles {
            updates.push("roles = ?");
            values.push(Value::from(r.as_str()));
        }
        if let Some(t) = &input.tax_number {
            updates.push("tax_number = ?");
            values.push(Value::from(t.as_str()));
        }
        if let Some(l) = &input.language {
            updates.push("language = ?");
            values.push(Value::from(l.as_str()));
        }
        if let Some(c) = &input.currency {
            updates.push("currency = ?");
            values.push(Value::from(c.as_str()));
        }
        if input.personal_data.is_some() {
            updates.push("personal_data = ?");
            values.push(Value::from(i32::from(input.personal_data.unwrap_or(false))));
        }
        if let Some(d) = input.payment_terms_days {
            updates.push("payment_terms_days = ?");
            values.push(Value::from(d));
        }
        if let Some(a) = &input.default_account {
            updates.push("default_account = ?");
            values.push(Value::from(a.as_str()));
        }
        if let Some(t) = &input.default_tax_code {
            updates.push("default_tax_code = ?");
            values.push(Value::from(t.as_str()));
        }

        values.push(Value::from(ctx.company_id.as_str()));
        values.push(Value::from(input.id.as_str()));

        // Guard: party exists, in company, not merged.
        let guard_key = guard::guard_key(&["party_not_merged", &input.id]);
        let mut statements = vec![guard::assert_statement(
            &guard_key,
            "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        )];

        let statement = Statement::with_values(
            format!(
                "UPDATE vendor SET {} WHERE company_id = ? AND id = ?",
                updates.join(", ")
            ),
            values,
        );
        statements.push(statement);
        statements.push(guard::release_statement(&guard_key));

        let party = PartyView {
            id: input.id.clone(),
            name: input
                .name
                .clone()
                .unwrap_or_else(|| row.get::<String>("name").unwrap_or_default()),
            vat_id: input
                .vat_id
                .clone()
                .or_else(|| row.get::<Option<String>>("vat_id").flatten()),
            iban: row.get::<Option<String>>("iban").flatten(),
            bic: row.get::<Option<String>>("bic").flatten(),
            roles: input
                .roles
                .clone()
                .unwrap_or_else(|| row.get::<String>("roles").unwrap_or_default()),
            merged_into: None,
            latest_vies: None,
            personal_data: input
                .personal_data
                .unwrap_or(row.get::<i64>("personal_data").unwrap_or(0) != 0),
            possible_duplicates: Vec::new(),
        };

        let mut plan = Plan::with_output(UpdateOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        Ok(plan)
    }
}

// ===================================================================== set_bank

/// The input of `parties.set_bank`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetBankInput {
    /// The party id (from the path).
    pub id: String,
    /// The new IBAN.
    pub iban: String,
    /// The new BIC (optional).
    #[serde(default)]
    pub bic: Option<String>,
    /// Reason for the change.
    pub reason: String,
}

/// The output of `parties.set_bank`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SetBankOutput {
    /// The updated party.
    pub party: PartyView,
}

/// `parties.set_bank`: change IBAN/BIC with history tracking (human only).
pub struct PartiesSetBank {
    /// The person's id (who made the change), set by dispatch before execution.
    pub person_id: Option<String>,
}

impl PartiesSetBank {
    /// Create a new `SetBank` command with optional `person_id`.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesSetBank {
    const NAME: &'static str = "parties.set_bank";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = SetBankInput;
    type Output = SetBankOutput;

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
        _db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Require person_id: a bank change must never be written without the acting person.
        let changed_by = self
            .person_id
            .clone()
            .ok_or(CommandError::Unauthenticated)?;

        // Validate IBAN (basic mod-97 check).
        if !validate_iban(&input.iban) {
            return Err(CommandError::InvalidInput(
                "invalid IBAN checksum".to_string(),
            ));
        }

        let change_id = format!("change_{}", ctx.event_id);
        let mut statements = Vec::new();

        // Guard: party exists, in company, not merged.
        statements.push(Statement::with_values(
            "INSERT INTO ledger_guard (guard_key, ok) SELECT ?, CASE WHEN (SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1 THEN 1 ELSE NULL END".to_string()
        , vec![
            Value::from(format!("party_not_merged:{}", input.id).as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(input.id.as_str()),
        ]));

        // Insert bank change record (captures before values atomically).
        statements.push(Statement::with_values(
            "INSERT INTO vendor_bank_change (id, company_id, vendor_id, before_iban, before_bic, after_iban, after_bic, changed_by, changed_at, reason, created_at) SELECT ?, company_id, id, iban, bic, ?, ?, ?, ?, ?, ? FROM vendor WHERE company_id = ? AND id = ?".to_string()
        , vec![
            Value::from(change_id.as_str()),
            Value::from(input.iban.as_str()),
            input.bic.as_ref().map_or(Value::String(None), |b| Value::from(b.as_str())),
            Value::from(changed_by.as_str()),
            Value::from(ctx.now.as_str()),
            Value::from(input.reason.as_str()),
            Value::from(ctx.now.as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(input.id.as_str()),
        ]));

        // Update vendor with new IBAN/BIC.
        statements.push(Statement::with_values(
            "UPDATE vendor SET iban = ?, bic = ?, updated_at = ? WHERE company_id = ? AND id = ?"
                .to_string(),
            vec![
                Value::from(input.iban.as_str()),
                input
                    .bic
                    .as_ref()
                    .map_or(Value::String(None), |b| Value::from(b.as_str())),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));

        // Flag open bills with iban_changed=1.
        statements.push(Statement::with_values(
            "UPDATE bill SET iban_changed = 1, updated_at = ? WHERE company_id = ? AND vendor_id = ? AND posted_entry_id IS NULL AND discarded_at IS NULL".to_string()
        , vec![
            Value::from(ctx.now.as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(input.id.as_str()),
        ]));

        let party = PartyView {
            id: input.id.clone(),
            name: "Party".to_string(),
            vat_id: None,
            iban: Some(input.iban.clone()),
            bic: input.bic.clone(),
            roles: "vendor".to_string(),
            merged_into: None,
            latest_vies: None,
            personal_data: false,
            possible_duplicates: Vec::new(),
        };

        let mut plan = Plan::with_output(SetBankOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        plan.objects.push(format!("vendor_bank_change:{change_id}"));
        Ok(plan)
    }
}

// ===================================================================== check_vat

/// The input of `parties.check_vat`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckVatInput {
    /// The party id (from the path).
    pub id: String,
    /// The country code (e.g., 'DE', 'FR'); must be EU.
    pub country_code: String,
    /// The VAT number (without country prefix).
    pub vat_number: String,
}

/// The output of `parties.check_vat`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CheckVatOutput {
    /// The VIES check result.
    pub vies_result: ViesCheckResult,
}

/// The VIES check result returned to the caller.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ViesCheckResult {
    /// Whether the VAT number is valid.
    pub valid: bool,
    /// The returned business name (if valid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The returned address (if valid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The VIES request identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consultation_number: Option<String>,
    /// The VIES request date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_date: Option<String>,
}

/// `parties.check_vat`: validate VAT number against VIES (agent allowed).
pub struct PartiesCheckVat {
    pub transport: Option<Arc<dyn Transport>>,
}

impl PartiesCheckVat {
    /// Create a new `CheckVat` command with an optional transport (for testing).
    #[must_use]
    pub fn new(transport: Option<Arc<dyn Transport>>) -> Self {
        Self { transport }
    }
}

#[async_trait::async_trait]
impl Command for PartiesCheckVat {
    const NAME: &'static str = "parties.check_vat";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");

    type Input = CheckVatInput;
    type Output = CheckVatOutput;

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
        _db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Validate country code is EU.
        let eu_codes = [
            "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "EL", "ES", "FI", "FR", "GR", "HR",
            "HU", "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
            "XI",
        ]; // XI = Northern Ireland
        if !eu_codes.contains(&input.country_code.as_str()) {
            return Err(CommandError::InvalidInput(
                "VAT checking is only available for EU countries".to_string(),
            ));
        }

        // Get transport from context or use provided one.
        let transport = self
            .transport
            .clone()
            .ok_or_else(|| CommandError::InvalidInput("transport not configured".to_string()))?;

        // Call VIES.
        let body = json!({
            "countryCode": input.country_code,
            "vatNumber": input.vat_number,
        });

        let (status, response) = transport
            .post_json(
                "https://ec.europa.eu/taxation_customs/vies/rest-api/check-vat-number",
                &[],
                &body,
            )
            .await
            .map_err(|e| CommandError::InvalidInput(format!("VIES error: {e}")))?;

        if status != 200 {
            return Err(CommandError::InvalidInput(
                "VIES returned an error".to_string(),
            ));
        }

        let valid = response
            .get("valid")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let name = response
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let address = response
            .get("address")
            .and_then(|v| v.as_str())
            .map(String::from);
        let consultation_number = response
            .get("requestIdentifier")
            .and_then(|v| v.as_str())
            .map(String::from);
        let request_date = response
            .get("requestDate")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Insert vies_check record.
        let check_id = format!("vies_{}", ctx.event_id);
        let statement = Statement::with_values(
            "INSERT INTO vies_check (id, company_id, vendor_id, country_code, vat_number, valid, name, address, consultation_number, request_date, checked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".to_string()
        , vec![
            Value::from(check_id.as_str()),
            Value::from(ctx.company_id.as_str()),
            Value::from(input.id.as_str()),
            Value::from(input.country_code.as_str()),
            Value::from(input.vat_number.as_str()),
            Value::from(i32::from(valid)),
            name.as_ref().map_or(Value::String(None), |n| Value::from(n.as_str())),
            address.as_ref().map_or(Value::String(None), |a| Value::from(a.as_str())),
            consultation_number.as_ref().map_or(Value::String(None), |c| Value::from(c.as_str())),
            request_date.as_ref().map_or(Value::String(None), |r| Value::from(r.as_str())),
            Value::from(ctx.now.as_str()),
        ]);

        let result = ViesCheckResult {
            valid,
            name,
            address,
            consultation_number,
            request_date,
        };

        let mut plan = Plan::with_output(CheckVatOutput {
            vies_result: result,
        });
        plan.statements.push(statement);
        plan.objects.push(format!("vies_check:{check_id}"));
        Ok(plan)
    }
}

// ===================================================================== merge

/// The input of `parties.merge`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeInput {
    /// The surviving party id (from the path).
    pub id: String,
    /// The party to merge away.
    pub from: String,
}

/// The output of `parties.merge`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MergeOutput {
    /// The surviving party.
    pub party: PartyView,
}

/// `parties.merge`: merge two parties (human only).
pub struct PartiesMerge {
    /// The person's id (who performed the merge), set by dispatch before execution.
    pub person_id: Option<String>,
}

impl PartiesMerge {
    /// Create a new Merge command with optional `person_id`.
    #[must_use]
    pub fn new(person_id: Option<String>) -> Self {
        Self { person_id }
    }
}

#[async_trait::async_trait]
impl Command for PartiesMerge {
    const NAME: &'static str = "parties.merge";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = true;
    const TARGET: Option<&'static str> = Some("id");

    type Input = MergeInput;
    type Output = MergeOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
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
        if input.id == input.from {
            return Err(CommandError::InvalidInput(
                "cannot merge a party into itself".to_string(),
            ));
        }

        // Guard: both parties exist, not merged, distinct.
        let survivor = db
            .query(&Statement::with_values(
                format!("SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND id = ?"),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ))
            .await
            .map_err(|_| CommandError::InvalidInput("database error".to_string()))?;

        let from_party = db
            .query(&Statement::with_values(
                format!("SELECT {VENDOR_COLUMNS} FROM vendor WHERE company_id = ? AND id = ?"),
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.from.as_str()),
                ],
            ))
            .await
            .map_err(|_| CommandError::InvalidInput("database error".to_string()))?;

        let survivor_row = survivor
            .rows
            .first()
            .ok_or_else(|| CommandError::InvalidInput("survivor party not found".to_string()))?;
        let from_row = from_party
            .rows
            .first()
            .ok_or_else(|| CommandError::InvalidInput("source party not found".to_string()))?;

        if survivor_row
            .get::<Option<String>>("merged_into")
            .flatten()
            .is_some()
        {
            return Err(CommandError::InvalidInput(
                "survivor party is already merged".to_string(),
            ));
        }
        if from_row
            .get::<Option<String>>("merged_into")
            .flatten()
            .is_some()
        {
            return Err(CommandError::InvalidInput(
                "source party is already merged".to_string(),
            ));
        }

        // Guard: survivor exists, in company, not merged.
        let survivor_guard_key = guard::guard_key(&["merge_survivor", &input.id]);
        // Guard: source exists, in company, not merged.
        let source_guard_key = guard::guard_key(&["merge_source", &input.from]);

        let mut statements = vec![
            guard::assert_statement(
                &survivor_guard_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.id.as_str()),
                ],
            ),
            guard::assert_statement(
                &source_guard_key,
                "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(input.from.as_str()),
                ],
            ),
        ];

        // Repoint bills from source to survivor.
        statements.push(Statement::with_values(
            "UPDATE bill SET vendor_id = ?, updated_at = ? WHERE company_id = ? AND vendor_id = ?"
                .to_string(),
            vec![
                Value::from(input.id.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.from.as_str()),
            ],
        ));

        // Mark source party as merged, ensuring it's not already merged (defence in depth).
        statements.push(Statement::with_values(
            "UPDATE vendor SET merged_into = ?, updated_at = ? WHERE company_id = ? AND id = ? AND merged_into IS NULL"
                .to_string(),
            vec![
                Value::from(input.id.as_str()),
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.from.as_str()),
            ],
        ));

        // Release guards
        statements.push(guard::release_statement(&survivor_guard_key));
        statements.push(guard::release_statement(&source_guard_key));

        let party = PartyView {
            id: input.id.clone(),
            name: survivor_row.get::<String>("name").unwrap_or_default(),
            vat_id: survivor_row.get::<Option<String>>("vat_id").flatten(),
            iban: survivor_row.get::<Option<String>>("iban").flatten(),
            bic: survivor_row.get::<Option<String>>("bic").flatten(),
            roles: survivor_row.get::<String>("roles").unwrap_or_default(),
            merged_into: None,
            latest_vies: None,
            personal_data: survivor_row.get::<i64>("personal_data").unwrap_or(0) != 0,
            possible_duplicates: Vec::new(),
        };

        let mut plan = Plan::with_output(MergeOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        plan.objects.push(format!("vendor:{}", input.from));
        Ok(plan)
    }
}

// ===================================================================== zero-rating gate

/// Guard statement for zero-rating gate: checks that a party has a valid VIES check.
/// Used to enforce that intra-EU sales to a party can only be zero-rated if their VAT
/// id has been validated through VIES.
///
/// Returns a Statement that fails (returns NULL in the ok column) if no valid VIES
/// check exists for the party's current VAT id.
#[must_use]
pub fn vies_valid_guard(company_id: &str, party_id: &str) -> Statement {
    Statement::with_values(
        "INSERT INTO ledger_guard (guard_key, ok) SELECT ?, CASE WHEN EXISTS(\
            SELECT 1 FROM vies_check v \
            JOIN vendor p ON v.vendor_id = p.id \
            WHERE v.company_id = ? AND v.vendor_id = ? AND v.valid = 1 \
            AND v.vat_number = p.vat_id AND v.checked_at = (\
                SELECT MAX(checked_at) FROM vies_check \
                WHERE vendor_id = ? AND valid = 1\
            )\
        ) THEN 1 ELSE NULL END"
            .to_string(),
        vec![
            Value::from(format!("vies_valid:{party_id}").as_str()),
            Value::from(company_id),
            Value::from(party_id),
            Value::from(party_id),
        ],
    )
}

// ===================================================================== helpers

/// Standard columns for vendor SELECT queries.
const VENDOR_COLUMNS: &str = "id, name, vat_id, iban, bic, roles, merged_into, personal_data";

/// Convert optional string to Value (None becomes SQL NULL).
fn optional_string_value(s: Option<&String>) -> Value {
    s.map_or(Value::String(None), |v| Value::from(v.as_str()))
}

/// Normalizes a name for duplicate detection (removes non-alphanumeric, lowercase).
fn normalise_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Normalizes a VAT id for duplicate detection (removes spaces, lowercase).
fn normalise_vat(vat: &str) -> String {
    vat.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Normalizes an IBAN for duplicate detection (removes spaces, uppercase).
fn normalise_iban(iban: &str) -> String {
    iban.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
}

/// Validates IBAN using mod-97 checksum.
fn validate_iban(iban: &str) -> bool {
    let iban = iban.replace(' ', "").to_uppercase();

    // Basic format check
    if iban.len() < 15 || iban.len() > 34 {
        return false;
    }

    // Move first 4 chars to end
    let rearranged = format!("{}{}", &iban[4..], &iban[..4]);

    // Replace letters with numbers (A=10, B=11, ..., Z=35)
    let mut numeric = String::new();
    for c in rearranged.chars() {
        if c.is_ascii_digit() {
            numeric.push(c);
        } else if c.is_ascii_uppercase() {
            numeric.push_str(&(c as u32 - 'A' as u32 + 10).to_string());
        } else {
            return false;
        }
    }

    // Check mod 97 == 1
    let remainder: u32 = numeric
        .chars()
        .fold(0u32, |acc, c| (acc * 10 + c.to_digit(10).unwrap_or(0)) % 97);

    remainder == 1
}
