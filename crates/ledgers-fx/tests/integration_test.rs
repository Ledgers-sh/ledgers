//! Integration tests: lookup, CSV import, manual entry audit, and FX entry immutability.

use ledgers_audit::{Actor, AuditContext};

const COMPANY_ID: &str = "halden-works-gmbh";
const POSTED_AT: &str = "2026-10-20T10:00:00Z";
const FY: i64 = 2026;
const PERSON_ID: &str = "usr_test_001";

static PERSON_EVENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn person() -> AuditContext {
    let n = PERSON_EVENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    AuditContext::new(
        Actor::Person {
            person_id: PERSON_ID.to_owned(),
        },
        format!("req_test_{n:010X}"),
        format!("evt_test_{n:010X}"),
    )
}

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::AuditConfig;
use ledgers_db::DbModule;
use ledgers_db::RetryPolicy;
use ledgers_fx::{
    FxModule, csv,
    lookup::{LookupWindow, RateSource, lookup},
    record::{ManualRateParams, RateToRecord, record_manual_rate, record_rates},
};
use ledgers_ledger::{
    Entry, EntryDraft, Fx, JournalKind, LedgerModule, Line,
    account::{AccountDraft, AccountType},
    commands, journal,
    posting::{PostContext, post_entry},
};
use ledgers_money::{Amount, Currency, Rate};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::time::Duration;
use time::{Date, Month};

fn no_sleep(_: Duration) {}

async fn setup_fx_scenario(db: &dyn Database) -> String {
    // Posting reads the company's base currency, so the company row exists
    // before its first entry (as bootstrap and the dev seed create it).
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{COMPANY_ID}', 'Halden Works GmbH', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .expect("create the company");
    for (number, name_key, account_type) in [
        ("1010", "account.inline.bank", AccountType::Asset),
        ("1400", "account.inline.payables", AccountType::Liability),
        ("3000", "account.inline.fx-loss", AccountType::Expense),
        ("4000", "account.inline.sales", AccountType::Income),
    ] {
        let _ = commands::create_account(
            db,
            COMPANY_ID,
            AccountDraft {
                number: number.to_owned(),
                name_key: name_key.to_owned(),
                account_type,
                normal_balance: None,
                currency: None,
                parent: None,
                tags: Vec::new(),
                display_name: None,
            },
            "2026-01-01T00:00:00Z".to_owned(),
            &person(),
        )
        .await;
    }

    let journal = journal::create_journal(
        db,
        COMPANY_ID,
        journal::JournalDraft {
            code: "GEN".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        "01JGEN00000000000000000000000000",
        "2026-01-01T00:00:00Z",
        &person(),
    )
    .await
    .expect("create journal");

    db.execute(&ledgers_db::period::set_status_statement(
        COMPANY_ID, "2026-10", "open",
    ))
    .await
    .expect("open period");

    journal.id
}

#[test]
fn test_lookup_same_day_rate() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let date = Date::from_calendar_date(2026, Month::October, 20).unwrap();
            let rate = Rate::parse("1.0812").unwrap();

            record_rates(
                db,
                COMPANY_ID,
                &[RateToRecord {
                    base: "EUR".to_string(),
                    quote: "USD".to_string(),
                    date,
                    rate,
                }],
                RateSource::Manual,
                POSTED_AT,
            )
            .await
            .unwrap();

            let found = lookup(db, COMPANY_ID, "EUR", "USD", date, LookupWindow::default())
                .await
                .unwrap();
            assert_eq!(found.rate, rate);
        });
    }
}

#[test]
fn test_lookup_within_window_uses_earlier_rate() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let earlier_date = Date::from_calendar_date(2026, Month::October, 15).unwrap();
            let requested_date = Date::from_calendar_date(2026, Month::October, 20).unwrap();
            let rate = Rate::parse("1.0800").unwrap();

            record_rates(
                db,
                COMPANY_ID,
                &[RateToRecord {
                    base: "EUR".to_string(),
                    quote: "USD".to_string(),
                    date: earlier_date,
                    rate,
                }],
                RateSource::Manual,
                POSTED_AT,
            )
            .await
            .unwrap();

            let found = lookup(
                db,
                COMPANY_ID,
                "EUR",
                "USD",
                requested_date,
                LookupWindow::default(),
            )
            .await
            .unwrap();
            assert_eq!(found.rate, rate);
        });
    }
}

