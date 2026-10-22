//! Entry reversals: mirror the original entry with sides flipped, set reverses,
//! date it based on period rules, and post via `post_entry` so all guards apply.

use crate::entry::{Entry, EntryDraft, Fx, JournalError, Line, Side};
use crate::posting::{PostContext, PostedEntry, post_entry};
use cratefield_core::Database;
use ledgers_db::period as db_period;
use ledgers_db::retry::RetryPolicy;
use ledgers_money::{Amount, Currency};
use std::time::Duration;
use time::Date;

/// The fields from a `journal_entry` header row.
struct EntryHeader {
    journal_id: String,
    document_date: Date,
    posting_date: Date,
    tax_point_date: Date,
    base_currency: Currency,
    number: Option<i64>,
    reverses: Option<String>,
}

/// Context for reversing an entry.
pub struct ReverseContext {
    /// Company ID for the reversal.
    pub company_id: String,
    /// Entry ID to reverse.
    pub entry_id: String,
    /// Requested posting date; may be adjusted if period is not open.
    pub requested_posting_date: Date,
    /// Whether the requester is a human (affects soft-close period rules).
    pub human: bool,
}

/// Reverses an entry: reads original, mirrors lines (sides flipped, amounts unchanged),
/// posts via `post_entry`. Posting date: requested if period is open (or soft-closed and
/// human); else first day of first open period after it. Returns error if no open
/// period exists.
///
/// # Errors
/// Returns `JournalError` on database errors, if entry not found, or if no open period exists.
pub async fn reverse_entry(
    db: &dyn Database,
    reverse_ctx: &ReverseContext,
    post_ctx: &PostContext,
    policy: RetryPolicy,
    sleep_fn: impl FnMut(Duration),
    audit: &ledgers_audit::AuditContext,
) -> Result<PostedEntry, JournalError> {
    let (original, lines) =
        read_entry_with_lines(db, &reverse_ctx.company_id, &reverse_ctx.entry_id).await?;

    let mirror_lines: Vec<Line> = lines
        .into_iter()
        .map(|line| Line {
            side: line.side.flip(),
            ..line
        })
        .collect();

    let posting_date = determine_posting_date(
        db,
        &reverse_ctx.company_id,
        reverse_ctx.requested_posting_date,
        reverse_ctx.human,
    )
    .await?;

    let draft = EntryDraft {
        journal_id: original.journal_id.clone(),
        document_date: original.posting_date,
        posting_date,
        tax_point_date: Some(posting_date),
        base_currency: original.base_currency,
        description: format!("Reversal of entry #{}", original.number.unwrap_or(0)),
        source_documents: vec![format!("reversal:{}", reverse_ctx.entry_id)],
        external_id: None,
        source_app: None,
        reverses: Some(reverse_ctx.entry_id.clone()),
        lines: mirror_lines,
    };

    let entry = Entry::from_draft(draft, &reverse_ctx.company_id)?;
    post_entry(db, &entry, post_ctx, policy, sleep_fn, audit).await
}

async fn determine_posting_date(
    db: &dyn Database,
    company_id: &str,
    requested: Date,
    human: bool,
) -> Result<Date, JournalError> {
    let requested_str = format!(
        "{:04}-{:02}-{:02}",
        requested.year(),
        u8::from(requested.month()),
        requested.day()
    );
    let requested_period =
        db_period::period_of_date(&requested_str).ok_or(JournalError::PeriodNotOpen {
            period: requested_str.clone(),
        })?;

    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT status FROM accounting_period WHERE company_id = ? AND period = ?".to_owned(),
            vec![
                sea_query::Value::from(company_id),
                sea_query::Value::from(requested_period.as_str()),
            ],
        ))
        .await?;

    if rows
        .first()
        .and_then(|row| row.get::<String>("status"))
        .is_some_and(|status_str| {
            status_str == db_period::STATUS_OPEN
                || (status_str == db_period::STATUS_SOFT_CLOSED && human)
        })
    {
        return Ok(requested);
    }

    // Scan forward for the first open period.
    let mut scan_date = requested.next_day().ok_or(JournalError::PeriodNotOpen {
        period: requested_str.clone(),
    })?;
    for _ in 0..400 {
        let scan_str = format!(
            "{:04}-{:02}-{:02}",
            scan_date.year(),
            u8::from(scan_date.month()),
            scan_date.day()
        );
        if let Some(scan_period) = db_period::period_of_date(&scan_str) {
            let rows = db
                .query(&cratefield_core::Statement::with_values(
                    "SELECT status FROM accounting_period WHERE company_id = ? AND period = ?"
                        .to_owned(),
                    vec![
                        sea_query::Value::from(company_id),
                        sea_query::Value::from(scan_period.as_str()),
                    ],
                ))
                .await?;

            if rows
                .first()
                .and_then(|row| row.get::<String>("status"))
                .is_some_and(|status_str| status_str == db_period::STATUS_OPEN)
            {
                let (year, month) = scan_period
                    .split_once('-')
                    .and_then(|(y, m)| Some((y.parse::<i32>().ok()?, m.parse::<u32>().ok()?)))
                    .ok_or(JournalError::PeriodNotOpen {
                        period: scan_str.clone(),
                    })?;
                let month_enum = time::Month::try_from(u8::try_from(month).map_err(|_| {
                    JournalError::PeriodNotOpen {
                        period: scan_str.clone(),
                    }
                })?)
                .map_err(|_| JournalError::PeriodNotOpen {
                    period: scan_str.clone(),
                })?;
                return Date::from_calendar_date(year, month_enum, 1)
                    .map_err(|_| JournalError::PeriodNotOpen { period: scan_str });
            }
        }
        scan_date = scan_date.next_day().ok_or(JournalError::PeriodNotOpen {
            period: scan_str.clone(),
        })?;
    }

    Err(JournalError::PeriodNotOpen {
        period: requested_str,
    })
}

