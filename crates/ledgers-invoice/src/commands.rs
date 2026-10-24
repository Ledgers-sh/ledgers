//! Commands for issuing invoices and credit notes.

use crate::model::{
    CreditNoteInput, CreditNoteOutput, CreditSpec, InvoiceTotals, IssueInvoiceInput,
    IssueInvoiceOutput,
};
use crate::store;
use async_trait::async_trait;
use cratefield_core::{Database, Statement};
use ledgers_commands::auth::Tier;
use ledgers_commands::{Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan};
use ledgers_db::guard as db_guard;
use ledgers_ledger::surface::{
    PostEntryInput, SerializableAmount, SerializableEntryDraft, SerializableLine,
    plan_post_entry_with_control,
};
use ledgers_money::{Amount, Currency, Rate, Rounding};
use ledgers_subledger::builders;
use ledgers_tax::{
    calc::{Basis, CalculateParams, TaxLineInput},
    code::TaxKind,
    country::{generic_pack, pack_for_country},
};
use sea_query::Value;
use std::collections::BTreeMap;
use time::Date;

/// Issue an invoice command.
#[derive(Clone, Debug)]
pub struct IssueInvoice;

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for IssueInvoice {
    const NAME: &'static str = "invoices.issue";
    const TIER: Tier = Tier::PostWithinLimits;

    type Input = IssueInvoiceInput;
    type Output = IssueInvoiceOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Validate input early
        validate_invoice_input(input)?;

        // Build entry to get the same lines that will be posted
        let (entry, _) = entry_for_invoice(db, ctx, input).await?;

        // Create a PostEntryInput and get its preview
        let journal_id = find_journal_for_company(db, &ctx.company_id).await?;
        let fiscal_year =
            extract_fiscal_year_for_date(db, &ctx.company_id, &input.issue_date).await?;
        let post_entry_input = PostEntryInput {
            journal_id,
            fiscal_year,
            entry,
        };

        // Get the preview from the ledger's PostEntry command
        ledgers_ledger::surface::PostEntry
            .preview(db, ctx, &post_entry_input)
            .await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Validate invoice lines
        validate_invoice_input(input)?;

        let fiscal_year =
            extract_fiscal_year_for_date(db, &ctx.company_id, &input.issue_date).await?;
        let supply_date = input
            .supply_date
            .as_deref()
            .unwrap_or(input.issue_date.as_str())
            .to_owned();
        let currency = input.currency.as_deref().unwrap_or("EUR").to_string();
        let base_currency = load_base_currency(db, &ctx.company_id).await?;

        // Build entry using shared function (validates customer, loads chart, calculates tax)
        let (entry_draft, (net_total, tax_total, gross_minor)) =
            entry_for_invoice(db, ctx, input).await?;

        // Load chart to get AR control account
        let chart = load_chart(db, &ctx.company_id).await?;
        let ar_control = chart.receivable.clone().ok_or_else(|| {
            CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
        })?;

        // Check VIES for any intra-EU zero-rated lines
        let pack = pack_for_country(&load_company_country(db, &ctx.company_id).await?)
            .map_err(|e| CommandError::InvalidInput(format!("tax regime error: {e}")))?
            .unwrap_or_else(|| generic_pack().expect("generic pack always available"));
        let vies_check_id = check_vies_for_intra_eu_lines(db, ctx, input, &pack).await?;

        // Handle foreign currency rate if needed
        let rate_scaled: Option<String> = if currency == base_currency {
            // If rate is provided but currency is base, it must be 1 or absent
            if let Some(rate_str) = &input.rate {
                let parsed_rate = rate_str
                    .parse::<Rate>()
                    .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?;
                if parsed_rate != Rate::ONE {
                    return Err(CommandError::InvalidInput(
                        "rate must be 1 or absent when currency is base".into(),
                    ));
                }
            }
            None
        } else {
            Some(input.rate.clone().ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "invoice in {currency} requires exchange rate to {base_currency}"
                ))
            })?)
        };

        // Calculate base amounts if currency differs
        let (gross_minor_tx, gross_minor_base) = if currency == base_currency {
            (gross_minor, gross_minor)
        } else {
            let rate_obj = input
                .rate
                .as_ref()
                .ok_or_else(|| {
                    CommandError::InvalidInput("rate required for non-base currency".into())
                })?
                .parse::<Rate>()
                .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?;
            let gross_amount = Amount::new(
                i128::from(gross_minor),
                Currency::from_code(&currency)
                    .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?,
            );
            let base_gross = gross_amount
                .mul_rate(rate_obj, Rounding::HalfEven)
                .map_err(|e| CommandError::InvalidInput(format!("rate conversion failed: {e}")))?;
            let base_minor_i64 = i64::try_from(base_gross.minor())
                .map_err(|_| CommandError::InvalidInput("base amount overflow".into()))?;
            (gross_minor, base_minor_i64)
        };

        // Post the entry
        let posted = plan_post_entry_with_control(
            token,
            db,
            ctx,
            &PostEntryInput {
                journal_id: find_journal_for_company(db, &ctx.company_id).await?,
                fiscal_year,
                entry: entry_draft,
            },
        )
        .await?;

        let mut statements = posted.statements;
        let entry_id = posted.output.entry.entry_id.clone();

        // Read counter before batch
        let next_number =
            store::read_invoice_counter(db, &ctx.company_id, &input.series, fiscal_year)
                .await
                .map_err(CommandError::Domain)?;

        // Generate IDs
        let invoice_id = format!("inv_{}", ctx.event_id);
        let posting_date = ctx.now.get(..10).unwrap_or("").to_owned();
        let item_id = format!("oi_{}", ctx.event_id);
        let invoice_number = format!("{}-{}-{:04}", input.series, fiscal_year, next_number);

        // Calculate due date
        let payment_terms = input.payment_terms_days.unwrap_or(14);
        let due_date = add_days(&input.issue_date, i64::from(payment_terms))?;

        // Create AR open_item with calculated gross (tx and base)
        let item_insert = builders::build_open_item_insert(
            &item_id,
            &ctx.company_id,
            "ar",
            "invoice",
            &input.customer_id,
            Some(&invoice_number),
            &entry_id,
            &ar_control,
            &currency,
            rate_scaled.as_deref(),
            input.issue_date.as_str(),
            &posting_date,
            Some(&due_date),
            gross_minor_tx,
            gross_minor_base,
            &ctx.now,
        );
        statements.push(item_insert);

        // Provision, bump, and guard counter
        let (_, counter_upsert, counter_bump, counter_guard) = provision_bump_counter(
            &ctx.company_id,
            "invoice_number_counter",
            &input.series,
            fiscal_year,
            next_number,
            &ctx.event_id,
        );
        statements.push(counter_upsert);
        statements.push(counter_bump);
        statements.push(counter_guard);

        // Insert invoice
        let invoice_insert = Statement::with_values(
            "INSERT INTO invoice (id, company_id, customer_id, series, number, fiscal_year, \
             issue_date, supply_date, due_date, currency, rate_scaled, vies_check_id, posted_entry_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                Value::from(invoice_id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.customer_id.as_str()),
                Value::from(input.series.as_str()),
                Value::from(next_number),
                Value::from(fiscal_year),
                Value::from(input.issue_date.as_str()),
                Value::from(supply_date.as_str()),
                Value::from(due_date.as_str()),
                Value::from(&currency),
                rate_scaled.as_ref()
                    .map_or_else(|| Value::String(None), |r| Value::from(r.as_str())),
                vies_check_id.as_ref()
                    .map_or_else(|| Value::String(None), |id| Value::from(id.as_str())),
                Value::from(entry_id.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        );
        statements.push(invoice_insert);

        // Insert invoice lines (resolve revenue account once per line before insert)
        for (seq, line_input) in input.lines.iter().enumerate() {
            let line_id = format!("inv_line_{}_{}", ctx.event_id, seq);
            // net_minor is validated to exist and be positive in validate_invoice_input
            let net_minor = line_input.net_minor.unwrap();

            // Resolve revenue account: from input, or default from customer, or error
            let revenue_account = if let Some(acct) = &line_input.revenue_account {
                if acct.is_empty() {
                    return Err(CommandError::InvalidInput(
                        "revenue account cannot be empty string".into(),
                    ));
                }
                acct.clone()
            } else {
                load_customer_default_account(db, &ctx.company_id, &input.customer_id)
                    .await?
                    .ok_or_else(|| {
                        CommandError::InvalidInput(format!(
                            "no revenue account for line; customer {} has no default",
                            input.customer_id
                        ))
                    })?
            };

            #[allow(clippy::cast_possible_wrap)]
            let sequence = (seq + 1) as i64;

            let line_insert = Statement::with_values(
                "INSERT INTO invoice_line (id, invoice_id, company_id, sequence, description, \
                 quantity_scaled, unit, unit_price_minor, net_minor, tax_code, revenue_account, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(line_id.as_str()),
                    Value::from(invoice_id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(sequence),
                    Value::from(line_input.description.as_str()),
                    Value::from(line_input.quantity.as_str()),
                    line_input.unit.as_ref()
                        .map_or_else(|| Value::String(None), |u| Value::from(u.as_str())),
                    line_input.unit_price_minor
                        .map_or_else(|| Value::String(None), Value::from),
                    Value::from(net_minor),
                    Value::from(line_input.tax_code.as_str()),
                    Value::from(revenue_account.as_str()),
                    Value::from(ctx.now.as_str()),
                ],
            );
            statements.push(line_insert);
        }

        let out = Plan::with_output(IssueInvoiceOutput {
            invoice_id,
            invoice_number,
            entry_id,
            totals: InvoiceTotals {
                net_minor: net_total,
                tax_minor: tax_total,
                gross_minor,
                currency,
            },
            due_date,
        });

        let mut plan = out;
        plan.statements = statements;
        Ok(plan)
    }
}

