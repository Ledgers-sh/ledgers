//! The bank store's SQL on every backing engine the environment provides:
//! in-memory SQLite always, a throwaway Postgres 16 when `FZ_TEST_POSTGRES_URL`
//! names one (CI's service container, `--features ledgers-bank/postgres`).
//! One scenario, every dialect: rows are written with the portable DDL and
//! read back through the store functions, and the write-time guard refuses a
//! line that is no longer unmatched.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bank::BankModule;
use ledgers_bank::store;
use ledgers_bills::BillsModule;
use ledgers_db::DbModule;
use sea_query::Value;

const COMPANY: &str = "01BANKMATRIXCOMPANY0000001";
const ACCOUNT: &str = "01BANKMATRIXACCOUNT00000001";
const STATEMENT: &str = "01BANKMATRIXSTATEMENT0000001";

async fn exec(db: &dyn Database, sql: &str, values: Vec<Value>) {
    db.execute(&Statement::with_values(sql.to_owned(), values))
        .await
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
}

async fn seed(db: &dyn Database) {
    exec(
        db,
        "INSERT INTO bank_account (id, company_id, name, iban, bic, currency, ledger_account_number, \
         opening_minor, opening_date, csv_mapping, archived_at, created_by, created_at, updated_at) \
         VALUES (?, ?, 'Girokonto', 'DE64100200300400500600', NULL, 'EUR', '1020', 1250000, '2026-03-01', \
         NULL, NULL, 'person:owner', '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z')",
        vec![Value::from(ACCOUNT), Value::from(COMPANY)],
    )
    .await;
    exec(
        db,
        "INSERT INTO bank_statement (id, company_id, account_id, document_id, ordinal, format, reference, \
         currency, opening_date, opening_minor, closing_date, closing_minor, balance_checked, line_count, \
         created_by, created_at) VALUES (?, ?, ?, 'doc-1', 0, 'camt053', NULL, 'EUR', '2026-03-01', 1250000, \
         '2026-03-31', 1654060, 1, 3, 'person:owner', '2026-03-31T00:00:00Z')",
        vec![Value::from(STATEMENT), Value::from(COMPANY), Value::from(ACCOUNT)],
    )
    .await;
    for (id, date, minor, status, remittance) in [
        (
            "01BANKMATRIXLINE00000000001",
            "2026-03-02",
            -123_450i64,
            "unmatched",
            "Rechnung 4711",
        ),
        (
            "01BANKMATRIXLINE00000000002",
            "2026-03-03",
            420_000,
            "unmatched",
            "RE 2026-031",
        ),
        (
            "01BANKMATRIXLINE00000000003",
            "2026-03-27",
            -2_500,
            "ignored",
            "Entgelt",
        ),
    ] {
        exec(
            db,
            "INSERT INTO bank_transaction (id, company_id, account_id, statement_id, fingerprint, group_key, \
             booking_date, value_date, minor, currency, counterparty_name, counterparty_iban, remittance, \
             bank_reference, status, matched_entry_id, matched_bill_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, 'EUR', 'Halden Supplies GmbH', NULL, ?, NULL, ?, NULL, NULL, \
             '2026-03-31T00:00:00Z', '2026-03-31T00:00:00Z')",
            vec![
                Value::from(id),
                Value::from(COMPANY),
                Value::from(ACCOUNT),
                Value::from(STATEMENT),
                Value::from(format!("fp-{id}")),
                Value::from(format!("group-{date}-{minor}")),
                Value::from(date),
                Value::from(minor),
                Value::from(remittance),
                Value::from(status),
            ],
        )
        .await;
    }
    exec(
        db,
        "INSERT INTO bank_rule (id, company_id, name, contains_text, counterparty_iban, amount_min, amount_max, \
         account_number, tax_code, unit, enabled, proposed_by, enabled_by, created_at, updated_at) \
         VALUES ('01BANKMATRIXRULE000000000001', ?, 'Rechnungen', 'Rechnung', NULL, NULL, NULL, '5030', NULL, \
         NULL, 1, 'agent:key', 'person:owner', '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z')",
        vec![Value::from(COMPANY)],
    )
    .await;
}

