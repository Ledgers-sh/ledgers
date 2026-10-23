//! Business units: the unit registry (create, rename, archive, audited),
//! the unit rule on profit-and-loss lines, recharges between units, the
//! consolidated elimination, and the side-by-side summary.
//!
//! Each case runs on every available dialect through the harness kit.

mod common;

use common::{COMPANY_ID, FY, NOW, OTHER_COMPANY_ID, POSTED_AT, agent, mount, open_period, person};
use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::RetryPolicy;
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::commands;
use ledgers_ledger::entry::Side;
use ledgers_ledger::posting::PostContext;
use ledgers_ledger::report;
use ledgers_ledger::units::{self, TransferDraft, UnitChange, UnitDraft, UnitScope};
use ledgers_ledger::{AccountsError, Entry, EntryDraft, JournalError, LedgerModule, Line};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use time::{Date, Month};

const PERIOD: &str = "2026-04";
const JOURNAL: &str = "general";

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the table")
}

fn date(day: u8) -> Date {
    Date::from_calendar_date(2026, Month::April, day).expect("a valid April date")
}

/// One fresh harness per available dialect; each case body runs on all.
fn harness_kits() -> Vec<TestHarness> {
    TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(LedgerModule),
            Box::new(ledgers_db::DbModule),
        ]
    })
}