/// Credit note command.
#[derive(Clone, Debug)]
pub struct CreditInvoice;

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Command for CreditInvoice {
    const NAME: &'static str = "invoices.credit";
    const TIER: Tier = Tier::PostWithinLimits;

    type Input = CreditNoteInput;
    type Output = CreditNoteOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Validate input early
        validate_credit_input(input)?;

        // Build entry to get the same lines that will be posted
        let (entry, _) = entry_for_credit(db, ctx, input).await?;

        // Create a PostEntryInput and get its preview
        let journal_id = find_journal_for_company(db, &ctx.company_id).await?;
        let fiscal_year =
            extract_fiscal_year_for_date(db, &ctx.company_id, &input.issue_date).await?;
        let post_entry_input = PostEntryInput {
            journal_id,
            fiscal_year,
            entry,
        };

        // Get the preview from the ledger's PostEntry command
        ledgers_ledger::surface::PostEntry
            .preview(db, ctx, &post_entry_input)
            .await
    }

    async fn plan(
        &self,
        token: &ExecToken,
        db: &dyn Database,
        ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Validate credit input
        validate_credit_input(input)?;

        let fiscal_year =
            extract_fiscal_year_for_date(db, &ctx.company_id, &input.issue_date).await?;

        // Load original invoice (needed for details like currency)
        let invoice = store::load_invoice(db, &ctx.company_id, &input.invoice_id)
            .await
            .map_err(CommandError::Domain)?
            .ok_or_else(|| CommandError::InvalidInput("invoice not found".into()))?;

        // Build entry and get credit lines using shared function
        let (entry_draft, (credit_lines, total_credited, tax_minor)) =
            entry_for_credit(db, ctx, input).await?;

        // Load chart to get AR control account
        let chart = load_chart(db, &ctx.company_id).await?;
        let ar_control = chart
            .receivable
            .as_ref()
            .ok_or_else(|| {
                CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
            })?
            .clone();

        // Post the entry
        let posted = plan_post_entry_with_control(
            token,
            db,
            ctx,
            &PostEntryInput {
                journal_id: find_journal_for_company(db, &ctx.company_id).await?,
                fiscal_year,
                entry: entry_draft,
            },
        )
        .await?;

        let mut statements = posted.statements;
        let entry_id = posted.output.entry.entry_id.clone();

        // Read counter before batch
        let next_number =
            store::read_credit_note_counter(db, &ctx.company_id, &input.series, fiscal_year)
                .await
                .map_err(CommandError::Domain)?;

        // Create credit-note open_item (negative balance)
        let item_id = format!("oi_{}", ctx.event_id);
        let posting_date = ctx.now.get(..10).unwrap_or("").to_owned();
        let cn_number_str = format!("{}-{}-{:04}", input.series, fiscal_year, next_number);

        // Calculate gross amount (net + tax already calculated in entry_for_credit)
        let gross_minor = total_credited + tax_minor;

        let item_insert = builders::build_open_item_insert(
            &item_id,
            &ctx.company_id,
            "ar",
            "credit-note",
            &invoice.customer_id,
            Some(&cn_number_str),
            &entry_id,
            &ar_control,
            &invoice.currency,
            invoice.rate_scaled.as_deref(),
            input.issue_date.as_str(),
            &posting_date,
            None,
            -gross_minor,
            -gross_minor,
            &ctx.now,
        );
        statements.push(item_insert);

        // Provision, bump, and guard counter
        let (_, counter_upsert, counter_bump, counter_guard) = provision_bump_counter(
            &ctx.company_id,
            "credit_note_number_counter",
            &input.series,
            fiscal_year,
            next_number,
            &ctx.event_id,
        );
        statements.push(counter_upsert);
        statements.push(counter_bump);
        statements.push(counter_guard);

        // Insert credit note
        let credit_note_id = format!("cn_{}", ctx.event_id);
        let cn_insert = Statement::with_values(
            "INSERT INTO credit_note (id, company_id, invoice_id, series, number, fiscal_year, \
             issue_date, reason, posted_entry_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                .to_owned(),
            vec![
                Value::from(credit_note_id.as_str()),
                Value::from(ctx.company_id.as_str()),
                Value::from(input.invoice_id.as_str()),
                Value::from(input.series.as_str()),
                Value::from(next_number),
                Value::from(fiscal_year),
                Value::from(input.issue_date.as_str()),
                input
                    .reason
                    .as_ref()
                    .map_or_else(|| Value::String(None), |r| Value::from(r.as_str())),
                Value::from(entry_id.as_str()),
                Value::from(ctx.now.as_str()),
            ],
        );
        statements.push(cn_insert);

        // Insert credit note lines
        for (line_idx, (orig_line, credited_amount)) in credit_lines.iter().enumerate() {
            let cnl_id = format!("cn_line_{}_{}", ctx.event_id, line_idx);
            let credit_line_stmt = Statement::with_values(
                "INSERT INTO credit_note_line (id, credit_note_id, invoice_line_id, company_id, \
                 net_credited_minor, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    Value::from(cnl_id.as_str()),
                    Value::from(credit_note_id.as_str()),
                    Value::from(orig_line.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(*credited_amount),
                    Value::from(ctx.now.as_str()),
                ],
            );
            statements.push(credit_line_stmt);
        }

        // Add over-credit guard for each credited line (after inserts, evaluated in batch)
        // Guard filters by company_id for tenant isolation
        for (orig_line, _) in &credit_lines {
            let line_guard = db_guard::assert_statement(
                db_guard::guard_key(&[
                    "credit-line-limit",
                    &ctx.company_id,
                    &orig_line.id,
                    &ctx.event_id,
                ]),
                "(SELECT COALESCE(SUM(net_credited_minor), 0) FROM credit_note_line \
                 WHERE company_id = ? AND invoice_line_id = ?) <= (SELECT net_minor FROM invoice_line WHERE company_id = ? AND id = ?)",
                vec![
                    Value::from(ctx.company_id.as_str()),
                    Value::from(orig_line.id.as_str()),
                    Value::from(ctx.company_id.as_str()),
                    Value::from(orig_line.id.as_str()),
                ],
            );
            statements.push(line_guard);
        }

        let out = Plan::with_output(CreditNoteOutput {
            credit_note_id,
            credit_note_number: cn_number_str,
            entry_id,
            total_credited_minor: total_credited,
            currency: invoice.currency,
        });

        let mut plan = out;
        plan.statements = statements;
        Ok(plan)
    }
}

