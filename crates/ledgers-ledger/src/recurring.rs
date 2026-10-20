//! Recurring templates: cron-like scheduling of journal entries via drafts.
//! A template carries a name, schedule ('monthly'|'quarterly'|'yearly'),
//! `next_due` date, and an `EntryDraft` blueprint. Actual `run_due_recurring` logic
//! is scaffolding; the schema and tables support the full implementation.

use crate::draft::StoredEntryDraft;
use crate::entry::EntryDraft;
use cratefield_core::{DbError, Statement};
use sea_query::Value as SeaValue;
use time::Date;

/// The schedule of a recurring template.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Schedule {
    Monthly,
    Quarterly,
    Yearly,
}

impl Schedule {
    /// Parses the schedule from a string: "monthly", "quarterly", or "yearly".
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "monthly" => Some(Schedule::Monthly),
            "quarterly" => Some(Schedule::Quarterly),
            "yearly" => Some(Schedule::Yearly),
            _ => None,
        }
    }

    /// The schedule as a string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Schedule::Monthly => "monthly",
            Schedule::Quarterly => "quarterly",
            Schedule::Yearly => "yearly",
        }
    }
}

impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Context for creating a recurring template.
pub struct CreateRecurringContext {
    /// Template ID.
    pub template_id: String,
    /// Company ID.
    pub company_id: String,
    /// Journal ID.
    pub journal_id: String,
    /// Template name.
    pub name: String,
    /// Schedule frequency.
    pub schedule: Schedule,
    /// Next due date.
    pub next_due: Date,
    /// Timestamp for creation.
    pub recorded_at: String,
}

/// Creates a recurring template: stores the name, schedule, `next_due` date,
/// and the `EntryDraft` blueprint.
///
/// # Errors
/// [`cratefield_core::DbError`] on database errors.
pub async fn create_recurring(
    db: &dyn cratefield_core::Database,
    ctx: &CreateRecurringContext,
    blueprint: &EntryDraft,
) -> Result<(), DbError> {
    let stored = StoredEntryDraft::from_draft(blueprint);
    let body = serde_json::to_string(&stored).map_err(|e| DbError::Query(e.to_string()))?;
    let next_due_str = format!(
        "{:04}-{:02}-{:02}",
        ctx.next_due.year(),
        u8::from(ctx.next_due.month()),
        ctx.next_due.day()
    );

    db.execute(&Statement::with_values(
        "INSERT INTO recurring_template (id, company_id, journal_id, name, schedule, next_due, body, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(ctx.template_id.as_str()),
            SeaValue::from(ctx.company_id.as_str()),
            SeaValue::from(ctx.journal_id.as_str()),
            SeaValue::from(ctx.name.as_str()),
            SeaValue::from(ctx.schedule.as_str()),
            SeaValue::from(next_due_str.as_str()),
            SeaValue::from(body.as_str()),
            SeaValue::from(ctx.recorded_at.as_str()),
        ],
    ))
    .await
    .map(|_| ())
}

/// Lists recurring templates for a company.
///
/// # Errors
/// [`cratefield_core::DbError`] on database errors.
pub async fn list_recurring(
    db: &dyn cratefield_core::Database,
    company_id: &str,
) -> Result<Vec<(String, String, Schedule, String)>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, name, schedule, next_due FROM recurring_template WHERE company_id = ? ORDER BY created_at"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;

    let templates = rows
        .rows
        .iter()
        .filter_map(|row| {
            let id = row.get::<String>("id")?;
            let name = row.get::<String>("name")?;
            let schedule_str = row.get::<String>("schedule")?;
            let next_due_str = row.get::<String>("next_due")?;
            let schedule = Schedule::parse(&schedule_str)?;
            Some((id, name, schedule, next_due_str))
        })
        .collect();

    Ok(templates)
}

/// Pauses (or unpauses) a recurring template by id.
///
/// # Errors
/// [`cratefield_core::DbError`] on database errors.
pub async fn pause_recurring(
    db: &dyn cratefield_core::Database,
    template_id: &str,
    paused: bool,
) -> Result<(), DbError> {
    db.execute(&Statement::with_values(
        "UPDATE recurring_template SET paused = ? WHERE id = ?".to_owned(),
        vec![
            SeaValue::from(u8::from(paused)),
            SeaValue::from(template_id),
        ],
    ))
    .await
    .map(|_| ())
}