/// Creates a chart account with tags.
async fn account(
    db: &dyn Database,
    company: &str,
    number: &str,
    account_type: AccountType,
    tags: &[&str],
) {
    commands::create_account(
        db,
        company,
        AccountDraft {
            number: number.to_owned(),
            name_key: format!("account.inline.{number}"),
            account_type,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            display_name: None,
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .unwrap_or_else(|err| panic!("create account {number}: {err}"));
}

/// The chart a unit company needs: the scenario accounts plus a shared
/// cost expense, the inter-unit clearing account and the due pair.
async fn unit_chart(db: &dyn Database, company: &str) {
    mount(db, company).await;
    account(db, company, "6100", AccountType::Expense, &[]).await;
    account(
        db,
        company,
        "9000",
        AccountType::Equity,
        &["inter-unit-clearing"],
    )
    .await;
    account(
        db,
        company,
        "1070",
        AccountType::Asset,
        &["inter-unit-due-from"],
    )
    .await;
    account(
        db,
        company,
        "2050",
        AccountType::Liability,
        &["inter-unit-due-to"],
    )
    .await;
    db.execute(&Statement::new(format!(
        "INSERT INTO journal (id, company_id, code, kind, name, created_at) \
         VALUES ('{JOURNAL}', '{company}', 'GN', 'general', 'General', '{NOW}')"
    )))
    .await
    .unwrap();
    open_period(db, company, PERIOD).await;
}

fn line(account: &str, side: Side, minor: i128, unit: Option<&str>) -> Line {
    let dimensions = match unit {
        Some(code) => BTreeMap::from([("unit".to_owned(), code.to_owned())]),
        None => BTreeMap::new(),
    };
    Line {
        account_number: account.to_owned(),
        side,
        base: Amount::new(minor, eur()),
        fx: None,
        tax_code: None,
        dimensions,
        memo: None,
    }
}

fn draft(lines: Vec<Line>, description: &str) -> EntryDraft {
    EntryDraft {
        journal_id: JOURNAL.to_owned(),
        document_date: date(3),
        posting_date: date(3),
        tax_point_date: None,
        base_currency: eur(),
        description: description.to_owned(),
        source_documents: vec![],
        external_id: None,
        source_app: None,
        reverses: None,
        lines,
    }
}

async fn post(
    db: &dyn Database,
    company: &str,
    draft: EntryDraft,
    entry_id: &str,
) -> Result<ledgers_ledger::PostedEntry, JournalError> {
    let entry = Entry::from_draft(draft, company)?;
    let ctx = PostContext {
        fiscal_year: FY,
        entry_id: entry_id.to_owned(),
        recorded_at: POSTED_AT.to_owned(),
        outbox_id: format!("{entry_id}-outbox"),
    };
    ledgers_ledger::post_entry(db, &entry, &ctx, RetryPolicy::default(), |_| {}, &agent()).await
}

async fn create_unit(db: &dyn Database, company: &str, code: &str, name: &str) {
    units::create_unit(
        db,
        company,
        UnitDraft {
            code: code.to_owned(),
            name: name.to_owned(),
            colour: None,
            logo_document_id: None,
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .unwrap_or_else(|err| panic!("create unit {code}: {err}"));
}

/// Posts a shared cost: the factory pays 1000 for a service from the bank
/// (no unit on the bank line), and the factory unit carries the cost.
async fn post_shared_cost(db: &dyn Database, company: &str) {
    post(
        db,
        company,
        draft(
            vec![
                line("6100", Side::Debit, 100_000, Some("FAC")),
                line("1010", Side::Credit, 100_000, None),
            ],
            "Shared service paid",
        ),
        "01HQZX9K3M7VN2P4R6T8W1S001",
    )
    .await
    .expect("the shared cost posts");
}

fn transfer(from: &str, to: &str, minor: i128) -> TransferDraft {
    TransferDraft {
        journal_id: JOURNAL.to_owned(),
        from_unit: from.to_owned(),
        to_unit: to.to_owned(),
        amount: Amount::new(minor, eur()),
        document_date: date(5),
        posting_date: date(5),
        tax_point_date: None,
        description: "Shared-cost recharge".to_owned(),
        recovery_account: "4000".to_owned(),
        cost_account: "6100".to_owned(),
        base_currency: eur(),
    }
}

async fn post_transfer(db: &dyn Database, company: &str, t: &TransferDraft, entry_id: &str) {
    let planned = units::plan_transfer(db, company, t)
        .await
        .expect("the transfer plans");
    post(db, company, planned, entry_id)
        .await
        .expect("the transfer posts");
}

async fn audit_commands(db: &dyn Database, company: &str) -> Vec<String> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT payload FROM audit_event WHERE company_id = ? ORDER BY seq".to_owned(),
            vec![SeaValue::from(company)],
        ))
        .await
        .unwrap();
    rows.rows
        .iter()
        .filter_map(|row| row.get::<String>("payload"))
        .filter_map(|payload| {
            let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
            value.get("command")?.as_str().map(str::to_owned)
        })
        .collect()
}

// ---------------------------------------------------------------- registry

#[test]
#[allow(clippy::too_many_lines)]
fn unit_registry_is_audited_and_validated() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "fac", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;

            let listed = units::list_units(db, COMPANY_ID, false).await.unwrap();
            let codes: Vec<&str> = listed.iter().map(|u| u.code.as_str()).collect();
            assert_eq!(
                codes,
                ["FAC", "LED"],
                "codes are upper-cased and listed by code"
            );
            assert_eq!(listed[0].colour, units::DEFAULT_COLOUR);

            // Rename and recolour in one change; the other fields stay.
            let renamed = units::rename_unit(
                db,
                COMPANY_ID,
                "LED",
                UnitChange {
                    name: Some("  Ledgers GmbH  ".to_owned()),
                    colour: Some("#e9b44c".to_owned()),
                    logo_document_id: None,
                },
                NOW.to_owned(),
                &person(),
            )
            .await
            .unwrap();
            assert_eq!(renamed.name, "Ledgers GmbH", "names are trimmed");
            assert_eq!(renamed.colour, "#E9B44C", "colours are upper-cased");

            // Archive: hidden from the default list, kept with include_archived.
            units::archive_unit(db, COMPANY_ID, "LED", NOW.to_owned(), &person())
                .await
                .unwrap();
            assert_eq!(
                units::list_units(db, COMPANY_ID, false)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                units::list_units(db, COMPANY_ID, true).await.unwrap().len(),
                2
            );
            assert!(
                matches!(
                    units::rename_unit(
                        db,
                        COMPANY_ID,
                        "LED",
                        UnitChange::default(),
                        NOW.to_owned(),
                        &person()
                    )
                    .await,
                    Err(AccountsError::UnitArchived { .. })
                ),
                "an archived unit takes no changes"
            );
            assert!(matches!(
                units::archive_unit(db, COMPANY_ID, "LED", NOW.to_owned(), &person()).await,
                Err(AccountsError::UnitArchived { .. })
            ));

            // Refusals name the problem.
            let bad = |code: &str| UnitDraft {
                code: code.to_owned(),
                name: "X".to_owned(),
                colour: None,
                logo_document_id: None,
            };
            for code in ["A", "TOO-LONG-CODE-XXXXX", "ALL", "A B"] {
                assert!(
                    matches!(
                        units::create_unit(db, COMPANY_ID, bad(code), NOW.to_owned(), &person())
                            .await,
                        Err(AccountsError::InvalidUnit { .. })
                    ),
                    "{code:?} is not a unit code"
                );
            }
            assert!(matches!(
                units::create_unit(db, COMPANY_ID, bad("FAC"), NOW.to_owned(), &person()).await,
                Err(AccountsError::UnitExists { .. })
            ));
            assert!(matches!(
                units::create_unit(
                    db,
                    COMPANY_ID,
                    UnitDraft {
                        colour: Some("blue".to_owned()),
                        ..bad("TEN")
                    },
                    NOW.to_owned(),
                    &person()
                )
                .await,
                Err(AccountsError::InvalidUnit { .. })
            ));
            let logo_refused = units::create_unit(
                db,
                COMPANY_ID,
                UnitDraft {
                    logo_document_id: Some("NO-SUCH-DOCUMENT".to_owned()),
                    ..bad("LOGO")
                },
                NOW.to_owned(),
                &person(),
            )
            .await;
            assert!(
                logo_refused.is_err(),
                "a logo must name a stored document of the company"
            );

            // Every write is audited in its own batch, with its command name.
            let commands = audit_commands(db, COMPANY_ID).await;
            assert_eq!(
                commands
                    .iter()
                    .filter(|c| *c == units::COMMAND_CREATE_UNIT)
                    .count(),
                2
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|c| *c == units::COMMAND_RENAME_UNIT)
                    .count(),
                1
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|c| *c == units::COMMAND_ARCHIVE_UNIT)
                    .count(),
                1
            );
        });
    }
}