// Helper functions

/// Check if a quantity string is a valid positive decimal (no f64 parsing).
/// Allows: ASCII digits '0'-'9' and at most one '.'.
/// Requires: at least one non-zero digit (strictly > 0).
/// Rejects: signs, exponents, multiple dots.
fn is_valid_positive_quantity(qty: &str) -> bool {
    if qty.is_empty() {
        return false;
    }

    let mut dot_count = 0;
    let mut has_nonzero = false;

    for ch in qty.chars() {
        match ch {
            '0'..='9' => {
                if ch != '0' {
                    has_nonzero = true;
                }
            }
            '.' => {
                dot_count += 1;
                if dot_count > 1 {
                    return false;
                }
            }
            _ => return false,
        }
    }

    has_nonzero && dot_count <= 1
}

/// Validate invoice input.
fn validate_invoice_input(input: &IssueInvoiceInput) -> Result<(), CommandError> {
    if input.lines.is_empty() {
        return Err(CommandError::InvalidInput(
            "invoice must have at least one line".into(),
        ));
    }

    for (idx, line) in input.lines.iter().enumerate() {
        // Validate line amounts are not negative
        if let Some(net) = line.net_minor {
            if net <= 0 {
                return Err(CommandError::InvalidInput(format!(
                    "line {idx} net_minor must be positive, got {net}"
                )));
            }
        } else {
            return Err(CommandError::InvalidInput(format!(
                "line {idx} must have net_minor"
            )));
        }

        if !line.quantity.is_empty() && !is_valid_positive_quantity(&line.quantity) {
            return Err(CommandError::InvalidInput(format!(
                "line {idx} quantity must not be negative"
            )));
        }

        // If unit_price_minor is provided, it must be positive
        if let Some(up) = line.unit_price_minor
            && up <= 0
        {
            return Err(CommandError::InvalidInput(format!(
                "line {idx} unit_price_minor must be positive"
            )));
        }
    }

    Ok(())
}

/// Validate credit note input.
fn validate_credit_input(input: &CreditNoteInput) -> Result<(), CommandError> {
    match &input.credit_spec {
        CreditSpec::Lines { lines } => {
            for spec in lines {
                if spec.net_minor <= 0 {
                    return Err(CommandError::InvalidInput(format!(
                        "credit line net_minor must be positive, got {}",
                        spec.net_minor
                    )));
                }
            }
        }
        CreditSpec::Amount { amount_minor } => {
            if *amount_minor <= 0 {
                return Err(CommandError::InvalidInput(format!(
                    "credit amount must be positive, got {amount_minor}"
                )));
            }
        }
    }
    Ok(())
}