/// Runs all due recurring templates as of a date: for each unpaused template
/// with `next_due <= as_of`, creates one draft per due date (with `posting_date =
/// due_date`), advances `next_due` by schedule, and guards against concurrent
/// double-creation. Returns created draft ids.
///
/// # Errors
/// [`cratefield_core::DbError`] on database errors.
pub async fn run_due_recurring(
    db: &dyn cratefield_core::Database,
    company_id: &str,
    as_of: Date,
    recorded_at: &str,
) -> Result<Vec<String>, DbError> {
    let as_of_str = format!(
        "{:04}-{:02}-{:02}",
        as_of.year(),
        u8::from(as_of.month()),
        as_of.day()
    );

    let rows = db
        .query(&Statement::with_values(
            "SELECT id, journal_id, next_due, schedule, body FROM recurring_template \
             WHERE company_id = ? AND paused = 0 AND next_due <= ? ORDER BY next_due"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(as_of_str.as_str()),
            ],
        ))
        .await?;

    let mut created = Vec::new();
    for row in &rows.rows {
        let Some(template_id) = row.get::<String>("id") else {
            continue;
        };
        let Some(journal_id) = row.get::<String>("journal_id") else {
            continue;
        };
        let Some(next_due_str) = row.get::<String>("next_due") else {
            continue;
        };
        let Some(schedule_str) = row.get::<String>("schedule") else {
            continue;
        };
        let Some(body_json) = row.get::<String>("body") else {
            continue;
        };

        let Some(schedule) = Schedule::parse(&schedule_str) else {
            continue;
        };
        let Ok(stored) = serde_json::from_str::<StoredEntryDraft>(&body_json) else {
            continue;
        };
        let Ok(next_due_date) = time::Date::parse(
            &next_due_str,
            &time::format_description::well_known::Iso8601::DEFAULT,
        ) else {
            continue;
        };

        let new_next_due = advance_date(next_due_date, schedule);
        let new_next_due_str = format!(
            "{:04}-{:02}-{:02}",
            new_next_due.year(),
            u8::from(new_next_due.month()),
            new_next_due.day()
        );

        let draft_id = format!("draft_{template_id}_{next_due_str}");
        let guard_key =
            ledgers_db::guard::guard_key(&["recurring-due", &template_id, &next_due_str]);

        // Build draft with posting_date = due_date.
        let mut draft = stored.clone();
        draft.posting_date = next_due_str.clone();
        draft.tax_point_date = Some(next_due_str.clone());

        let draft_body = serde_json::to_string(&draft).unwrap_or_default();

        let statements = vec![
            ledgers_db::guard::assert_statement(
                &guard_key,
                "(SELECT COUNT(*) FROM entry_draft WHERE recurring_template_id = ? AND due_date = ?) = 0",
                vec![SeaValue::from(template_id.as_str()), SeaValue::from(next_due_str.as_str())],
            ),
            Statement::with_values(
                "INSERT INTO entry_draft (id, company_id, journal_id, body, recurring_template_id, due_date, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_owned(),
                vec![
                    SeaValue::from(draft_id.as_str()),
                    SeaValue::from(company_id),
                    SeaValue::from(journal_id.as_str()),
                    SeaValue::from(draft_body.as_str()),
                    SeaValue::from(template_id.as_str()),
                    SeaValue::from(next_due_str.as_str()),
                    SeaValue::from(recorded_at),
                    SeaValue::from(recorded_at),
                ],
            ),
            Statement::with_values(
                "UPDATE recurring_template SET next_due = ? WHERE id = ?"
                    .to_owned(),
                vec![
                    SeaValue::from(new_next_due_str.as_str()),
                    SeaValue::from(template_id.as_str()),
                ],
            ),
            ledgers_db::guard::release_statement(&guard_key),
        ];

        if db.batch_atomic(&statements).await.is_ok() {
            created.push(draft_id);
        }
    }

    Ok(created)
}

/// Advances a date by schedule, clamping month-end (Jan 31 → Feb 28/29).
fn advance_date(date: Date, schedule: Schedule) -> Date {
    use time::Month;

    match schedule {
        Schedule::Monthly => {
            let (next_year, next_month) = match date.month() {
                Month::December => (date.year() + 1, Month::January),
                m => (date.year(), m.next()),
            };
            // Try the same day in the next month, or fall back to the last day of that month.
            let day = date.day();
            let last_day_of_month = days_in_month(next_year, next_month);
            let target_day = day.min(last_day_of_month);
            // Safe: target_day is guaranteed to be valid for this month.
            Date::from_calendar_date(next_year, next_month, target_day)
                .expect("target_day is guaranteed to be valid for the month")
        }
        Schedule::Quarterly => {
            let next_month_num = u8::from(date.month()) + 3;
            let (next_year, next_month_num) = if next_month_num > 12 {
                (date.year() + 1, next_month_num - 12)
            } else {
                (date.year(), next_month_num)
            };
            let next_month =
                Month::try_from(next_month_num).expect("next_month_num is guaranteed to be 1-12");
            let day = date.day();
            let last_day_of_month = days_in_month(next_year, next_month);
            let target_day = day.min(last_day_of_month);
            Date::from_calendar_date(next_year, next_month, target_day)
                .expect("target_day is guaranteed to be valid for the month")
        }
        Schedule::Yearly => {
            let next_year = date.year() + 1;
            // Leap year handling: if trying Feb 29 in a non-leap year, use Feb 28.
            let day = date.day();
            let month = date.month();
            let last_day_of_month = days_in_month(next_year, month);
            let target_day = day.min(last_day_of_month);
            Date::from_calendar_date(next_year, month, target_day)
                .expect("target_day is guaranteed to be valid for the month")
        }
    }
}

/// Returns the number of days in a given month.
fn days_in_month(year: i32, month: time::Month) -> u8 {
    use time::Month;
    match month {
        Month::January
        | Month::March
        | Month::May
        | Month::July
        | Month::August
        | Month::October
        | Month::December => 31,
        Month::April | Month::June | Month::September | Month::November => 30,
        Month::February => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