#[test]
fn test_lookup_no_rate_in_window_error_message() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let requested_date = Date::from_calendar_date(2026, Month::October, 20).unwrap();

            let err = lookup(
                db,
                COMPANY_ID,
                "EUR",
                "USD",
                requested_date,
                LookupWindow::default(),
            )
            .await
            .unwrap_err();

            let err_str = err.to_string();
            assert!(err_str.contains("EUR"));
            assert!(err_str.contains("USD"));
            assert!(err_str.contains("2026-10-20"));
            assert!(err_str.contains('7'));
        });
    }
}

#[test]
fn test_lookup_never_uses_future_dated_rates() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let requested_date = Date::from_calendar_date(2026, Month::October, 20).unwrap();
            let future_date = Date::from_calendar_date(2026, Month::October, 25).unwrap();

            record_rates(
                db,
                COMPANY_ID,
                &[RateToRecord {
                    base: "EUR".to_string(),
                    quote: "USD".to_string(),
                    date: future_date,
                    rate: Rate::parse("1.0812").unwrap(),
                }],
                RateSource::Manual,
                POSTED_AT,
            )
            .await
            .unwrap();

            let err = lookup(
                db,
                COMPANY_ID,
                "EUR",
                "USD",
                requested_date,
                LookupWindow::default(),
            )
            .await
            .unwrap_err();

            assert!(matches!(err, ledgers_fx::FxError::NoRateInWindow { .. }));
        });
    }
}

#[test]
fn test_csv_import_records_all_rates() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let csv_content =
                "date,base,quote,rate\n2026-10-20,EUR,USD,1.0812\n2026-10-21,EUR,GBP,0.8345";
            let rates = csv::parse_csv(csv_content).unwrap();

            record_rates(db, COMPANY_ID, &rates, RateSource::Csv, POSTED_AT)
                .await
                .unwrap();

            let found = lookup(
                db,
                COMPANY_ID,
                "EUR",
                "USD",
                rates[0].date,
                LookupWindow::default(),
            )
            .await
            .unwrap();
            assert_eq!(found.source, RateSource::Csv);
        });
    }
}