/// Build entry for invoice (used in both preview and plan).
/// Returns the entry draft and calculated totals (net, tax, gross).
#[allow(clippy::too_many_arguments)]
async fn entry_for_invoice(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &IssueInvoiceInput,
) -> Result<(SerializableEntryDraft, (i64, i64, i64)), CommandError> {
    let currency = input.currency.as_deref().unwrap_or("EUR").to_string();
    let base_currency = load_base_currency(db, &ctx.company_id).await?;

    // Validate customer exists and has proper role
    validate_customer(db, ctx, input).await?;

    // Load chart
    let chart = load_chart(db, &ctx.company_id).await?;
    chart.receivable.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
    })?;
    chart.vat_output.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no output VAT account (tag vat-output)".into())
    })?;

    // Load company tax settings
    let company_country = load_company_country(db, &ctx.company_id).await?;
    let pack = pack_for_country(&company_country)
        .map_err(|e| CommandError::InvalidInput(format!("tax regime error: {e}")))?
        .unwrap_or_else(|| generic_pack().expect("generic pack always available"));

    // Calculate totals
    let (net_total, tax_total, gross_total, _tax_lines) =
        calculate_invoice_totals_with_tax(db, ctx, input, &chart, &pack, &currency).await?;
    let gross_minor = gross_total;

    // Validate rate if needed
    if currency != base_currency {
        if let Some(rate_str) = &input.rate {
            let _parsed_rate = rate_str
                .parse::<Rate>()
                .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?;
        } else {
            return Err(CommandError::InvalidInput(format!(
                "invoice in {currency} requires exchange rate to {base_currency}"
            )));
        }
    } else if let Some(rate_str) = &input.rate {
        let parsed_rate = rate_str
            .parse::<Rate>()
            .map_err(|e| CommandError::InvalidInput(format!("invalid rate: {e}")))?;
        if parsed_rate != Rate::ONE {
            return Err(CommandError::InvalidInput(
                "rate must be 1 or absent when currency is base".into(),
            ));
        }
    }

    // Build the entry
    let entry = build_entry_for_invoice(
        db,
        ctx,
        input,
        &chart,
        net_total,
        tax_total,
        gross_minor,
        &currency,
        &pack,
    )
    .await?;

    Ok((entry, (net_total, tax_total, gross_minor)))
}

/// Build entry for credit note (used in both preview and plan).
/// Returns the entry draft, credit lines, total credited amount, and tax amount.
async fn entry_for_credit(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &CreditNoteInput,
) -> Result<
    (
        SerializableEntryDraft,
        (Vec<(store::InvoiceLineRow, i64)>, i64, i64),
    ),
    CommandError,
> {
    // Load original invoice
    let invoice = store::load_invoice(db, &ctx.company_id, &input.invoice_id)
        .await
        .map_err(CommandError::Domain)?
        .ok_or_else(|| CommandError::InvalidInput("invoice not found".into()))?;

    let lines = store::load_invoice_lines(db, &ctx.company_id, &input.invoice_id)
        .await
        .map_err(CommandError::Domain)?;

    // Determine which lines to credit and validate over-credit (pre-read for nice error)
    let mut total_credited = 0i64;
    let mut credit_lines = Vec::new();

    match &input.credit_spec {
        CreditSpec::Lines { lines: specified } => {
            for spec in specified {
                let orig_line = lines
                    .iter()
                    .find(|l| l.id == spec.invoice_line_id)
                    .ok_or_else(|| {
                        CommandError::InvalidInput(format!(
                            "invoice line {} not found",
                            spec.invoice_line_id
                        ))
                    })?;

                // Pre-read check for nicer error message (guard enforces atomically)
                let already_credited =
                    store::total_credited_for_line(db, &ctx.company_id, &spec.invoice_line_id)
                        .await
                        .map_err(CommandError::Domain)?;

                if already_credited + spec.net_minor > orig_line.net_minor {
                    return Err(CommandError::InvalidInput(format!(
                        "line {} cannot credit {} (already {} of {})",
                        spec.invoice_line_id, spec.net_minor, already_credited, orig_line.net_minor
                    )));
                }

                credit_lines.push((orig_line.clone(), spec.net_minor));
                total_credited += spec.net_minor;
            }
        }
        CreditSpec::Amount { amount_minor } => {
            // Spread pro-rata across lines
            let total_net: i64 = lines.iter().map(|l| l.net_minor).sum();
            if total_net == 0 {
                return Err(CommandError::InvalidInput(
                    "cannot allocate credit across zero-net invoice".into(),
                ));
            }

            let mut remaining = *amount_minor;
            for (idx, line) in lines.iter().enumerate() {
                if remaining <= 0 {
                    break;
                }

                let share = if idx == lines.len() - 1 {
                    remaining
                } else {
                    (*amount_minor * line.net_minor) / total_net
                };

                let to_credit = share.min(line.net_minor);
                credit_lines.push((line.clone(), to_credit));
                total_credited += to_credit;
                remaining -= to_credit;
            }
        }
    }

    // Load chart
    let chart = load_chart(db, &ctx.company_id).await?;
    chart.receivable.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
    })?;
    chart.vat_output.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no output VAT account (tag vat-output)".into())
    })?;

    // Load company settings for tax calculation
    let company_country = load_company_country(db, &ctx.company_id).await?;
    let pack = pack_for_country(&company_country)
        .map_err(|e| CommandError::InvalidInput(format!("tax regime error: {e}")))?
        .unwrap_or_else(|| generic_pack().expect("generic pack always available"));

    // Calculate tax for credit note
    let tax_minor = calculate_tax_for_credit_lines(db, ctx, &invoice, &credit_lines, &pack).await?;

    // Build entry for credit note
    let entry = build_entry_for_credit(db, ctx, &invoice, &credit_lines, &chart, &pack).await?;

    Ok((entry, (credit_lines, total_credited, tax_minor)))
}