#[test]
fn a_unit_needs_the_clearing_account_and_the_chart_tags_must_be_unique() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            // OTHER_COMPANY_ID has the scenario chart but no clearing account.
            mount(db, OTHER_COMPANY_ID).await;
            let refused = units::create_unit(
                db,
                OTHER_COMPANY_ID,
                UnitDraft {
                    code: "FAC".to_owned(),
                    name: "Factory".to_owned(),
                    colour: None,
                    logo_document_id: None,
                },
                NOW.to_owned(),
                &person(),
            )
            .await;
            assert!(matches!(
                refused,
                Err(AccountsError::ChartTagMissing { .. })
            ));
            assert!(!units::units_enabled(db, OTHER_COMPANY_ID).await.unwrap());
        });
    }
}

// ------------------------------------------------------- the unit rule

#[test]
fn profit_and_loss_lines_need_a_unit_once_units_exist() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            // No units yet: a company-level P&L line is fine.
            post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("6100", Side::Debit, 5_000, None),
                        line("1010", Side::Credit, 5_000, None),
                    ],
                    "Before units",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S010",
            )
            .await
            .expect("without units a company-level line posts");

            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            assert!(units::units_enabled(db, COMPANY_ID).await.unwrap());

            let refused = post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("6100", Side::Debit, 5_000, None),
                        line("1010", Side::Credit, 5_000, None),
                    ],
                    "Unattributed cost",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S011",
            )
            .await;
            assert!(refused.is_err(), "a P&L line without a unit is refused");

            // The bank line is balance-sheet: company-level is still fine.
            post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("6100", Side::Debit, 5_000, Some("FAC")),
                        line("1010", Side::Credit, 5_000, None),
                    ],
                    "Attributed cost",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S012",
            )
            .await
            .expect("an attributed P&L line with a company-level bank line posts");

            // Archiving the last unit turns the rule off again.
            units::archive_unit(db, COMPANY_ID, "FAC", NOW.to_owned(), &person())
                .await
                .unwrap();
            assert!(!units::units_enabled(db, COMPANY_ID).await.unwrap());
        });
    }
}