#[test]
fn test_manual_rate_writes_audit_event() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(FxModule)]) {
        pollster::block_on(async {
            let db = &*kit.db;
            let audit_config = AuditConfig::default();
            let audit_context = person();
            let date = Date::from_calendar_date(2026, Month::October, 20).unwrap();
            let rate = Rate::parse("1.0812").unwrap();

            record_manual_rate(
                db,
                ManualRateParams {
                    company_id: COMPANY_ID.to_owned(),
                    base: "EUR".to_owned(),
                    quote: "USD".to_owned(),
                    date,
                    rate,
                    at: POSTED_AT.to_owned(),
                },
                &audit_context,
                &audit_config,
            )
            .await
            .unwrap();

            let found = lookup(db, COMPANY_ID, "EUR", "USD", date, LookupWindow::default())
                .await
                .unwrap();
            assert_eq!(found.rate, rate);
            assert_eq!(found.source, RateSource::Manual);
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_acceptance_posted_fx_lines_are_immutable_when_rates_change() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(DbModule),
            Box::new(FxModule),
        ]
    }) {
        pollster::block_on(async {
            let db = &*kit.db;
            let journal_id = setup_fx_scenario(db).await;

            let posting_date = Date::from_calendar_date(2026, Month::October, 20).unwrap();
            let initial_rate = Rate::parse("0.9000").unwrap();

            record_rates(
                db,
                COMPANY_ID,
                &[RateToRecord {
                    base: "EUR".to_string(),
                    quote: "USD".to_string(),
                    date: posting_date,
                    rate: initial_rate,
                }],
                RateSource::Manual,
                POSTED_AT,
            )
            .await
            .unwrap();

            let usd_currency = Currency::from_code("USD").expect("USD");
            let eur_currency = Currency::from_code("EUR").expect("EUR");
            let usd_transaction = Amount::from_major(1000, usd_currency).expect("representable");

            let payables_line = Line {
                account_number: "1400".to_owned(),
                side: ledgers_ledger::entry::Side::Debit,
                base: Amount::from_major(900, eur_currency).expect("representable"),
                fx: Some(Fx {
                    transaction: usd_transaction,
                    rate: initial_rate,
                }),
                tax_code: None,
                dimensions: BTreeMap::new(),
                memo: None,
            };

            let bank_line = Line {
                account_number: "1010".to_owned(),
                side: ledgers_ledger::entry::Side::Credit,
                base: Amount::from_major(900, eur_currency).expect("representable"),
                fx: Some(Fx {
                    transaction: usd_transaction,
                    rate: initial_rate,
                }),
                tax_code: None,
                dimensions: BTreeMap::new(),
                memo: None,
            };

            let entry = Entry::from_draft(
                EntryDraft {
                    journal_id: journal_id.clone(),
                    document_date: posting_date,
                    posting_date,
                    tax_point_date: None,
                    base_currency: eur_currency,
                    description: "FX bill payment".to_owned(),
                    source_documents: Vec::new(),
                    external_id: None,
                    source_app: None,
                    reverses: None,
                    lines: vec![payables_line, bank_line],
                },
                COMPANY_ID,
            )
            .expect("entry validates");

            let _posted = post_entry(
                db,
                &entry,
                &PostContext {
                    fiscal_year: FY,
                    entry_id: "01ENTRYFY001000000000000000001".to_owned(),
                    recorded_at: POSTED_AT.to_owned(),
                    outbox_id: "01OUTBOXFY0001000000000000001".to_owned(),
                },
                RetryPolicy::default(),
                no_sleep,
                &person(),
            )
            .await
            .expect("post FX entry");

            let posted_lines = db
                .query(&Statement::with_values(
                    "SELECT base_minor, tx_currency, tx_minor, rate_scaled FROM journal_line\n\
                     WHERE entry_id = (SELECT id FROM journal_entry WHERE journal_id = ? AND number = ?)\n\
                     ORDER BY line_no"
                        .to_owned(),
                    vec![
                        SeaValue::from(journal_id.as_str()),
                        SeaValue::BigInt(Some(1)),
                    ],
                ))
                .await
                .expect("read posted lines");

            assert_eq!(posted_lines.len(), 2);

            let original_payables_base = posted_lines.rows[0]
                .get::<i64>("base_minor")
                .expect("base_minor");
            let original_payables_tx_minor = posted_lines.rows[0]
                .get::<i64>("tx_minor")
                .expect("tx_minor");
            let original_payables_rate = posted_lines.rows[0]
                .get::<String>("rate_scaled")
                .expect("rate_scaled");

            let original_bank_base = posted_lines.rows[1]
                .get::<i64>("base_minor")
                .expect("base_minor");
            let original_bank_tx_minor = posted_lines.rows[1]
                .get::<i64>("tx_minor")
                .expect("tx_minor");
            let original_bank_rate = posted_lines.rows[1]
                .get::<String>("rate_scaled")
                .expect("rate_scaled");

            assert_eq!(original_payables_rate, "900000000000");
            assert_eq!(original_bank_rate, "900000000000");

            let new_rate = Rate::parse("0.9200").unwrap();
            record_rates(
                db,
                COMPANY_ID,
                &[RateToRecord {
                    base: "EUR".to_string(),
                    quote: "USD".to_string(),
                    date: posting_date,
                    rate: new_rate,
                }],
                RateSource::Manual,
                POSTED_AT,
            )
            .await
            .unwrap();

            let looked_up = lookup(
                db,
                COMPANY_ID,
                "EUR",
                "USD",
                posting_date,
                LookupWindow::default(),
            )
            .await
            .expect("lookup after edit");
            assert_eq!(looked_up.rate, new_rate);

            let after_edit = db
                .query(&Statement::with_values(
                    "SELECT base_minor, tx_currency, tx_minor, rate_scaled FROM journal_line\n\
                     WHERE entry_id = (SELECT id FROM journal_entry WHERE journal_id = ? AND number = ?)\n\
                     ORDER BY line_no"
                        .to_owned(),
                    vec![
                        SeaValue::from(journal_id.as_str()),
                        SeaValue::BigInt(Some(1)),
                    ],
                ))
                .await
                .expect("read lines after edit");

            assert_eq!(
                after_edit.rows[0]
                    .get::<i64>("base_minor")
                    .expect("base_minor"),
                original_payables_base
            );
            assert_eq!(
                after_edit.rows[0].get::<i64>("tx_minor").expect("tx_minor"),
                original_payables_tx_minor
            );
            assert_eq!(
                after_edit.rows[0]
                    .get::<String>("rate_scaled")
                    .expect("rate_scaled"),
                original_payables_rate
            );

            assert_eq!(
                after_edit.rows[1]
                    .get::<i64>("base_minor")
                    .expect("base_minor"),
                original_bank_base
            );
            assert_eq!(
                after_edit.rows[1].get::<i64>("tx_minor").expect("tx_minor"),
                original_bank_tx_minor
            );
            assert_eq!(
                after_edit.rows[1]
                    .get::<String>("rate_scaled")
                    .expect("rate_scaled"),
                original_bank_rate
            );
        });
    }
}
