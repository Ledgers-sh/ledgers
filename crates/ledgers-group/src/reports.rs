//! The two group reports: the intercompany reconciliation and the group view.
//!
//! The reconciliation reads the books, not the drafts: for each agreement it
//! sums the ledger lines its own entries posted (matched by the `ic-<agreement>-`
//! external id, and by the reversals of those entries), so a difference shows
//! where the two companies' books disagree. The group view is each accessible
//! member's P&L and balance sheet side by side; it does not eliminate
//! intercompany balances, which is consolidation and lives in Ledgers Cloud.

use crate::model::{DocStatus, Side};
use crate::store::{self, AgreementRow, DocRow};
use async_trait::async_trait;
use cratefield_core::{Database, DbError, Row, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One company, as a report names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompanyRef {
    /// The company id.
    pub id: String,
    /// The company's name.
    pub name: String,
}

/// One period's documents, as the reconciliation lists them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeriodStatus {
    /// The period, `YYYY-MM`.
    pub period: String,
    /// The licensor's status.
    pub licensor_status: DocStatus,
    /// The licensee's status.
    pub licensee_status: DocStatus,
    /// The gross royalty.
    pub gross_minor: i64,
    /// The withholding.
    pub wht_minor: i64,
    /// The net the licensee owes.
    pub net_minor: i64,
}

/// One agreement, reconciled between its two companies.
///
/// The balances are the books' own: `receivable` is the licensor's receivable
/// plus its withholding receivable, `payable` the licensee's payable plus its
/// withholding payable. Both are zero after a period settles, so a difference
/// is a disagreement between the books.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairView {
    /// The agreement id.
    pub agreement_id: String,
    /// The group.
    pub group_id: String,
    /// The licensor.
    pub licensor: CompanyRef,
    /// The licensee.
    pub licensee: CompanyRef,
    /// The agreement currency.
    pub currency: String,
    /// The licensor's receivable and withholding receivable, minor units.
    pub receivable_minor: i64,
    /// The licensee's payable and withholding payable, minor units.
    pub payable_minor: i64,
    /// Receivable less payable: zero when the books agree.
    pub balance_difference_minor: i64,
    /// The licensor's royalty income.
    pub income_minor: i64,
    /// The licensee's royalty expense.
    pub expense_minor: i64,
    /// Income less expense: zero when the books agree.
    pub income_difference_minor: i64,
    /// `balanced` when both differences are zero, `difference` otherwise.
    pub status: String,
    /// Periods that are posted but not yet settled.
    pub open_periods: Vec<String>,
    /// Every period, in order.
    pub periods: Vec<PeriodStatus>,
}

/// Output of `reports.intercompany_reconciliation`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReconciliationOutput {
    /// The group.
    pub group_id: String,
    /// One entry per agreement whose both companies the person may see.
    pub pairs: Vec<PairView>,
}

/// One ledger total: a company's account, for one external id, summed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerTotal {
    /// The company.
    pub company_id: String,
    /// The external id of the entry (or of the entry it reverses).
    pub external_id: String,
    /// The account number.
    pub account: String,
    /// The debit-positive total, minor units.
    pub total: i64,
}

fn text(row: &Row, column: &str) -> Result<String, DbError> {
    row.get::<String>(column)
        .ok_or_else(|| DbError::Query(format!("report row lacks {column}")))
}

/// The intercompany ledger totals of `companies`: every line of an entry
/// whose external id starts `ic-`, or that reverses such an entry, grouped by
/// company, external id and account. One query, whatever the agreement count.
///
/// # Errors
/// A database error.
pub async fn ic_ledger_totals(
    db: &dyn Database,
    companies: &[String],
) -> Result<Vec<LedgerTotal>, DbError> {
    if companies.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; companies.len()].join(", ");
    let stmt = Statement::with_values(
        format!(
            "SELECT je.company_id AS company_id, COALESCE(je.external_id, orig.external_id) AS ext, \
             jl.account_number AS account, CAST(SUM(jl.base_minor) AS BIGINT) AS total \
             FROM journal_line jl \
             JOIN journal_entry je ON je.id = jl.entry_id \
             LEFT JOIN journal_entry orig ON orig.id = je.reverses \
             WHERE je.company_id IN ({marks}) AND (je.external_id LIKE 'ic-%' OR orig.external_id LIKE 'ic-%') \
             GROUP BY je.company_id, COALESCE(je.external_id, orig.external_id), jl.account_number"
        ),
        companies.iter().map(|c| Value::from(c.as_str())).collect(),
    );
    let rows = db.query(&stmt).await?;
    let mut out = Vec::new();
    for row in &rows.rows {
        let Some(external_id) = row.get::<String>("ext") else {
            continue;
        };
        out.push(LedgerTotal {
            company_id: text(row, "company_id")?,
            external_id,
            account: text(row, "account")?,
            total: row.get::<i64>("total").unwrap_or(0),
        });
    }
    Ok(out)
}

