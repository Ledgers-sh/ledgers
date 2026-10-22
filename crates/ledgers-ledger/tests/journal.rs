//! The journal scenario matrix: the posting lifecycle over every
//! backing engine the environment provides — in-memory SQLite always,
//! a throwaway Postgres 16 database when `FZ_TEST_POSTGRES_URL` names
//! one (CI's service container). One test definition, every dialect:
//! the parity loop the testing kit documents, the same shape as the
//! accounts matrix one file over.
//!
//! The harness factory mounts **both** modules: `LedgerModule` for the
//! chart and journal tables, `DbModule` for the guard foundation
//! (numbering counter, probe table, periods, audit chain, outbox) the
//! posting batch reads and writes.
//!
//! What the scenarios prove, mapped to the acceptance criteria:
//!
//! - **journals** — create, duplicate-code refusal, lookup by code and
//!   id;
//! - **numbering** — two posts into one journal take the numbers 1 and
//!   2, gapless (the concurrency file proves it at scale);
//! - **the balance invariant at write time** — an unbalanced entry is
//!   refused by the SQL guard alone (via the raw, validation-free
//!   path), consumes no number, and leaves nothing behind; a refusal
//!   the guard fired is mapped back to its domain variant by
//!   diagnostic reads after the failed write (ADR 0004: reads after a
//!   failed write choose the error, they never gate one), so closed
//!   and unknown periods surface `PeriodNotOpen` and unknown or
//!   archived accounts `AccountArchived`;
//! - **cross-company posts** — a post into another company's journal,
//!   or a reversal naming another company's entry, is refused
//!   fail-closed by the ownership guards, consuming no number;
//! - **the FX acceptance case** — an EUR company paying a USD bill
//!   posts with its FX-loss line, and the same entry without the line
//!   is refused with the base difference;
//! - **immutability** — UPDATE and DELETE on `journal_line` and on a
//!   posted `journal_entry` fail at the storage layer (the triggers),
//!   on every dialect;
//! - **reversals** — the `reverses` relation reads back through a
//!   query, because no mutable `reversed_by` column exists;
//! - **the outbox** — one `journal.entry.posted` event per committed
//!   post, payload intact.
//!
//! Below the matrix sits the source-scan gate: no source file under
//! `crates/*/src` may contain SQL updating or deleting the journal
//! tables — the no-update/no-delete contract, checked mechanically so
//! a regression cannot land as prose.

mod common;

use common::{COMPANY_ID, FY, NOW, POSTED_AT, count, counter_value, create_chart, open_period};
use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, RetryPolicy};
use ledgers_ledger::close::CloseCheckRegistry;
use ledgers_ledger::commands;
use ledgers_ledger::entry::{Side, reversed_by};
use ledgers_ledger::fiscal::FiscalYearSpec;
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::period;
use ledgers_ledger::posting::{
    PostContext, RawEntry, RawLine, TOPIC_ENTRY_POSTED, post_entry, post_raw_entry,
};
use ledgers_ledger::{Entry, EntryDraft, EntryJournal, Fx, JournalError, LedgerModule, Line};
use ledgers_money::{Amount, Currency, Rate};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use time::{Date, Month};

#[test]
fn journal_scenarios_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        // The scenarios panic with their own diagnosis on failure. Fresh
        // module instances per dialect: all_dialects runs the factory
        // once per leg.
        pollster::block_on(scenarios(&*kit.db));
    }
}