// --------------------------------------------- recharges and elimination

#[test]
fn a_recharge_balances_per_unit_and_the_consolidated_view_eliminates_it() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;

            post_shared_cost(db, COMPANY_ID).await;
            // The factory recharges 400 of the cost to Ledgers.
            post_transfer(
                db,
                COMPANY_ID,
                &transfer("FAC", "LED", 40_000),
                "01HQZX9K3M7VN2P4R6T8W1S020",
            )
            .await;

            let fac = UnitScope::Unit("FAC".to_owned());
            let led = UnitScope::Unit("LED".to_owned());

            // Each unit's own P&L shows its part of the recharge.
            let fac_pnl = report::profit_and_loss_scoped(db, COMPANY_ID, PERIOD, &fac)
                .await
                .unwrap();
            assert_eq!(fac_pnl.revenue.minor(), 40_000, "factory recovers 400");
            assert_eq!(
                fac_pnl.expenses.minor(),
                100_000,
                "factory bears its own 1000"
            );
            let led_pnl = report::profit_and_loss_scoped(db, COMPANY_ID, PERIOD, &led)
                .await
                .unwrap();
            assert_eq!(led_pnl.revenue.minor(), 0);
            assert_eq!(
                led_pnl.expenses.minor(),
                40_000,
                "ledgers carries the recharged 400"
            );

            // The consolidated P&L is the external one: the recharge is gone
            // from both sides, so only the 1000 shared cost remains.
            let company = report::profit_and_loss(db, COMPANY_ID, PERIOD)
                .await
                .unwrap();
            assert_eq!(company.revenue.minor(), 0, "the recovery is eliminated");
            assert_eq!(
                company.expenses.minor(),
                100_000,
                "the cost is counted once"
            );
            assert_eq!(company.net_income.minor(), -100_000);

            // The per-unit sums overshoot the company by exactly the recharge.
            assert_eq!(
                fac_pnl.revenue.minor() + led_pnl.revenue.minor() - company.revenue.minor(),
                40_000
            );
            assert_eq!(
                fac_pnl.expenses.minor() + led_pnl.expenses.minor() - company.expenses.minor(),
                40_000
            );

            // The consolidated balance sheet balances, with no due balances left.
            let sheet = report::balance_sheet_scoped(
                db,
                COMPANY_ID,
                "2026-04-30",
                &UnitScope::Consolidated,
            )
            .await
            .unwrap();
            assert!(sheet.balanced, "the consolidated sheet balances");
            let due = report::balance_sheet_scoped(db, COMPANY_ID, "2026-04-30", &led)
                .await
                .unwrap();
            assert!(due.balanced, "a unit's own sheet balances too");

            // The unit's sheet carries the payable; the company's does not.
            assert_eq!(due.liabilities.minor(), 40_000, "ledgers owes factory 400");
            assert_eq!(
                sheet.liabilities.minor(),
                0,
                "consolidated: no due balances"
            );

            // The trial balance by unit: the factory's due receivable is 400.
            let fac_tb = report::trial_balance_scoped(db, COMPANY_ID, PERIOD, &fac)
                .await
                .unwrap();
            let due_from = fac_tb
                .iter()
                .find(|row| row.account_code == "1070")
                .expect("factory due row");
            assert_eq!(due_from.balance.minor(), 40_000);
            let company_tb = report::trial_balance(db, COMPANY_ID, PERIOD).await.unwrap();
            assert!(
                company_tb
                    .iter()
                    .all(|row| row.account_code != "1070" && row.account_code != "2050"),
                "the consolidated trial balance drops the due accounts"
            );
        });
    }
}