async fn load_base_currency(db: &dyn Database, company_id: &str) -> Result<String, CommandError> {
    let stmt = Statement::with_values(
        "SELECT base_currency FROM company WHERE id = ?".to_owned(),
        vec![Value::from(company_id)],
    );

    let rows = db.query(&stmt).await?;
    rows.first()
        .and_then(|r| r.get::<String>("base_currency"))
        .ok_or_else(|| CommandError::InvalidInput("the company has no base currency".into()))
}

async fn find_journal_for_company(
    db: &dyn Database,
    company_id: &str,
) -> Result<String, CommandError> {
    // Use ledgers-ledger's journal finder instead of hard-coding "GN"
    ledgers_ledger::journal::find_journal(db, company_id, "GN")
        .await
        .map_err(|e| CommandError::InvalidInput(format!("journal error: {e}")))?
        .map(|j| j.id)
        .ok_or_else(|| CommandError::InvalidInput("no general journal GN exists".into()))
}

/// Calculate tax amounts for lines using real ledgers-tax machinery.
/// Calculates taxes for invoice or credit scenarios.
/// Returns (`net_total`, `tax_total`, `gross_total`)
async fn calculate_tax_amounts(
    db: &dyn Database,
    ctx: &CommandContext,
    tax_inputs: &[TaxLineInput],
    issue_date_str: &str,
    _currency: &str,
    chart: &Chart,
    pack: &ledgers_tax::country::CountryPack,
) -> Result<(i64, i64, i64), CommandError> {
    // Parse issue date for tax rate lookup
    let issue_date = Date::parse(
        issue_date_str,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| CommandError::InvalidInput("invalid issue date format".into()))?;

    // Get company's country for jurisdiction
    let company_country = load_company_country(db, &ctx.company_id).await?;
    let jurisdiction = ledgers_tax::rate::Jurisdiction::new(&company_country);

    // Load company's custom rates: company_rate table is optional, return empty if not defined
    let company_rates = Vec::new();
    let rate_table = pack
        .rate_table(&company_rates)
        .map_err(|e| CommandError::InvalidInput(format!("tax rate table error: {e}")))?;

    // Build tax codes from chart tags
    let tax_codes = pack
        .tax_codes(&chart.accounts_by_tag)
        .map_err(|e| CommandError::InvalidInput(format!("tax code error: {e}")))?;

    let code_lookup = make_code_lookup(&tax_codes);

    // Calculate taxes
    let calc_params = CalculateParams {
        lines: tax_inputs,
        date: issue_date,
        table: &rate_table,
        jurisdiction: &jurisdiction,
        code_lookup: &code_lookup,
        basis: Basis::Net,
        scope: pack.rounding,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let tax_result = ledgers_tax::calculate(&calc_params)
        .map_err(|e| CommandError::InvalidInput(format!("tax calculation error: {e}")))?;

    // Sum totals
    let mut net_total = 0i64;
    let mut tax_total = 0i64;

    for (_, tax_line) in &tax_result.lines {
        net_total += i64::try_from(tax_line.net.minor())
            .map_err(|_| CommandError::InvalidInput("net amount overflow".into()))?;

        let vat_minor = i64::try_from(tax_line.output_vat.minor())
            .map_err(|_| CommandError::InvalidInput("output vat overflow".into()))?;
        tax_total += vat_minor;
    }

    Ok((net_total, tax_total, net_total + tax_total))
}

/// Calculate invoice totals using real ledgers-tax machinery (for `IssueInvoice` command).
/// Returns (`net_total`, `tax_total`, `gross_total`, `tax_lines_by_account`)
async fn calculate_invoice_totals_with_tax(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &IssueInvoiceInput,
    chart: &Chart,
    pack: &ledgers_tax::country::CountryPack,
    currency: &str,
) -> Result<(i64, i64, i64, Vec<(String, i64, i64)>), CommandError> {
    if input.lines.is_empty() {
        return Err(CommandError::InvalidInput(
            "invoice must have at least one line".into(),
        ));
    }

    let curr = Currency::from_code(currency)
        .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?;

    // Prepare tax lines for calculation
    let mut tax_inputs = Vec::new();
    for line in &input.lines {
        let net = line.net_minor.ok_or_else(|| {
            CommandError::InvalidInput("each invoice line must have net_minor".into())
        })?;
        tax_inputs.push(TaxLineInput {
            amount: Amount::new(i128::from(net), curr),
            tax_code: line.tax_code.clone(),
        });
    }

    let (net_total, tax_total, gross_total) = calculate_tax_amounts(
        db,
        ctx,
        &tax_inputs,
        &input.issue_date,
        currency,
        chart,
        pack,
    )
    .await?;

    // Group tax by account for VAT lines
    let tax_codes = pack
        .tax_codes(&chart.accounts_by_tag)
        .map_err(|e| CommandError::InvalidInput(format!("tax code error: {e}")))?;

    let company_country = load_company_country(db, &ctx.company_id).await?;
    let jurisdiction = ledgers_tax::rate::Jurisdiction::new(&company_country);
    let company_rates = Vec::new();
    let rate_table = pack
        .rate_table(&company_rates)
        .map_err(|e| CommandError::InvalidInput(format!("tax rate table error: {e}")))?;

    let issue_date = Date::parse(
        &input.issue_date,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| CommandError::InvalidInput("invalid issue date format".into()))?;

    let code_lookup = make_code_lookup(&tax_codes);
    let calc_params = CalculateParams {
        lines: &tax_inputs,
        date: issue_date,
        table: &rate_table,
        jurisdiction: &jurisdiction,
        code_lookup: &code_lookup,
        basis: Basis::Net,
        scope: pack.rounding,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let tax_result = ledgers_tax::calculate(&calc_params)
        .map_err(|e| CommandError::InvalidInput(format!("tax calculation error: {e}")))?;

    let mut tax_by_account: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for (input_line, tax_line) in &tax_result.lines {
        let tax_code = code_lookup(&input_line.tax_code)
            .map_err(|e| CommandError::InvalidInput(format!("tax code error: {e}")))?;

        let vat_minor = i64::try_from(tax_line.output_vat.minor())
            .map_err(|_| CommandError::InvalidInput("output vat overflow".into()))?;
        if vat_minor > 0 {
            let account = tax_code.output_account.as_ref().ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "tax code {} has no output account",
                    tax_code.code
                ))
            })?;
            let (n, t) = tax_by_account.entry(account.clone()).or_insert((0, 0));
            *n += i64::try_from(tax_line.net.minor())
                .map_err(|_| CommandError::InvalidInput("net amount overflow".into()))?;
            *t += vat_minor;
        }
    }

    let tax_lines: Vec<_> = tax_by_account
        .into_iter()
        .map(|(acc, (n, t))| (acc, n, t))
        .collect();

    Ok((net_total, tax_total, gross_total, tax_lines))
}