/// The journal and posting lifecycle, end to end over one database.
///
/// # Panics
///
/// Panics when any step returns the wrong result — exactly what a
/// conformance run should surface.
pub async fn scenarios(db: &dyn Database) {
    // Create the test companies with EUR base currency if they don't exist
    let _ = db
        .execute(&Statement::new(
            format!(
                "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
                 VALUES ('{COMPANY_ID}', 'Test Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            ),
        ))
        .await;
    let _ = db
        .execute(&Statement::new(
            format!(
                "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
                 VALUES ('{}', 'Other Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                common::OTHER_COMPANY_ID
            ),
        ))
        .await;

    journal_lifecycle(db).await;
    base_currency_is_fixed_once_entries_exist(db).await;
    balanced_posts_are_numbered_gaplessly(db).await;
    the_fx_bill_payment_posts_and_its_missing_leg_is_refused(db).await;
    closed_and_unknown_periods_refuse(db).await;
    unknown_and_archived_accounts_refuse(db).await;
    a_foreign_journal_refuses_without_touching_their_numbering(db).await;
    a_cross_company_reversal_refuses(db).await;
    the_db_guard_alone_rejects_unbalanced_lines_without_consuming_a_number(db).await;
    posted_rows_are_immutable_at_the_storage_layer(db).await;
    reversals_read_back_through_the_reverses_relation(db).await;
}

/// Creates the scenario chart and one general journal, opens
/// `period`, and returns the journal's id.
async fn fixtures(db: &dyn Database, journal_code: &str, period: &str) -> String {
    create_chart(db, COMPANY_ID).await;
    let journal = journal::create_journal(
        db,
        COMPANY_ID,
        JournalDraft {
            code: journal_code.to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        &format!("01J{journal_code}000000000000000000000"),
        NOW,
        &common::person(),
    )
    .await
    .unwrap_or_else(|err| panic!("create journal {journal_code}: {err}"));
    open_period(db, COMPANY_ID, period).await;
    journal.id
}

/// The scenario date: everything posts on the same April day.
fn april(tenth: u8) -> Date {
    Date::from_calendar_date(2026, Month::April, tenth).expect("a calendar date")
}

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the table")
}

fn usd() -> Currency {
    Currency::from_code("USD").expect("USD is in the table")
}

/// An EUR amount from major units, for readable fixtures.
fn eur_major(major: i128) -> Amount {
    Amount::from_major(major, eur()).expect("a representable amount")
}

/// A draft posting on the fixture dates.
fn draft(journal_id: &str, lines: Vec<Line>) -> EntryDraft {
    EntryDraft {
        journal_id: journal_id.to_owned(),
        document_date: april(10),
        posting_date: april(10),
        tax_point_date: None,
        base_currency: eur(),
        description: "Scenario entry".to_owned(),
        source_documents: Vec::new(),
        external_id: None,
        source_app: None,
        reverses: None,
        lines,
    }
}

/// A plain EUR line.
fn eur_line(account: &str, side: Side, base: Amount) -> Line {
    Line {
        account_number: account.to_owned(),
        side,
        base,
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    }
}

/// A fresh post context: entry and outbox ids unique per scenario
/// (`code`, the journal's) and per post (`n`) — ids are primary keys,
/// so a reuse would collide as a spurious conflict.
fn ctx(code: &str, n: u64) -> PostContext {
    PostContext {
        fiscal_year: FY,
        entry_id: format!("01ENTRYSCEN{code}{n:03}0000000000000000000"),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: format!("01OUTBOXSCEN{code}{n:03}0000000000000000000"),
    }
}

/// The null sleeper: the scenarios are sequential, and the library
/// itself never touches `std::thread`.
fn no_sleep(_: std::time::Duration) {}

async fn journal_lifecycle(db: &dyn Database) {
    let id = fixtures(db, "GJ", "2026-04").await;

    // Duplicate codes are refused, friendly, before the key fires.
    let duplicate = journal::create_journal(
        db,
        COMPANY_ID,
        JournalDraft {
            code: "GJ".to_owned(),
            kind: JournalKind::Sales,
            name: "Another journal".to_owned(),
        },
        "01JANOTHERJOURNAL0000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        duplicate,
        JournalError::JournalExists {
            code: "GJ".to_owned()
        }
    );

    let by_code = journal::find_journal(db, COMPANY_ID, "GJ")
        .await
        .expect("find by code");
    assert_eq!(by_code.expect("the journal exists").id, id);
    assert!(
        journal::find_journal(db, COMPANY_ID, "NOPE")
            .await
            .expect("the query runs")
            .is_none()
    );

    let by_id = journal::get_journal(db, &id)
        .await
        .expect("get by id")
        .expect("the journal exists");
    assert_eq!(by_id.kind, JournalKind::General);
    assert_eq!(by_id.name, "General journal");

    // Codes are validated.
    let bad_code = journal::create_journal(
        db,
        COMPANY_ID,
        JournalDraft {
            code: "g j".to_owned(),
            kind: JournalKind::Cash,
            name: "x".to_owned(),
        },
        "01JBADCODEJOURNAL000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .unwrap_err();
    assert!(matches!(bad_code, JournalError::InvalidJournalCode { .. }));
}

async fn balanced_posts_are_numbered_gaplessly(db: &dyn Database) {
    let journal_id = fixtures(db, "NJ", "2026-04").await;
    for n in 1..=2u64 {
        let entry = Entry::from_draft(
            draft(
                &journal_id,
                vec![
                    eur_line("1010", Side::Debit, eur_major(15)),
                    eur_line("4000", Side::Credit, eur_major(15)),
                ],
            ),
            COMPANY_ID,
        )
        .expect("the draft validates");
        let posted = post_entry(
            db,
            &entry,
            &ctx("NJ", n),
            RetryPolicy::default(),
            no_sleep,
            &common::person(),
        )
        .await
        .unwrap_or_else(|err| panic!("post {n}: {err}"));
        assert_eq!(posted.number, i64::try_from(n).expect("small"));
        assert_eq!(posted.recorded_at, POSTED_AT);
        assert_eq!(posted.fiscal_year, FY);
    }

    // The stored row carries the derived VAT period, the status, and
    // the recorded-at stamp.
    let rows = db
        .query(&Statement::with_values(
            "SELECT number, status, vat_period, recorded_at, base_currency FROM journal_entry\n\
             WHERE journal_id = ? ORDER BY number"
                .to_owned(),
            vec![SeaValue::from(journal_id.as_str())],
        ))
        .await
        .expect("read the entries back");
    assert_eq!(rows.len(), 2);
    for (index, row) in rows.rows.iter().enumerate() {
        assert_eq!(
            row.get::<i64>("number").expect("number"),
            i64::try_from(index + 1).expect("small")
        );
        assert_eq!(row.get::<String>("status").expect("status"), "posted");
        assert_eq!(row.get::<String>("vat_period").expect("vat"), "2026-04");
        assert_eq!(
            row.get::<String>("recorded_at").expect("recorded_at"),
            POSTED_AT
        );
        assert_eq!(row.get::<String>("base_currency").expect("currency"), "EUR");
    }

    // The lines carry the signed minor units, and no FX columns on a
    // pure base-currency line.
    let lines = db
        .query(&Statement::with_values(
            "SELECT line_no, account_number, base_minor, tx_currency, tx_minor, rate_scaled\n\
             FROM journal_line WHERE entry_id = (\n\
             SELECT id FROM journal_entry WHERE journal_id = ? AND number = ?) ORDER BY line_no"
                .to_owned(),
            vec![
                SeaValue::from(journal_id.as_str()),
                SeaValue::BigInt(Some(1)),
            ],
        ))
        .await
        .expect("read the lines back");
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines.rows[0].get::<i64>("base_minor").expect("signed"),
        1500,
        "debit is positive"
    );
    assert_eq!(
        lines.rows[1].get::<i64>("base_minor").expect("signed"),
        -1500,
        "credit is negative"
    );
    assert_eq!(
        lines.rows[0].get::<String>("tx_currency").expect("tx"),
        "EUR"
    );
    assert_eq!(lines.rows[0].get::<i64>("tx_minor").expect("tx"), 1500);
    assert!(
        lines.rows[0].get::<String>("rate_scaled").is_none(),
        "a pure base line carries no rate"
    );
}

/// The acceptance case: an EUR company pays a USD 1 000.00 bill that
/// was booked at 0.9000 (EUR 900.00 of payables); the bank charge at
/// 0.9200 costs EUR 920.00, so the entry carries an EUR 20.00 FX loss.
/// With the loss line it posts; without it the domain refuses with the
/// base difference.
async fn the_fx_bill_payment_posts_and_its_missing_leg_is_refused(db: &dyn Database) {
    let journal_id = fixtures(db, "FJ", "2026-04").await;

    let mut payables = eur_line("1400", Side::Debit, eur_major(900));
    payables.fx = Some(Fx {
        transaction: Amount::from_major(1000, usd()).expect("a representable amount"),
        rate: Rate::parse("0.9000").expect("a rate"),
    });
    let fx_loss = eur_line("3000", Side::Debit, eur_major(20));
    let mut bank = eur_line("1010", Side::Credit, eur_major(920));
    bank.fx = Some(Fx {
        transaction: Amount::from_major(1000, usd()).expect("a representable amount"),
        rate: Rate::parse("0.9200").expect("a rate"),
    });

    // Without the loss line: EUR 900 + 0 − 920 = −20.00. The USD legs
    // balance (+1 000 − 1 000), so the currency list is empty: only
    // the base leg is wrong.
    let refusal = Entry::from_draft(
        draft(&journal_id, vec![payables.clone(), bank.clone()]),
        COMPANY_ID,
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        JournalError::Unbalanced {
            base_difference: eur_major(-20),
            currency_differences: Vec::new(),
        }
    );

    // With it, the entry posts and every leg round-trips.
    let entry = Entry::from_draft(
        draft(&journal_id, vec![payables, fx_loss, bank]),
        COMPANY_ID,
    )
    .expect("the FX bill payment validates");
    let posted = post_entry(
        db,
        &entry,
        &ctx("FJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("the FX bill payment posts");
    assert_eq!(posted.number, 1);

    let rows = db
        .query(&Statement::with_values(
            "SELECT account_number, base_minor, tx_currency, tx_minor, rate_scaled FROM journal_line\n\
             WHERE entry_id = (\n\
             SELECT id FROM journal_entry WHERE journal_id = ? AND number = ?) ORDER BY line_no"
                .to_owned(),
            vec![
                SeaValue::from(journal_id.as_str()),
                SeaValue::BigInt(Some(1)),
            ],
        ))
        .await
        .expect("read the FX lines back");
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.rows[0].get::<String>("tx_currency").expect("tx"),
        "USD"
    );
    assert_eq!(
        rows.rows[0].get::<i64>("tx_minor").expect("tx minor"),
        100_000,
        "USD 1 000.00 in minor units"
    );
    assert_eq!(
        rows.rows[0].get::<String>("rate_scaled").expect("rate"),
        "900000000000",
        "0.9000 scaled by 10^12"
    );
    assert_eq!(
        rows.rows[2].get::<String>("rate_scaled").expect("rate"),
        "920000000000",
        "0.9200 scaled by 10^12"
    );
    assert_eq!(
        rows.rows[1].get::<i64>("base_minor").expect("the loss"),
        2000,
        "Dr FX loss 20.00 EUR"
    );
}

async fn closed_and_unknown_periods_refuse(db: &dyn Database) {
    // Closed: the row exists, the status is not open. The guard fires;
    // the diagnostic read after the failed write maps it to the period
    // variant.
    let journal_id = fixtures(db, "CJ", "2026-03").await;
    db.execute(&ledgers_db::period::set_status_statement(
        COMPANY_ID, "2026-03", "closed",
    ))
    .await
    .expect("close the period");
    let entry =
        Entry::from_draft(draft_march(&journal_id), COMPANY_ID).expect("the draft validates");
    let err = post_entry(
        db,
        &entry,
        &ctx("CJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("a closed period refuses the post");
    assert_eq!(
        err,
        JournalError::PeriodNotOpen {
            period: "2026-03".to_owned(),
        },
        "the refusal maps to its variant, not a bare 500"
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );

    // Unknown: no row at all (2026-05 was never opened) — the guard
    // fails closed exactly as for a closed one.
    let journal_id = fixtures(db, "UJ", "2026-04").await;
    let entry = Entry::from_draft(draft_may(&journal_id), COMPANY_ID).expect("the draft validates");
    let err = post_entry(
        db,
        &entry,
        &ctx("UJ", 2),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("an unknown period refuses the post");
    assert_eq!(
        err,
        JournalError::PeriodNotOpen {
            period: "2026-05".to_owned(),
        }
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );
    assert_eq!(
        counter_value(db, &journal_id, FY).await.unwrap_or(0),
        0,
        "the refused batch consumed no number"
    );
}

async fn unknown_and_archived_accounts_refuse(db: &dyn Database) {
    let journal_id = fixtures(db, "AJ", "2026-04").await;

    // Unknown account: the entry validates in the domain (the chart is
    // the database's to check), the write-time guard refuses, and the
    // diagnostic read names the account.
    let entry = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("9999", Side::Debit, eur_major(5)),
                eur_line("4000", Side::Credit, eur_major(5)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("the domain does not know the chart");
    let err = post_entry(
        db,
        &entry,
        &ctx("AJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("an unknown account refuses the post");
    assert_eq!(
        err,
        JournalError::UnknownAccount {
            account: "9999".to_owned(),
        }
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );

    // Archived account: present in the chart, retired from posting.
    // A spare account with no postings: an account that holds a balance
    // cannot be archived (AccountHasBalance), so the archived case uses one
    // that never moved money.
    commands::create_account(
        db,
        COMPANY_ID,
        ledgers_ledger::account::AccountDraft {
            number: "3500".to_owned(),
            name_key: "account.inline.spare".to_owned(),
            account_type: ledgers_ledger::account::AccountType::Expense,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: Vec::new(),
            display_name: None,
        },
        NOW.to_owned(),
        &common::person(),
    )
    .await
    .expect("create a spare expense account");
    commands::archive_account(db, COMPANY_ID, "3500", NOW.to_owned(), &common::person())
        .await
        .expect("archive the spare account");
    let entry = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("3500", Side::Debit, eur_major(5)),
                eur_line("4000", Side::Credit, eur_major(5)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("the domain does not know the archive bit");
    let err = post_entry(
        db,
        &entry,
        &ctx("AJ", 2),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("an archived account refuses the post");
    assert_eq!(
        err,
        JournalError::AccountArchived {
            account: "3500".to_owned(),
        },
        "an archived account is named as archived, not as unknown"
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );
    assert_eq!(
        counter_value(db, &journal_id, FY).await.unwrap_or(0),
        0,
        "the refused batches consumed no number"
    );
}

/// A foreign journal refuses fail-closed: the ownership guard counts
/// rows by `(id, company_id)`, so a foreign journal and a nonexistent
/// one are the same refusal, and the guard sits ahead of the numbering
/// bump, so the other company's sequence is never touched.
async fn a_foreign_journal_refuses_without_touching_their_numbering(db: &dyn Database) {
    // The other company's own chart, journal and open period.
    create_chart(db, common::OTHER_COMPANY_ID).await;
    let other_journal = journal::create_journal(
        db,
        common::OTHER_COMPANY_ID,
        JournalDraft {
            code: "BJ".to_owned(),
            kind: JournalKind::General,
            name: "The other company's journal".to_owned(),
        },
        "01JBJ00000000000000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .expect("create the other company's journal");
    open_period(db, common::OTHER_COMPANY_ID, "2026-04").await;

    // This company posting into it: refused, nothing lands, and the
    // other company's counter row never exists.
    let entry = Entry::from_draft(
        draft(
            &other_journal.id,
            vec![
                eur_line("1010", Side::Debit, eur_major(4)),
                eur_line("4000", Side::Credit, eur_major(4)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("the draft validates — ownership is the database's to check");
    let err = post_entry(
        db,
        &entry,
        &ctx("XJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("a foreign journal refuses the post");
    assert_eq!(
        err,
        JournalError::ForeignJournal {
            journal: other_journal.id.clone(),
        }
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &other_journal.id).await,
        0
    );
    assert!(
        counter_value(db, &other_journal.id, FY).await.is_none(),
        "the other company's numbering is untouched"
    );
}

/// A cross-company reversal refuses fail-closed: the other company's
/// journal is its own, so its journal guard passes and the
/// reversal-target guard refuses — the target entry belongs to this
/// company.
async fn a_cross_company_reversal_refuses(db: &dyn Database) {
    // One valid entry of this company's.
    let journal_id = fixtures(db, "XJ", "2026-04").await;
    let original = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(6)),
                eur_line("4000", Side::Credit, eur_major(6)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("the draft validates");
    let posted = post_entry(
        db,
        &original,
        &ctx("XJ", 2),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("post the original");

    // The other company "reversing" it.
    let other_journal = journal::find_journal(db, common::OTHER_COMPANY_ID, "BJ")
        .await
        .expect("the query runs")
        .expect("the previous scenario created the other company's journal");
    let reversal = Entry::from_draft(
        EntryDraft {
            reverses: Some(posted.entry_id.clone()),
            ..draft(
                &other_journal.id,
                vec![
                    eur_line("4000", Side::Debit, eur_major(6)),
                    eur_line("1010", Side::Credit, eur_major(6)),
                ],
            )
        },
        common::OTHER_COMPANY_ID,
    )
    .expect("the draft validates — the target is the database's to check");
    let err = post_entry(
        db,
        &reversal,
        &ctx("BJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("a cross-company reversal refuses the post");
    assert_eq!(
        err,
        JournalError::ForeignReversal {
            entry: posted.entry_id.clone(),
        }
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &other_journal.id).await,
        0
    );
    assert!(
        counter_value(db, &other_journal.id, FY).await.is_none(),
        "still nothing numbered for the other company"
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        1,
        "the original stands"
    );
}

/// The second line of defence, alone: raw unbalanced lines — no domain
/// validation in front — are refused by the SQL guard, consume no
/// number, and leave nothing behind; the next good post takes number 1.
/// The balance refusal has no domain variant on this path, so it
/// surfaces as the database error it is.
#[allow(clippy::too_many_lines)]
async fn the_db_guard_alone_rejects_unbalanced_lines_without_consuming_a_number(db: &dyn Database) {
    let journal_id = fixtures(db, "RJ", "2026-04").await;
    let raw_line = |account: &str, minor: i64| RawLine {
        account_number: account.to_owned(),
        base_currency: "EUR".to_owned(),
        base_minor: minor,
        tx_currency: "EUR".to_owned(),
        tx_minor: minor,
        rate_scaled: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    let raw = RawEntry {
        company_id: COMPANY_ID.to_owned(),
        journal_id: journal_id.clone(),
        document_date: "2026-04-10".to_owned(),
        posting_date: "2026-04-10".to_owned(),
        tax_point_date: "2026-04-10".to_owned(),
        base_currency: "EUR".to_owned(),
        description: "Raw unbalanced".to_owned(),
        source_documents: Vec::new(),
        reverses: None,
        external_id: None,
        lines: vec![raw_line("1010", 100), raw_line("4000", -99)],
    };

    let first = ctx("RJ", 1);
    let err = post_raw_entry(
        db,
        &raw,
        &first,
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("the SQL guard refuses unbalanced lines on its own");
    assert!(
        matches!(err, JournalError::Database(_)),
        "the fired guard is a refusal, never a conflict: {err:?}"
    );
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );
    assert_eq!(
        count(db, "journal_line", "entry_id", &first.entry_id).await,
        0
    );
    assert_eq!(
        counter_value(db, &journal_id, FY).await.unwrap_or(0),
        0,
        "the refused batch consumed no number"
    );

    // A single line is refused too (the >= 2 conjunct).
    let single = ctx("RJ", 2);
    let one_line = RawEntry {
        lines: vec![raw_line("1010", 100)],
        ..raw
    };
    let err = post_raw_entry(
        db,
        &one_line,
        &single,
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("a one-line entry is refused");
    assert!(matches!(err, JournalError::Database(_)));

    // The next good post takes number 1 — no gap was left — and its
    // outbox event carries the payload.
    let third = ctx("RJ", 3);
    let entry = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(1)),
                eur_line("4000", Side::Credit, eur_major(1)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("valid");
    let posted = post_entry(
        db,
        &entry,
        &third,
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("the next post succeeds");
    assert_eq!(posted.number, 1, "the sequence starts clean");

    let events = db
        .query(&Statement::with_values(
            "SELECT topic, payload FROM outbox WHERE id = ?".to_owned(),
            vec![SeaValue::from(third.outbox_id.as_str())],
        ))
        .await
        .expect("read the outbox");
    let row = events.first().expect("one event per committed post");
    assert_eq!(
        row.get::<String>("topic").expect("topic"),
        TOPIC_ENTRY_POSTED
    );
    let payload: serde_json::Value =
        serde_json::from_str(&row.get::<String>("payload").expect("payload")).expect("json");
    assert_eq!(payload["entryId"], third.entry_id);
    assert_eq!(payload["vatPeriod"], "2026-04");
    assert!(
        payload.get("number").is_none(),
        "the payload names the entry, never its number"
    );
}

/// Immutability at the storage layer: the triggers refuse UPDATE and
/// DELETE on lines unconditionally and on posted entries, and the rows
/// survive the attempts. This runs on every dialect — SQLite's RAISE
/// and Postgres's plpgsql exception surface identically as failed
/// statements carrying the refusal text.
async fn posted_rows_are_immutable_at_the_storage_layer(db: &dyn Database) {
    let journal_id = fixtures(db, "IJ", "2026-04").await;
    let entry = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(7)),
                eur_line("4000", Side::Credit, eur_major(7)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("valid");
    let posted = post_entry(
        db,
        &entry,
        &ctx("IJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("post");
    let entry_id = posted.entry_id.clone();

    for sql in [
        format!("UPDATE journal_line SET memo = 'no' WHERE entry_id = '{entry_id}'"),
        format!("DELETE FROM journal_line WHERE entry_id = '{entry_id}'"),
        format!("UPDATE journal_entry SET description = 'no' WHERE id = '{entry_id}'"),
        format!("DELETE FROM journal_entry WHERE id = '{entry_id}'"),
    ] {
        let err = db
            .execute(&Statement::new(sql.clone()))
            .await
            .expect_err("the trigger refuses the statement");
        let text = err.to_string().to_ascii_lowercase();
        assert!(
            text.contains("append-only") || text.contains("immutable"),
            "the refusal must name the rule, on every dialect: {sql} -> {err}"
        );
    }
    assert_eq!(
        count(db, "journal_line", "entry_id", &entry_id).await,
        2,
        "the lines survived"
    );
    assert_eq!(
        count(db, "journal_entry", "id", &entry_id).await,
        1,
        "the entry survived"
    );
}

/// Reversals are new entries whose `reverses` names the original; the
/// relation reads back through a query because no mutable `reversed_by`
/// column exists.
async fn reversals_read_back_through_the_reverses_relation(db: &dyn Database) {
    let journal_id = fixtures(db, "VJ", "2026-04").await;
    let original = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(30)),
                eur_line("4000", Side::Credit, eur_major(30)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("valid");
    let posted = post_entry(
        db,
        &original,
        &ctx("VJ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("post the original");

    // The correction: the mirrored entry, flipping every side.
    let flipped: Vec<Line> = original
        .lines
        .iter()
        .map(|line| Line {
            account_number: line.account_number.clone(),
            side: line.side.flip(),
            base: line.base,
            fx: line.fx,
            tax_code: line.tax_code.clone(),
            dimensions: line.dimensions.clone(),
            memo: line.memo.clone(),
        })
        .collect();
    let reversal = Entry::from_draft(
        EntryDraft {
            reverses: Some(posted.entry_id.clone()),
            description: "Reverses the scenario entry".to_owned(),
            ..draft(&journal_id, flipped)
        },
        COMPANY_ID,
    )
    .expect("the reversal validates");
    let reversal_posted = post_entry(
        db,
        &reversal,
        &ctx("VJ", 2),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("post the reversal");
    assert_eq!(reversal_posted.number, 2, "the reversal is just a post");

    assert_eq!(
        reversed_by(db, COMPANY_ID, &posted.entry_id)
            .await
            .expect("query"),
        vec![reversal_posted.entry_id.clone()],
        "the relation reads back, scoped to the company"
    );
    assert!(
        reversed_by(db, COMPANY_ID, &reversal_posted.entry_id)
            .await
            .expect("query")
            .is_empty(),
        "nothing reverses the reversal yet"
    );
    assert!(
        reversed_by(db, common::OTHER_COMPANY_ID, &posted.entry_id)
            .await
            .expect("query")
            .is_empty(),
        "another company sees none of it"
    );
}

/// A March-posting draft (for the closed-period scenario).
fn draft_march(journal_id: &str) -> EntryDraft {
    EntryDraft {
        posting_date: Date::from_calendar_date(2026, Month::March, 10).expect("a date"),
        ..draft(
            journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(3)),
                eur_line("4000", Side::Credit, eur_major(3)),
            ],
        )
    }
}

/// A May-posting draft (for the unknown-period scenario).
fn draft_may(journal_id: &str) -> EntryDraft {
    EntryDraft {
        posting_date: Date::from_calendar_date(2026, Month::May, 10).expect("a date"),
        ..draft(
            journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(5)),
                eur_line("4000", Side::Credit, eur_major(5)),
            ],
        )
    }
}

#[test]
fn no_source_updates_or_deletes_the_journal_tables() {
    // The no-update/no-delete contract, scanned mechanically over every
    // crate's src tree (tests and migrations excluded: the tests
    // exercise the triggers' refusals, the migrations carry them).
    let crates_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let mut files = Vec::new();
    let Ok(crates) = std::fs::read_dir(&crates_root) else {
        panic!("cannot read {}", crates_root.display());
    };
    for entry in crates.flatten() {
        // Only the shipped source: src/, never tests/, migrations/ or
        // anything else a crate directory collects.
        collect_rust_sources(&entry.path().join("src"), &mut files);
    }
    assert!(
        files.len() >= 8,
        "found only {} Rust sources under {} — the walk itself broke, and a \
         silent miss would make this test vacuous",
        files.len(),
        crates_root.display()
    );

    let banned = [
        "update journal_entry",
        "update journal_line",
        "delete from journal_entry",
        "delete from journal_line",
    ];
    let mut findings = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        let lowered = text.to_ascii_lowercase();
        for token in banned {
            if lowered.contains(token) {
                findings.push(format!("{}: contains `{token}`", path.display()));
            }
        }
        // The query-builder route: a file that names the journal tables
        // must not build UPDATE/DELETE queries at all.
        let touches_journal = lowered.contains("journal_entry") || lowered.contains("journal_line");
        let builds_writes =
            lowered.contains("query::update()") || lowered.contains("query::delete()");
        if touches_journal && builds_writes {
            findings.push(format!(
                "{}: names the journal tables and builds an UPDATE/DELETE query",
                path.display()
            ));
        }
    }
    assert!(
        findings.is_empty(),
        "posted entries and their lines are immutable (docs/PLAN.md §3 rule 3): correct with a \
         reversal, never an UPDATE or DELETE:\n  {}",
        findings.join("\n  ")
    );
}

/// Collects every `.rs` file under `dir`, recursively.
fn collect_rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn test_empty_company_can_be_closed() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(test_empty_company_can_be_closed_impl(&*kit.db));
    }
}

/// A company with a USD base currency and no entries at all closes its
/// fiscal year: the close reads the base currency from the company row, not
/// from posted entries (there are none to read it from).
async fn test_empty_company_can_be_closed_impl(db: &dyn Database) {
    let company_id = "01COMPANY_TEST_USD00000000000";
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{company_id}', 'Test Company USD', 'USD', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .expect("create the USD company");

    let template = ledgers_ledger::generic_sme_template().expect("the bundled chart parses");
    commands::import_template(db, company_id, &template, NOW.to_owned(), &common::person())
        .await
        .expect("import the chart");
    journal::create_journal(
        db,
        company_id,
        JournalDraft {
            code: "GJ".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        "01JEMPTYCLOSE0000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .expect("create the general journal");
    period::open_fiscal_year(
        db,
        company_id,
        FiscalYearSpec {
            label: 2026,
            start_month: 1,
            has_adjustment_period: false,
        },
        NOW.to_owned(),
        &common::person(),
    )
    .await
    .expect("open fiscal 2026");
    for month in 1..=11 {
        period::close_period(
            db,
            company_id,
            &format!("2026-{month:02}"),
            &EntryJournal,
            &CloseCheckRegistry::new(),
            None,
            NOW.to_owned(),
            &common::person(),
        )
        .await
        .unwrap_or_else(|err| panic!("close 2026-{month:02}: {err}"));
    }
    period::open_fiscal_year(
        db,
        company_id,
        FiscalYearSpec {
            label: 2027,
            start_month: 1,
            has_adjustment_period: false,
        },
        NOW.to_owned(),
        &common::person(),
    )
    .await
    .expect("open fiscal 2027");
    let report = period::close_fiscal_year(
        db,
        company_id,
        2026,
        &EntryJournal,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &common::person(),
    )
    .await
    .expect("the empty company's fiscal year closes");
    assert!(
        report.closing_entry.is_none(),
        "a year with nothing to clear posts no closing entry"
    );
}

#[test]
fn test_post_with_mismatched_currency_is_refused() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
        ]
    }) {
        pollster::block_on(test_post_with_mismatched_currency_is_refused_impl(&*kit.db));
    }
}

async fn test_post_with_mismatched_currency_is_refused_impl(db: &dyn Database) {
    // Create the test company with EUR base currency
    let company_id = "01COMPANY_TEST_EUR00000000000".to_string();
    db.execute(&Statement::new(
        format!(
            "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
             VALUES ('{company_id}', 'Test Company EUR', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ),
    ))
    .await
    .expect("create the company");

    // Create chart and journal for the scenario
    create_chart(db, &company_id).await;
    let journal = journal::create_journal(
        db,
        &company_id,
        JournalDraft {
            code: "CU".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        "01JCUTEST000000000000000000000",
        NOW,
        &common::person(),
    )
    .await
    .unwrap_or_else(|err| panic!("create journal CU: {err}"));
    open_period(db, &company_id, "2026-04").await;
    let journal_id = journal.id;

    // The company has EUR as base currency (from fixtures which uses eur())
    // Try to post an entry with USD base currency
    let raw_line = |account: &str, minor: i64| RawLine {
        account_number: account.to_owned(),
        base_currency: "USD".to_owned(), // Mismatch: company is EUR
        base_minor: minor,
        tx_currency: "USD".to_owned(),
        tx_minor: minor,
        rate_scaled: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    let raw = RawEntry {
        company_id: company_id.clone(),
        journal_id: journal_id.clone(),
        document_date: "2026-04-10".to_owned(),
        posting_date: "2026-04-10".to_owned(),
        tax_point_date: "2026-04-10".to_owned(),
        base_currency: "USD".to_owned(), // Mismatch: company is EUR
        description: "Currency mismatch test".to_owned(),
        source_documents: Vec::new(),
        external_id: None,
        reverses: None,
        lines: vec![raw_line("1010", 100), raw_line("4000", -100)],
    };

    let ctx = ctx("CU", 1);
    let err = post_raw_entry(
        db,
        &raw,
        &ctx,
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect_err("posting with mismatched currency is refused");

    assert_eq!(
        err,
        JournalError::BaseCurrencyMismatch {
            company_currency: "EUR".to_owned(),
            entry_currency: "USD".to_owned(),
        },
        "the refusal maps to BaseCurrencyMismatch"
    );

    // Verify nothing was posted
    assert_eq!(
        count(db, "journal_entry", "journal_id", &journal_id).await,
        0
    );
    assert_eq!(
        counter_value(db, &journal_id, FY).await.unwrap_or(0),
        0,
        "the refused batch consumed no number"
    );
}

/// Issue #68: a company's base currency may be corrected while it has no
/// entries, and is refused afterwards by the database itself (a trigger that
/// holds on every dialect, not just the code path that writes the row).
async fn base_currency_is_fixed_once_entries_exist(db: &dyn Database) {
    let fresh = "01COMPANY_TEST_FIXED0000000000";
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{fresh}', 'Fixed currency', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .expect("create the company");
    db.execute(&Statement::new(format!(
        "UPDATE company SET base_currency = 'USD' WHERE id = '{fresh}'"
    )))
    .await
    .expect("with no entries the base currency may still be corrected");

    // Once the company has an entry, the same correction is refused and the
    // stored currency does not move.
    let journal_id = fixtures(db, "UQ", "2026-04").await;
    let entry = Entry::from_draft(
        draft(
            &journal_id,
            vec![
                eur_line("1010", Side::Debit, eur_major(12)),
                eur_line("4000", Side::Credit, eur_major(12)),
            ],
        ),
        COMPANY_ID,
    )
    .expect("the draft validates");
    post_entry(
        db,
        &entry,
        &ctx("UQ", 1),
        RetryPolicy::default(),
        no_sleep,
        &common::person(),
    )
    .await
    .expect("the first entry posts");

    let refused = db
        .execute(&Statement::new(format!(
            "UPDATE company SET base_currency = 'USD' WHERE id = '{COMPANY_ID}'"
        )))
        .await;
    assert!(
        refused.is_err(),
        "the database refuses a base-currency change once entries exist"
    );
    let rows = db
        .query(&Statement::new(format!(
            "SELECT base_currency FROM company WHERE id = '{COMPANY_ID}'"
        )))
        .await
        .expect("read the company");
    assert_eq!(
        rows.rows[0].get::<String>("base_currency").as_deref(),
        Some("EUR"),
        "the stored base currency did not move"
    );
}