#[test]
fn the_units_summary_adds_up_and_names_the_eliminations() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;
            create_unit(db, COMPANY_ID, "SQZ", "Sqeezin").await;
            post_shared_cost(db, COMPANY_ID).await;
            post_transfer(
                db,
                COMPANY_ID,
                &transfer("FAC", "LED", 40_000),
                "01HQZX9K3M7VN2P4R6T8W1S030",
            )
            .await;
            post_transfer(
                db,
                COMPANY_ID,
                &transfer("FAC", "SQZ", 10_000),
                "01HQZX9K3M7VN2P4R6T8W1S031",
            )
            .await;

            let summary = report::units_summary(db, COMPANY_ID, PERIOD).await.unwrap();
            let by = |code: &str| {
                summary
                    .units
                    .iter()
                    .find(|row| row.unit.as_deref() == Some(code))
                    .unwrap_or_else(|| panic!("a row for {code}"))
                    .clone()
            };
            let fac = by("FAC");
            assert_eq!(fac.revenue.minor(), 50_000);
            assert_eq!(fac.expenses.minor(), 100_000);
            assert_eq!(fac.result.minor(), -50_000);
            assert_eq!(by("LED").expenses.minor(), 40_000);
            assert_eq!(by("SQZ").expenses.minor(), 10_000);

            // Consolidated is external only; eliminations reconcile the rows.
            assert_eq!(summary.consolidated.revenue.minor(), 0);
            assert_eq!(summary.consolidated.expenses.minor(), 100_000);
            assert_eq!(summary.eliminations.revenue.minor(), -50_000);
            assert_eq!(summary.eliminations.expenses.minor(), -50_000);
            let sum_expenses: i128 = summary.units.iter().map(|row| row.expenses.minor()).sum();
            assert_eq!(
                sum_expenses + summary.eliminations.expenses.minor(),
                summary.consolidated.expenses.minor()
            );
        });
    }
}

#[test]
fn the_external_revenue_of_the_units_adds_up_to_the_company_revenue() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;
            // Factory recharges 40,000 to Ledgers: recharge income on FAC, which
            // is intercompany and not company revenue.
            post_transfer(
                db,
                COMPANY_ID,
                &transfer("FAC", "LED", 40_000),
                "01HQZX9K3M7VN2P4R6T8W1S050",
            )
            .await;
            // Ledgers sells 70,000 to a customer: the only external revenue.
            post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("1010", Side::Debit, 70_000, Some("LED")),
                        line("4000", Side::Credit, 70_000, Some("LED")),
                    ],
                    "Customer sale",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S051",
            )
            .await
            .expect("the sale posts");

            let summary = report::units_summary(db, COMPANY_ID, PERIOD).await.unwrap();
            let external = |code: &str| {
                summary
                    .units
                    .iter()
                    .find(|row| row.unit.as_deref() == Some(code))
                    .unwrap_or_else(|| panic!("a row for {code}"))
                    .external_revenue
                    .minor()
            };
            // Factory's own books still show its recharge income...
            assert_eq!(
                summary
                    .units
                    .iter()
                    .find(|row| row.unit.as_deref() == Some("FAC"))
                    .unwrap()
                    .revenue
                    .minor(),
                40_000
            );
            // ...but it has no external revenue, so its share is zero.
            assert_eq!(external("FAC"), 0);
            assert_eq!(external("LED"), 70_000);
            assert_eq!(summary.consolidated.revenue.minor(), 70_000);
            let sum: i128 = summary
                .units
                .iter()
                .map(|row| row.external_revenue.minor())
                .sum();
            assert_eq!(
                sum,
                summary.consolidated.revenue.minor(),
                "the shares add up to 100%"
            );
        });
    }
}

#[test]
fn an_intercompany_entry_must_net_its_profit_and_loss_to_zero() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            // A due-from leg with a one-sided income: the entry balances, but
            // its P&L does not net to zero, so the elimination would be wrong.
            let refused = post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("1070", Side::Debit, 50_000, Some("FAC")),
                        line("4000", Side::Credit, 50_000, Some("FAC")),
                    ],
                    "One-sided recharge",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S040",
            )
            .await;
            assert!(
                refused.is_err(),
                "an unbalanced-P&L intercompany entry is refused"
            );
        });
    }
}