#[allow(clippy::too_many_lines)]
async fn scenario(db: &dyn Database) {
    seed(db).await;
    let accounts = store::accounts(db, COMPANY, false).await.expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].opening_minor, 1_250_000);
    assert!(
        store::account(db, COMPANY, ACCOUNT)
            .await
            .expect("account")
            .is_some()
    );
    assert!(
        store::account(db, "OTHER", ACCOUNT)
            .await
            .expect("account")
            .is_none(),
        "company-scoped"
    );

    let statements = store::statements_of(db, COMPANY, &[ACCOUNT.to_owned()])
        .await
        .expect("statements");
    assert_eq!(statements.len(), 1);
    assert_eq!(statements[0].closing_minor, Some(1_654_060));
    let imported = store::imported_statements(db, COMPANY, ACCOUNT, "doc-1")
        .await
        .expect("imported");
    assert_eq!(imported, vec![(0, STATEMENT.to_owned())]);

    let unmatched = store::transactions(
        db,
        COMPANY,
        &store::TransactionFilter {
            status: Some("unmatched".into()),
            take: 10,
            ..Default::default()
        },
    )
    .await
    .expect("unmatched");
    assert_eq!(unmatched.len(), 2);
    assert_eq!(unmatched[0].booking_date, "2026-03-03", "newest first");
    let search = store::transactions(
        db,
        COMPANY,
        &store::TransactionFilter {
            search: Some("rechnung".into()),
            take: 10,
            ..Default::default()
        },
    )
    .await
    .expect("search");
    assert_eq!(search.len(), 1);
    let page = store::transactions(
        db,
        COMPANY,
        &store::TransactionFilter {
            after: Some(("2026-03-03".into(), "01BANKMATRIXLINE00000000002".into())),
            take: 10,
            ..Default::default()
        },
    )
    .await
    .expect("page");
    assert_eq!(
        page.len(),
        1,
        "only the line before the cursor is on the next page"
    );

    let groups = store::group_counts(db, COMPANY, ACCOUNT)
        .await
        .expect("groups");
    assert_eq!(groups.values().sum::<u32>(), 3);
    let counts = store::status_counts(db, COMPANY).await.expect("counts");
    assert_eq!(
        counts.get(&(ACCOUNT.to_owned(), "unmatched".to_owned())),
        Some(&2)
    );
    assert_eq!(
        counts.get(&(ACCOUNT.to_owned(), "ignored".to_owned())),
        Some(&1)
    );

    let rules = store::rules(db, COMPANY, true).await.expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].conditions.contains.as_deref(), Some("Rechnung"));
    assert!(
        store::rule(db, COMPANY, &rules[0].id)
            .await
            .expect("rule")
            .is_some()
    );

    // No posted lines and no bills yet: the candidate reads return nothing, without error.
    assert!(
        store::open_bills(db, COMPANY, "EUR")
            .await
            .expect("bills")
            .is_empty()
    );
    assert!(
        store::account_lines(db, COMPANY, &["1020".to_owned()], 50)
            .await
            .expect("lines")
            .is_empty()
    );
    assert!(
        store::ledger_lines(db, COMPANY, &["1020".to_owned()])
            .await
            .expect("ledger")
            .is_empty()
    );

    // The write-time guard: a false condition fails the whole batch.
    let refused = db
        .batch_atomic(&[ledgers_db::guard::assert_statement(
            "bank-unmatched:01BANKMATRIXLINE00000000003",
            "(SELECT status FROM bank_transaction WHERE company_id = ? AND id = ?) = 'unmatched'",
            vec![
                Value::from(COMPANY),
                Value::from("01BANKMATRIXLINE00000000003"),
            ],
        )])
        .await;
    assert!(
        refused.is_err(),
        "an ignored line fails the unmatched guard"
    );
}

async fn exec_party(
    db: &dyn Database,
    id: &str,
    name: &str,
    iban: &str,
    merged_into: Option<&str>,
) {
    exec(
        db,
        "INSERT INTO vendor (id, company_id, name, iban, source, needs_review, merged_into, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 'manual', 0, ?, '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z')",
        vec![
            Value::from(id),
            Value::from(COMPANY),
            Value::from(name),
            Value::from(iban),
            merged_into.map_or(Value::String(None), Value::from),
        ],
    )
    .await;
}

/// A bank counterparty is the party whose IBAN it is, however the IBAN is
/// spaced; a merged party is no longer a counterparty.
async fn parties_match_by_iban(db: &dyn Database) {
    seed(db).await;
    exec_party(
        db,
        "party-live",
        "Halden Works GmbH",
        "DE89 3704 0044 0532 0130 00",
        None,
    )
    .await;
    exec_party(
        db,
        "party-old",
        "Halden Old",
        "GB82WEST12345698765432",
        Some("party-live"),
    )
    .await;
    let by_iban = store::parties_by_iban(db, COMPANY).await.expect("parties");
    let live = by_iban
        .get("DE89370400440532013000")
        .expect("matched by normalised IBAN");
    assert_eq!(live.id, "party-live");
    assert_eq!(live.name, "Halden Works GmbH");
    assert!(
        !by_iban.contains_key("GB82WEST12345698765432"),
        "a merged party is not a counterparty"
    );
    let other = store::parties_by_iban(db, "01OTHERCOMPANY000000000000B")
        .await
        .expect("parties of another company");
    assert!(
        other.is_empty(),
        "another company's parties are not this company's counterparties"
    );
}