async fn check_vies_for_intra_eu_lines(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &IssueInvoiceInput,
    pack: &ledgers_tax::country::CountryPack,
) -> Result<Option<String>, CommandError> {
    // Get tax codes from pack
    let mut needs_vies = false;

    for line in &input.lines {
        // Find the tax code in the pack
        if let Some(pack_code) = pack.tax_codes.iter().find(|c| c.code == line.tax_code) {
            // VIES check is required for Zero-rated supplies (intra-EU supplies)
            if pack_code.kind == TaxKind::Zero {
                needs_vies = true;
                break;
            }
        } else {
            return Err(CommandError::InvalidInput(format!(
                "unknown tax code {} in pack {}",
                line.tax_code, pack.id
            )));
        }
    }

    if needs_vies {
        // Require a valid VIES check for the customer (filtered by company for tenant isolation)
        let vies_check_id =
            get_latest_valid_vies_check(db, &ctx.company_id, &input.customer_id).await?;
        if vies_check_id.is_none() {
            return Err(CommandError::InvalidInput(format!(
                "intra-EU zero-rated lines require a valid VIES check for customer {}",
                input.customer_id
            )));
        }
        Ok(vies_check_id)
    } else {
        Ok(None)
    }
}

async fn get_latest_valid_vies_check(
    db: &dyn Database,
    company_id: &str,
    customer_id: &str,
) -> Result<Option<String>, CommandError> {
    let stmt = Statement::with_values(
        "SELECT id FROM vies_check WHERE company_id = ? AND vendor_id = ? AND valid = 1 \
         ORDER BY checked_at DESC LIMIT 1"
            .to_owned(),
        vec![Value::from(company_id), Value::from(customer_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows.first().and_then(|r| r.get::<String>("id")))
}

#[allow(clippy::too_many_arguments)]
async fn build_entry_for_invoice(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &IssueInvoiceInput,
    chart: &Chart,
    _net_total: i64,
    tax_total: i64,
    gross_total: i64,
    currency: &str,
    pack: &ledgers_tax::country::CountryPack,
) -> Result<SerializableEntryDraft, CommandError> {
    let currency_str = currency.to_string();
    let posting_date = ctx.now.get(..10).unwrap_or("").to_owned();

    // Build entry lines:
    // 1. Debit AR control (gross amount)
    // 2. Credit revenue accounts (net amounts per line)
    // 3. Credit VAT output accounts (tax amounts, grouped by account)

    let mut lines = Vec::new();

    // Add AR control debit line (gross total)
    let ar_control = chart.receivable.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
    })?;

    lines.push(SerializableLine {
        account_number: ar_control.clone(),
        side: "debit".to_owned(),
        base: SerializableAmount {
            minor: gross_total.to_string(),
            currency: currency_str.clone(),
        },
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    });

    // Load tax codes to get account mappings (for validation, tax amounts already calculated)
    let _tax_codes = pack
        .tax_codes(&chart.accounts_by_tag)
        .map_err(|e| CommandError::InvalidInput(format!("tax code error: {e}")))?;

    for line in &input.lines {
        if let Some(net) = line.net_minor {
            // Revenue account: from input, or default from customer, or error
            let revenue_account = if let Some(acct) = &line.revenue_account {
                acct.clone()
            } else {
                load_customer_default_account(db, &ctx.company_id, &input.customer_id)
                    .await?
                    .ok_or_else(|| {
                        CommandError::InvalidInput(format!(
                            "no revenue account for line; customer {} has no default",
                            input.customer_id
                        ))
                    })?
            };

            lines.push(SerializableLine {
                account_number: revenue_account,
                side: "credit".to_owned(),
                base: SerializableAmount {
                    minor: net.to_string(),
                    currency: currency_str.clone(),
                },
                fx: None,
                tax_code: Some(line.tax_code.clone()),
                dimensions: BTreeMap::new(),
                memo: None,
            });
        }
    }

    // Add VAT output lines (one per unique output account)
    if tax_total != 0 {
        // For simplicity, use the single vat-output account from chart
        // In a multi-rate scenario, this should be split per rate/account
        let vat_account = chart.vat_output.as_ref().ok_or_else(|| {
            CommandError::InvalidInput(
                "the chart has no output VAT account (tag vat-output)".into(),
            )
        })?;

        lines.push(SerializableLine {
            account_number: vat_account.clone(),
            side: "credit".to_owned(),
            base: SerializableAmount {
                minor: tax_total.to_string(),
                currency: currency_str.clone(),
            },
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        });
    }

    Ok(SerializableEntryDraft {
        journal_id: find_journal_for_company(db, &ctx.company_id).await?,
        document_date: input.issue_date.clone(),
        posting_date,
        tax_point_date: None,
        base_currency: currency_str.clone(),
        description: format!("Invoice {}", input.series),
        source_documents: vec![],
        external_id: Some(format!("invoice-{}", ctx.event_id)),
        source_app: Some("ledgers-invoice".to_owned()),
        reverses: None,
        lines,
    })
}

async fn validate_customer(
    db: &dyn Database,
    ctx: &CommandContext,
    input: &IssueInvoiceInput,
) -> Result<(), CommandError> {
    let stmt = Statement::with_values(
        "SELECT id, roles FROM vendor WHERE company_id = ? AND id = ?".to_owned(),
        vec![
            Value::from(ctx.company_id.as_str()),
            Value::from(input.customer_id.as_str()),
        ],
    );

    let rows = db.query(&stmt).await?;

    if let Some(row) = rows.first() {
        let roles = row.get::<String>("roles").unwrap_or_default();
        // Customer must have role 'customer' or 'both'
        if !roles.contains("customer") && !roles.contains("both") {
            return Err(CommandError::InvalidInput(format!(
                "party {} is vendor-only; cannot issue invoice to vendor-only party",
                input.customer_id
            )));
        }
        Ok(())
    } else {
        Err(CommandError::InvalidInput(format!(
            "customer {} not found",
            input.customer_id
        )))
    }
}