async fn read_entry_with_lines(
    db: &dyn Database,
    company_id: &str,
    entry_id: &str,
) -> Result<(Entry, Vec<Line>), JournalError> {
    let rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT journal_id, document_date, posting_date, tax_point_date, base_currency, number, reverses \
             FROM journal_entry WHERE id = ? AND company_id = ?"
                .to_owned(),
            vec![sea_query::Value::from(entry_id), sea_query::Value::from(company_id)],
        ))
        .await?;

    let row = rows.first().ok_or(JournalError::ForeignReversal {
        entry: entry_id.to_owned(),
    })?;

    let header = parse_entry_header_row(row, entry_id)?;

    let line_rows = db
        .query(&cratefield_core::Statement::with_values(
            "SELECT account_number, base_minor, tx_currency, tx_minor, rate_scaled, tax_code, dimensions, memo \
             FROM journal_line WHERE entry_id = ? ORDER BY line_no"
                .to_owned(),
            vec![sea_query::Value::from(entry_id)],
        ))
        .await?;

    let mut lines = Vec::new();
    for line_row in &line_rows.rows {
        lines.push(parse_line_row(
            line_row,
            header.base_currency,
            header.base_currency.code(),
            entry_id,
        )?);
    }

    let entry = Entry {
        company_id: company_id.to_owned(),
        journal_id: header.journal_id,
        document_date: header.document_date,
        posting_date: header.posting_date,
        tax_point_date: header.tax_point_date,
        base_currency: header.base_currency,
        vat_period: crate::entry::vat_period_of(header.tax_point_date),
        description: String::new(),
        source_documents: vec![],
        external_id: None,
        source_app: None,
        reverses: header.reverses,
        lines: lines.clone(),
        number: header.number,
        recorded_at: None,
    };
    Ok((entry, lines))
}

fn parse_entry_header_row(
    row: &cratefield_core::Row,
    entry_id: &str,
) -> Result<EntryHeader, JournalError> {
    let journal_id = row
        .get::<String>("journal_id")
        .ok_or(JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;
    let document_date_str =
        row.get::<String>("document_date")
            .ok_or(JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
    let posting_date_str =
        row.get::<String>("posting_date")
            .ok_or(JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
    let tax_point_date_str =
        row.get::<String>("tax_point_date")
            .ok_or(JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
    let base_currency_code =
        row.get::<String>("base_currency")
            .ok_or(JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;

    let document_date = time::Date::parse(
        &document_date_str,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| JournalError::ForeignReversal {
        entry: entry_id.to_owned(),
    })?;
    let posting_date = time::Date::parse(
        &posting_date_str,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| JournalError::ForeignReversal {
        entry: entry_id.to_owned(),
    })?;
    let tax_point_date = time::Date::parse(
        &tax_point_date_str,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .map_err(|_| JournalError::ForeignReversal {
        entry: entry_id.to_owned(),
    })?;
    let base_currency =
        Currency::from_code(&base_currency_code).map_err(|_| JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;

    let number = row.get::<i64>("number");
    let reverses = row.get::<Option<String>>("reverses").flatten();

    Ok(EntryHeader {
        journal_id,
        document_date,
        posting_date,
        tax_point_date,
        base_currency,
        number,
        reverses,
    })
}

fn parse_line_row(
    row: &cratefield_core::Row,
    base_currency: Currency,
    base_currency_code: &str,
    entry_id: &str,
) -> Result<Line, JournalError> {
    let account = row
        .get::<String>("account_number")
        .ok_or(JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;
    let base_minor = row
        .get::<i64>("base_minor")
        .ok_or(JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;
    let tx_currency_code =
        row.get::<String>("tx_currency")
            .ok_or(JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
    let tx_minor = row
        .get::<i64>("tx_minor")
        .ok_or(JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;
    let rate_scaled = row.get::<Option<String>>("rate_scaled").flatten();
    let tax_code = row.get::<Option<String>>("tax_code").flatten();
    let dimensions_json = row.get::<String>("dimensions").unwrap_or_default();
    let dimensions: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&dimensions_json).unwrap_or_default();
    let memo = row.get::<Option<String>>("memo").flatten();

    let side = if base_minor >= 0 {
        Side::Debit
    } else {
        Side::Credit
    };
    let base = Amount::new(i128::from(base_minor.abs()), base_currency);
    let fx = if rate_scaled.is_some() && tx_currency_code != base_currency_code {
        let tx_currency =
            Currency::from_code(&tx_currency_code).map_err(|_| JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
        let rate_str = rate_scaled.ok_or(JournalError::ForeignReversal {
            entry: entry_id.to_owned(),
        })?;
        let rate =
            ledgers_money::Rate::parse(&rate_str).map_err(|_| JournalError::ForeignReversal {
                entry: entry_id.to_owned(),
            })?;
        Some(Fx {
            transaction: Amount::new(i128::from(tx_minor.abs()), tx_currency),
            rate,
        })
    } else {
        None
    };

    Ok(Line {
        account_number: account,
        side,
        base,
        fx,
        tax_code,
        dimensions,
        memo,
    })
}