/// A bill row for the payables read: posted or not, discarded or not, in one currency.
async fn exec_bill(
    db: &dyn Database,
    id: &str,
    vendor_id: &str,
    currency: &str,
    gross_minor: i64,
    posted: bool,
    discarded: bool,
) {
    exec(
        db,
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, currency, gross_minor, \
         lines_json, checks_json, instruction_flags, iban_changed, source, posted_entry_id, discarded_at, \
         created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, '[]', '[]', '[]', 0, 'manual', ?, ?, 'test', \
         '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z')",
        vec![
            Value::from(id),
            Value::from(COMPANY),
            Value::from(format!("doc-{id}").as_str()),
            Value::from(vendor_id),
            Value::from(format!("INV-{id}").as_str()),
            Value::from(currency),
            Value::from(gross_minor),
            if posted {
                Value::from(format!("entry-{id}").as_str())
            } else {
                Value::String(None)
            },
            if discarded {
                Value::from("2026-03-02T00:00:00Z")
            } else {
                Value::String(None)
            },
        ],
    )
    .await;
}

/// `parties.balances`: posted, undiscarded bills that no bank line settled, of
/// unmerged parties, grouped by party and currency.
async fn party_balances(db: &dyn Database) {
    seed(db).await;
    exec_party(
        db,
        "party-a",
        "Halden Works GmbH",
        "DE89370400440532013000",
        None,
    )
    .await;
    exec_party(db, "party-b", "Acme GmbH", "GB82WEST12345698765432", None).await;
    exec_party(
        db,
        "party-gone",
        "Old Co",
        "FR1420041010050500013M02606",
        Some("party-a"),
    )
    .await;
    exec_bill(db, "bill-open-a", "party-a", "EUR", 100_000, true, false).await;
    exec_bill(db, "bill-settled-a", "party-a", "EUR", 50_000, true, false).await;
    exec(
        db,
        "INSERT INTO bank_transaction (id, company_id, account_id, statement_id, fingerprint, group_key, \
         booking_date, value_date, minor, currency, counterparty_name, counterparty_iban, remittance, \
         bank_reference, status, matched_entry_id, matched_bill_id, created_at, updated_at) \
         VALUES ('bank-settles-a', ?, ?, ?, 'fp-settles-a', 'g-settles-a', '2026-03-10', NULL, -50000, 'EUR', \
         'Halden Works GmbH', NULL, 'INV-settled', NULL, 'matched', NULL, 'bill-settled-a', \
         '2026-03-10T00:00:00Z', '2026-03-10T00:00:00Z')",
        vec![Value::from(COMPANY), Value::from(ACCOUNT), Value::from(STATEMENT)],
    )
    .await;
    exec_bill(db, "bill-draft-a", "party-a", "EUR", 20_000, false, false).await;
    exec_bill(db, "bill-discarded-b", "party-b", "EUR", 30_000, true, true).await;
    exec_bill(db, "bill-usd-b", "party-b", "USD", 70_000, true, false).await;
    exec_bill(
        db,
        "bill-merged-gone",
        "party-gone",
        "EUR",
        90_000,
        true,
        false,
    )
    .await;

    let rows = ledgers_bank::party_balances::open_payables(db, COMPANY)
        .await
        .expect("balances");
    let got: Vec<(String, String, i64, u32)> = rows
        .iter()
        .map(|r| {
            (
                r.party_id.clone(),
                r.currency.clone(),
                r.payable_minor,
                r.open_bills,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("party-a".to_string(), "EUR".to_string(), 100_000, 1),
            ("party-b".to_string(), "USD".to_string(), 70_000, 1),
        ],
        "only open bills of live parties; settled, draft, discarded and merged are left out"
    );
}

#[test]
fn party_balances_count_only_open_payables_on_every_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(ledgers_ledger::LedgerModule),
            Box::new(DbModule),
            Box::new(BillsModule),
            Box::new(BankModule),
        ]
    }) {
        pollster::block_on(party_balances(&*kit.db));
    }
}

#[test]
fn parties_are_matched_to_counterparties_by_iban_on_every_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(ledgers_ledger::LedgerModule),
            Box::new(DbModule),
            Box::new(BillsModule),
            Box::new(BankModule),
        ]
    }) {
        pollster::block_on(parties_match_by_iban(&*kit.db));
    }
}

#[test]
fn bank_store_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| {
        vec![
            Box::new(ledgers_storage::StorageModule),
            Box::new(ledgers_ledger::LedgerModule),
            Box::new(DbModule),
            Box::new(BillsModule),
            Box::new(BankModule),
        ]
    }) {
        pollster::block_on(scenario(&*kit.db));
    }
}