#[derive(Clone, Debug, Default)]
struct Chart {
    receivable: Option<String>,
    vat_output: Option<String>,
    accounts_by_tag: BTreeMap<String, String>,
}

async fn load_chart(db: &dyn Database, company_id: &str) -> Result<Chart, CommandError> {
    let stmt = Statement::with_values(
        "SELECT number, tags FROM account WHERE company_id = ?".to_owned(),
        vec![Value::from(company_id)],
    );

    let rows = db.query(&stmt).await?;

    let mut chart = Chart::default();
    for row in rows.rows {
        let number = row.get::<String>("number").unwrap_or_default();
        let tags = row.get::<String>("tags").unwrap_or_default();

        // Split tags and add to map
        for tag in tags.split_whitespace() {
            let tag = tag.trim();
            if !tag.is_empty() {
                chart
                    .accounts_by_tag
                    .insert(tag.to_string(), number.clone());
            }
        }

        if tags.contains("ar") {
            chart.receivable = Some(number.clone());
        }
        if tags.contains("vat-output") {
            chart.vat_output = Some(number.clone());
        }
    }

    Ok(chart)
}

async fn extract_fiscal_year_for_date(
    db: &dyn Database,
    company_id: &str,
    date_str: &str,
) -> Result<i64, CommandError> {
    // Check if there's a fiscal year entry covering this date (portable: no strftime)
    let stmt = Statement::with_values(
        "SELECT starts_on FROM fiscal_year \
         WHERE company_id = ? AND starts_on <= ? AND ends_on >= ? \
         ORDER BY starts_on DESC LIMIT 1"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(date_str),
            Value::from(date_str),
        ],
    );

    let rows = db.query(&stmt).await?;

    if let Some(row) = rows.first()
        && let Some(starts_on) = row.get::<String>("starts_on")
        && let Some(year_str) = starts_on.get(..4)
        && let Ok(year) = year_str.parse::<i64>()
    {
        return Ok(year);
    }

    // Fallback to calendar year from posting date
    date_str
        .get(..4)
        .and_then(|y| y.parse::<i64>().ok())
        .ok_or_else(|| CommandError::InvalidInput("cannot extract year from date".into()))
}

async fn load_company_country(db: &dyn Database, company_id: &str) -> Result<String, CommandError> {
    let stmt = Statement::with_values(
        "SELECT value FROM company_setting WHERE company_id = ? AND key = 'country_pack'"
            .to_owned(),
        vec![Value::from(company_id)],
    );

    let rows = db.query(&stmt).await?;
    rows.first()
        .and_then(|r| r.get::<String>("value"))
        .ok_or_else(|| CommandError::InvalidInput("company has no country_pack setting".into()))
}

async fn load_customer_default_account(
    db: &dyn Database,
    company_id: &str,
    customer_id: &str,
) -> Result<Option<String>, CommandError> {
    let stmt = Statement::with_values(
        "SELECT default_account FROM vendor WHERE company_id = ? AND id = ?".to_owned(),
        vec![Value::from(company_id), Value::from(customer_id)],
    );

    let rows = db.query(&stmt).await?;
    Ok(rows
        .first()
        .and_then(|r| r.get::<Option<String>>("default_account"))
        .flatten())
}

fn add_days(date_str: &str, days: i64) -> Result<String, CommandError> {
    let date = time::Date::parse(
        date_str,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| CommandError::InvalidInput("invalid date format".into()))?;
    let duration = time::Duration::days(days);
    let new_date = date
        .checked_add(duration)
        .ok_or_else(|| CommandError::InvalidInput("date calculation overflow".into()))?;
    Ok(new_date.to_string())
}

fn make_code_lookup(
    codes: &[ledgers_tax::code::TaxCode],
) -> impl Fn(&str) -> Result<ledgers_tax::code::TaxCode, ledgers_tax::error::TaxError> + '_ {
    move |code_str: &str| {
        codes
            .iter()
            .find(|c| c.code == code_str)
            .cloned()
            .ok_or_else(|| ledgers_tax::error::TaxError::Invalid {
                reason: format!("unknown tax code {code_str:?}"),
            })
    }
}

/// Provision, bump, and guard a numbered counter (invoice or credit note).
/// Returns (`next_number`, `upsert_stmt`, `bump_stmt`, `guard_stmt`).
fn provision_bump_counter(
    company_id: &str,
    table: &str,
    series: &str,
    fiscal_year: i64,
    next_number: i64,
    event_id: &str,
) -> (i64, Statement, Statement, Statement) {
    let guard_key_type = if table == "invoice_number_counter" {
        "invoice_counter"
    } else {
        "credit_note_counter"
    };

    let upsert_sql = format!(
        "INSERT INTO {table} (company_id, series, fiscal_year, next_number) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (company_id, series, fiscal_year) DO NOTHING"
    );
    let upsert = Statement::with_values(
        upsert_sql,
        vec![
            Value::from(company_id),
            Value::from(series),
            Value::from(fiscal_year),
            Value::from(next_number),
        ],
    );

    let bump_sql = format!(
        "UPDATE {table} SET next_number = next_number + 1 \
         WHERE company_id = ? AND series = ? AND fiscal_year = ?"
    );
    let bump = Statement::with_values(
        bump_sql,
        vec![
            Value::from(company_id),
            Value::from(series),
            Value::from(fiscal_year),
        ],
    );

    let guard_sql = format!(
        "(SELECT COALESCE(next_number, 0) FROM {table} \
         WHERE company_id = ? AND series = ? AND fiscal_year = ?) = ?"
    );
    let guard = db_guard::assert_statement(
        db_guard::guard_key(&[guard_key_type, company_id, series, event_id]),
        &guard_sql,
        vec![
            Value::from(company_id),
            Value::from(series),
            Value::from(fiscal_year),
            Value::from(next_number + 1),
        ],
    );

    (next_number, upsert, bump, guard)
}