#[test]
fn a_settlement_between_units_posts_and_stays_in_the_unit_views() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;
            post_shared_cost(db, COMPANY_ID).await;
            post_transfer(
                db,
                COMPANY_ID,
                &transfer("FAC", "LED", 40_000),
                "01HQZX9K3M7VN2P4R6T8W1S050",
            )
            .await;
            // Ledgers pays factory 400 from the bank: a due-to settlement.
            post(
                db,
                COMPANY_ID,
                draft(
                    vec![
                        line("2050", Side::Debit, 40_000, Some("LED")),
                        line("1010", Side::Credit, 40_000, None),
                    ],
                    "Settled",
                ),
                "01HQZX9K3M7VN2P4R6T8W1S051",
            )
            .await
            .expect("a settlement posts");

            let led = UnitScope::Unit("LED".to_owned());
            let led_tb = report::trial_balance_scoped(db, COMPANY_ID, PERIOD, &led)
                .await
                .unwrap();
            let payable = led_tb
                .iter()
                .find(|row| row.account_code == "2050")
                .expect("ledgers payable");
            assert_eq!(
                payable.balance.minor(),
                0,
                "the payable is settled in the unit view"
            );
        });
    }
}

#[test]
fn transfers_refuse_the_wrong_units_and_accounts() {
    for kit in harness_kits() {
        pollster::block_on(async {
            let db = &*kit.db;
            unit_chart(db, COMPANY_ID).await;
            create_unit(db, COMPANY_ID, "FAC", "Factory").await;
            create_unit(db, COMPANY_ID, "LED", "Ledgers").await;

            let same = units::plan_transfer(db, COMPANY_ID, &transfer("FAC", "FAC", 1_000)).await;
            assert!(matches!(same, Err(AccountsError::InvalidUnit { .. })));
            let zero = units::plan_transfer(db, COMPANY_ID, &transfer("FAC", "LED", 0)).await;
            assert!(matches!(zero, Err(AccountsError::InvalidUnit { .. })));
            let missing =
                units::plan_transfer(db, COMPANY_ID, &transfer("FAC", "NOPE", 1_000)).await;
            assert!(matches!(missing, Err(AccountsError::UnitNotFound { .. })));
            let mut wrong_class = transfer("FAC", "LED", 1_000);
            wrong_class.recovery_account = "6100".to_owned();
            assert!(matches!(
                units::plan_transfer(db, COMPANY_ID, &wrong_class).await,
                Err(AccountsError::TransferAccount { .. })
            ));
            units::archive_unit(db, COMPANY_ID, "LED", NOW.to_owned(), &person())
                .await
                .unwrap();
            let retired =
                units::plan_transfer(db, COMPANY_ID, &transfer("FAC", "LED", 1_000)).await;
            assert!(matches!(retired, Err(AccountsError::UnitArchived { .. })));
        });
    }
}

#[test]
fn unit_scope_parses_all_and_codes() {
    assert_eq!(UnitScope::parse(None).unwrap(), UnitScope::Consolidated);
    assert_eq!(
        UnitScope::parse(Some("all")).unwrap(),
        UnitScope::Consolidated
    );
    assert_eq!(
        UnitScope::parse(Some("ALL")).unwrap(),
        UnitScope::Consolidated
    );
    assert_eq!(
        UnitScope::parse(Some("fac")).unwrap(),
        UnitScope::Unit("FAC".to_owned())
    );
    assert!(UnitScope::parse(Some("a-b")).is_err());
}

#[test]
fn names_and_colours_are_checked_before_anything_is_written() {
    assert!(
        units::validate_name("   ").is_err(),
        "an empty name is refused"
    );
    assert!(
        units::validate_name(&"x".repeat(81)).is_err(),
        "a long name is refused"
    );
    assert!(
        units::validate_name("Fac\tory").is_err(),
        "control characters are refused"
    );
    assert_eq!(units::validate_name(" Factory ").unwrap(), "Factory");
    assert!(units::validate_colour("#12345").is_err());
    assert!(units::validate_colour("12345678").is_err());
    assert_eq!(units::validate_colour("#abcdef").unwrap(), "#ABCDEF");
    assert_eq!(
        UnitChange::default().name,
        None,
        "a change with no fields changes nothing"
    );
}