/// The agreement id an `ic-<agreement>-...` external id names.
#[must_use]
pub fn agreement_of_external(external_id: &str) -> Option<&str> {
    let mut parts = external_id.split('-');
    (parts.next()? == "ic").then(|| parts.next()).flatten()
}

/// The rows of every agreement a group holds, both sides, in one query.
async fn agreements_of_group(
    db: &dyn Database,
    group_id: &str,
) -> Result<Vec<AgreementRow>, DbError> {
    let stmt = Statement::with_values(
        format!(
            "{} WHERE group_id = ? ORDER BY id, side",
            store::AGREEMENT_SELECT
        ),
        vec![Value::from(group_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(store::agreement_from_row).collect()
}

/// The document rows of the given agreements, in one query.
async fn docs_of(db: &dyn Database, agreements: &[String]) -> Result<Vec<DocRow>, DbError> {
    if agreements.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; agreements.len()].join(", ");
    let stmt = Statement::with_values(
        format!(
            "{} WHERE agreement_id IN ({marks}) ORDER BY period, side",
            store::DOC_SELECT
        ),
        agreements.iter().map(|a| Value::from(a.as_str())).collect(),
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(store::doc_from_row).collect()
}

fn company_ref(name: &str, id: &str) -> CompanyRef {
    CompanyRef {
        id: id.to_owned(),
        name: name.to_owned(),
    }
}

/// `reports.intercompany_reconciliation`: each agreement of the group, both
/// companies of which the person may see.
pub struct ReconciliationReport {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for ReconciliationReport {
    const NAME: &'static str = "reports.intercompany_reconciliation";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = crate::commands::ReconciliationInput;
    type Output = ReconciliationOutput;

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
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let rows = agreements_of_group(db, &input.id).await?;
        let mut by_agreement: BTreeMap<String, (Option<AgreementRow>, Option<AgreementRow>)> =
            BTreeMap::new();
        for row in rows {
            let entry = by_agreement.entry(row.view.id.clone()).or_default();
            match row.view.side {
                Side::Licensor => entry.0 = Some(row),
                Side::Licensee => entry.1 = Some(row),
            }
        }
        // Only pairs both of whose companies the person may see.
        let mut pairs: Vec<(String, AgreementRow, AgreementRow)> = Vec::new();
        for (id, (licensor, licensee)) in by_agreement {
            if let (Some(licensor), Some(licensee)) = (licensor, licensee)
                && self.accessible.contains(&licensor.company_id)
                && self.accessible.contains(&licensee.company_id)
            {
                pairs.push((id, licensor, licensee));
            }
        }
        let companies: Vec<String> = pairs
            .iter()
            .flat_map(|(_, l, e)| [l.company_id.clone(), e.company_id.clone()])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let totals = ic_ledger_totals(db, &companies).await?;
        // One name read per company, however many agreements there are.
        let names = store::company_names(db, &companies).await?;
        let name_of = |company: &str| names.get(company).cloned().unwrap_or_default();
        let ids: Vec<String> = pairs.iter().map(|(id, _, _)| id.clone()).collect();
        let docs = docs_of(db, &ids).await?;

        let mut out = Vec::new();
        for (id, licensor, licensee) in &pairs {
            let amount = |company: &str, account: Option<&String>| -> i64 {
                let Some(account) = account else {
                    return 0;
                };
                totals
                    .iter()
                    .filter(|t| t.company_id == company && t.account == *account)
                    .filter(|t| agreement_of_external(&t.external_id) == Some(id.as_str()))
                    .map(|t| t.total)
                    .sum()
            };
            let lic = &licensor.view.accounts;
            let lee = &licensee.view.accounts;
            let receivable = amount(&licensor.company_id, Some(&lic.ic))
                + amount(&licensor.company_id, lic.wht.as_ref());
            let payable = -(amount(&licensee.company_id, Some(&lee.ic))
                + amount(&licensee.company_id, lee.wht.as_ref()));
            let income = -amount(&licensor.company_id, Some(&lic.royalty));
            let expense = amount(&licensee.company_id, Some(&lee.royalty));
            let balance_difference = receivable - payable;
            let income_difference = income - expense;
            let mut periods: Vec<PeriodStatus> = Vec::new();
            for doc in docs.iter().filter(|d| &d.agreement_id == id) {
                if let Some(period) = periods.iter_mut().find(|p| p.period == doc.view.period) {
                    match doc.view.side {
                        Side::Licensor => period.licensor_status = doc.view.status,
                        Side::Licensee => period.licensee_status = doc.view.status,
                    }
                } else {
                    let (licensor_status, licensee_status) = match doc.view.side {
                        Side::Licensor => (doc.view.status, DocStatus::Draft),
                        Side::Licensee => (DocStatus::Draft, doc.view.status),
                    };
                    periods.push(PeriodStatus {
                        period: doc.view.period.clone(),
                        licensor_status,
                        licensee_status,
                        gross_minor: doc.view.gross_minor,
                        wht_minor: doc.view.wht_minor,
                        net_minor: doc.view.net_minor,
                    });
                }
            }
            let open_periods = periods
                .iter()
                .filter(|p| {
                    p.licensor_status == DocStatus::Posted || p.licensee_status == DocStatus::Posted
                })
                .map(|p| p.period.clone())
                .collect();
            let balanced = balance_difference == 0 && income_difference == 0;
            out.push(PairView {
                agreement_id: id.clone(),
                group_id: input.id.clone(),
                licensor: company_ref(&name_of(&licensor.company_id), &licensor.company_id),
                licensee: company_ref(&name_of(&licensee.company_id), &licensee.company_id),
                currency: licensor.view.currency.clone(),
                receivable_minor: receivable,
                payable_minor: payable,
                balance_difference_minor: balance_difference,
                income_minor: income,
                expense_minor: expense,
                income_difference_minor: income_difference,
                status: if balanced { "balanced" } else { "difference" }.to_owned(),
                open_periods,
                periods,
            });
        }
        Ok(Plan::with_output(ReconciliationOutput {
            group_id: input.id.clone(),
            pairs: out,
        }))
    }
}

/// One member's P&L and balance sheet in the group view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberFigures {
    /// The company.
    pub company: CompanyRef,
    /// What the member is in the group.
    pub kind: crate::model::GroupKind,
    /// The ownership in basis points.
    pub ownership_bp: i64,
    /// The company's base currency.
    pub currency: String,
    /// Revenue for the period, minor units.
    pub revenue_minor: i64,
    /// Expenses for the period, minor units.
    pub expenses_minor: i64,
    /// Net income for the period, minor units.
    pub net_income_minor: i64,
    /// Total assets at the as-of date.
    pub assets_minor: i64,
    /// Total liabilities at the as-of date.
    pub liabilities_minor: i64,
    /// Total equity at the as-of date, including the year's result.
    pub equity_minor: i64,
    /// Whether the sheet balances.
    pub balanced: bool,
}

/// Output of `reports.group_view`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GroupViewOutput {
    /// The group.
    pub group_id: String,
    /// The group name.
    pub name: String,
    /// The period of the P&L.
    pub period: String,
    /// The balance sheet date.
    pub as_of: String,
    /// The members the person may see, side by side.
    pub members: Vec<MemberFigures>,
    /// Consolidation is not in the core; it is a Ledgers Cloud feature.
    pub consolidated: String,
}

/// `reports.group_view`, with the companies the person may see.
pub struct GroupViewReport {
    /// The companies the person may see.
    pub accessible: Vec<String>,
}

#[async_trait]
impl Command for GroupViewReport {
    const NAME: &'static str = "reports.group_view";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const IDEMPOTENT: bool = true;

    type Input = crate::commands::GroupViewInput;
    type Output = GroupViewOutput;

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
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let group = store::group_by_id(db, &input.id)
            .await?
            .ok_or_else(|| CommandError::Domain(cratefield_core::Problem::not_found()))?;
        let members = store::members_of(db, &input.id).await?;
        let visible: Vec<_> = members
            .iter()
            .filter(|m| self.accessible.contains(&m.company_id))
            .collect();
        let ids: Vec<String> = visible.iter().map(|m| m.company_id.clone()).collect();
        let names = store::company_names(db, &ids).await?;
        let mut figures = Vec::new();
        for member in visible {
            let kind = crate::model::GroupKind::parse(&member.kind).ok_or_else(|| {
                CommandError::InvalidInput(format!("unknown member kind {}", member.kind))
            })?;
            let name = names.get(&member.company_id).cloned().unwrap_or_default();
            let currency = store::base_currency_of(db, &member.company_id)
                .await?
                .unwrap_or_default();
            let pnl =
                ledgers_ledger::report::profit_and_loss(db, &member.company_id, &input.period)
                    .await
                    .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
            let sheet = ledgers_ledger::report::balance_sheet(db, &member.company_id, &input.as_of)
                .await
                .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
            figures.push(MemberFigures {
                company: company_ref(&name, &member.company_id),
                kind,
                ownership_bp: member.ownership_bp,
                currency,
                revenue_minor: to_i64(pnl.revenue.minor())?,
                expenses_minor: to_i64(pnl.expenses.minor())?,
                net_income_minor: to_i64(pnl.net_income.minor())?,
                assets_minor: to_i64(sheet.assets.minor())?,
                liabilities_minor: to_i64(sheet.liabilities.minor())?,
                equity_minor: to_i64(sheet.equity.minor())?,
                balanced: sheet.balanced,
            });
        }
        Ok(Plan::with_output(GroupViewOutput {
            group_id: group.id,
            name: group.name,
            period: input.period.clone(),
            as_of: input.as_of.clone(),
            members: figures,
            consolidated: "not available: consolidation is a Ledgers Cloud feature".to_owned(),
        }))
    }
}

fn to_i64(value: i128) -> Result<i64, CommandError> {
    i64::try_from(value)
        .map_err(|_| CommandError::InvalidInput("an amount does not fit the report".into()))
}
