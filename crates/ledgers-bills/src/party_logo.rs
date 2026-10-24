//! `parties.set_logo`: a party's logo, a stored PNG or JPEG of at most 2 MiB.
//!
//! The logo is an ordinary document of the company (see
//! `ledgers_documents::logo`), named by its id. The command checks the
//! document's type and size, and a guard statement proves in the batch that
//! the document is this company's. Blank or absent clears the logo.

use crate::parties::PartyView;
use crate::party_fields::{self as pf, MERGED, NOT_FOUND, logo_refusal, refuse};
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard;
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// The input of `parties.set_logo`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLogoInput {
    /// The party id (from the path).
    pub id: String,
    /// The stored logo document (a PNG or JPEG of at most 2 MiB). Blank or absent clears it.
    #[serde(default)]
    pub logo_document_id: Option<String>,
}

/// The output of `parties.set_logo`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SetLogoOutput {
    /// The party with its logo.
    pub party: PartyView,
}

/// `parties.set_logo`: sets or clears a party's logo (agent allowed).
pub struct PartiesSetLogo;

#[async_trait::async_trait]
impl Command for PartiesSetLogo {
    const NAME: &'static str = "parties.set_logo";
    const TIER: Tier = Tier::Draft;
    const TARGET: Option<&'static str> = Some("id");

    type Input = SetLogoInput;
    type Output = SetLogoOutput;

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
        let row = crate::parties::read_party_row(db, &ctx.company_id, &input.id)
            .await?
            .ok_or_else(|| refuse(&NOT_FOUND, "id", "party not found"))?;
        let mut party = crate::parties::party_from_row(&row)?;
        if party.merged_into.is_some() {
            return Err(refuse(
                &MERGED,
                "id",
                "party is merged and cannot be changed",
            ));
        }
        let doc = input
            .logo_document_id
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string);
        let guard_key = guard::guard_key(&["party_not_merged", &input.id]);
        let mut statements = vec![guard::assert_statement(
            &guard_key,
            "(SELECT COUNT(*) FROM vendor WHERE company_id = ? AND id = ? AND merged_into IS NULL) = 1",
            vec![
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        )];
        if let Some(doc) = &doc {
            let stored = ledgers_documents::store::get(db, &ctx.company_id, doc)
                .await?
                .ok_or_else(|| {
                    refuse(
                        &pf::LOGO_NOT_STORED,
                        "logo_document_id",
                        "the logo must be an image this company has uploaded",
                    )
                })?;
            ledgers_documents::logo::check_logo(&stored.mime, stored.size_bytes)
                .map_err(|refusal| logo_refusal(&refusal))?;
            let doc_key = guard::guard_key(&["party-logo", doc]);
            statements.push(guard::assert_statement(
                &doc_key,
                "(SELECT COUNT(*) FROM document WHERE company_id = ? AND id = ?) = 1",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(doc.as_str()),
                ],
            ));
            statements.push(guard::release_statement(&doc_key));
        }
        statements.push(Statement::with_values(
            "UPDATE vendor SET logo_document_id = ?, updated_at = ? WHERE company_id = ? AND id = ?"
                .to_string(),
            vec![
                match &doc {
                    Some(d) => Value::from(d.as_str()),
                    None => Value::String(None),
                },
                Value::from(ctx.now.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.id.as_str()),
            ],
        ));
        statements.push(guard::release_statement(&guard_key));
        party.logo_document_id = doc;
        let mut plan = Plan::with_output(SetLogoOutput { party });
        plan.statements = statements;
        plan.objects.push(format!("vendor:{}", input.id));
        Ok(plan)
    }
}