/// Calculate tax for credit note lines using the ORIGINAL invoice's issue date for rate lookup.
/// Returns the total tax amount.
async fn calculate_tax_for_credit_lines(
    db: &dyn Database,
    ctx: &CommandContext,
    invoice: &store::InvoiceRow,
    credit_lines: &[(store::InvoiceLineRow, i64)],
    pack: &ledgers_tax::country::CountryPack,
) -> Result<i64, CommandError> {
    let chart = load_chart(db, &ctx.company_id).await?;

    // Prepare tax lines for calculation
    let mut tax_inputs = Vec::new();
    for (orig_line, credited_net) in credit_lines {
        tax_inputs.push(TaxLineInput {
            amount: Amount::new(
                i128::from(*credited_net),
                Currency::from_code(&invoice.currency)
                    .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?,
            ),
            tax_code: orig_line.tax_code.clone(),
        });
    }

    // Calculate using invoice's issue date and currency
    let (_, tax_total, _) = calculate_tax_amounts(
        db,
        ctx,
        &tax_inputs,
        &invoice.issue_date,
        &invoice.currency,
        &chart,
        pack,
    )
    .await?;

    Ok(tax_total)
}

/// Build a ledger entry for a credit note.
/// The entry structure is the reverse of an invoice:
/// - Debit revenue accounts for net credited amounts
/// - Debit VAT output accounts for tax on credited amounts
/// - Credit AR control for the gross amount
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn build_entry_for_credit(
    db: &dyn Database,
    ctx: &CommandContext,
    invoice: &store::InvoiceRow,
    credit_lines: &[(store::InvoiceLineRow, i64)],
    chart: &Chart,
    pack: &ledgers_tax::country::CountryPack,
) -> Result<SerializableEntryDraft, CommandError> {
    let posting_date = ctx.now.get(..10).unwrap_or("").to_owned();
    let journal_id = find_journal_for_company(db, &ctx.company_id).await?;

    // Parse invoice issue date for tax rate lookup
    let issue_date = Date::parse(
        &invoice.issue_date,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| CommandError::InvalidInput("invalid invoice issue date format".into()))?;

    let company_country = load_company_country(db, &ctx.company_id).await?;
    let jurisdiction = ledgers_tax::rate::Jurisdiction::new(&company_country);
    let company_rates = Vec::new();
    let rate_table = pack
        .rate_table(&company_rates)
        .map_err(|e| CommandError::InvalidInput(format!("tax rate table error: {e}")))?;

    let tax_codes = pack
        .tax_codes(&chart.accounts_by_tag)
        .map_err(|e| CommandError::InvalidInput(format!("tax code error: {e}")))?;

    // Prepare tax lines for calculation
    let mut tax_inputs = Vec::new();
    let mut net_by_line: BTreeMap<String, i64> = BTreeMap::new();
    for (orig_line, credited_net) in credit_lines {
        tax_inputs.push(TaxLineInput {
            amount: Amount::new(
                i128::from(*credited_net),
                Currency::from_code(&invoice.currency)
                    .map_err(|e| CommandError::InvalidInput(format!("invalid currency: {e}")))?,
            ),
            tax_code: orig_line.tax_code.clone(),
        });
        net_by_line.insert(orig_line.id.clone(), *credited_net);
    }

    let code_lookup = make_code_lookup(&tax_codes);

    // Calculate taxes using the original invoice's rate table
    let calc_params = CalculateParams {
        lines: &tax_inputs,
        date: issue_date,
        table: &rate_table,
        jurisdiction: &jurisdiction,
        code_lookup: &code_lookup,
        basis: Basis::Net,
        scope: pack.rounding,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let tax_result = ledgers_tax::calculate(&calc_params)
        .map_err(|e| CommandError::InvalidInput(format!("tax calculation error: {e}")))?;

    let mut lines = Vec::new();
    let mut tax_by_account: BTreeMap<String, i64> = BTreeMap::new();
    let mut total_net = 0i64;
    let mut total_tax = 0i64;

    // Add debit lines for each credited line's revenue account
    for (orig_line, credited_net) in credit_lines {
        let revenue_account = &orig_line.revenue_account;
        lines.push(SerializableLine {
            account_number: revenue_account.clone(),
            side: "debit".to_owned(),
            base: SerializableAmount {
                minor: credited_net.to_string(),
                currency: invoice.currency.clone(),
            },
            fx: None,
            tax_code: Some(orig_line.tax_code.clone()),
            dimensions: BTreeMap::new(),
            memo: None,
        });
        total_net += credited_net;
    }

    // Calculate and accumulate tax by output account
    for (input_line, tax_line) in &tax_result.lines {
        let tax_code = tax_codes
            .iter()
            .find(|c| c.code == input_line.tax_code)
            .ok_or_else(|| {
                CommandError::InvalidInput(format!("tax code {} not found", input_line.tax_code))
            })?;

        let vat_minor = i64::try_from(tax_line.output_vat.minor())
            .map_err(|_| CommandError::InvalidInput("output vat overflow".into()))?;
        if vat_minor > 0 {
            let account = tax_code.output_account.as_ref().ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "tax code {} has no output account",
                    tax_code.code
                ))
            })?;
            tax_by_account
                .entry(account.clone())
                .and_modify(|v| *v += vat_minor)
                .or_insert(vat_minor);
            total_tax += vat_minor;
        }
    }

    // Add debit lines for VAT output accounts
    for (account, vat_amount) in &tax_by_account {
        lines.push(SerializableLine {
            account_number: account.clone(),
            side: "debit".to_owned(),
            base: SerializableAmount {
                minor: vat_amount.to_string(),
                currency: invoice.currency.clone(),
            },
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        });
    }

    // Add credit line for AR control (gross = net + tax)
    let ar_control = chart.receivable.as_ref().ok_or_else(|| {
        CommandError::InvalidInput("the chart has no receivable account (tag ar)".into())
    })?;

    lines.push(SerializableLine {
        account_number: ar_control.clone(),
        side: "credit".to_owned(),
        base: SerializableAmount {
            minor: (total_net + total_tax).to_string(),
            currency: invoice.currency.clone(),
        },
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    });

    Ok(SerializableEntryDraft {
        journal_id,
        document_date: invoice.issue_date.clone(),
        posting_date,
        tax_point_date: None,
        base_currency: invoice.currency.clone(),
        description: format!("Credit Note {}", ctx.event_id),
        source_documents: vec![],
        external_id: Some(format!("credit-note-{}", ctx.event_id)),
        source_app: Some("ledgers-invoice".to_owned()),
        reverses: None,
        lines,
    })
}
