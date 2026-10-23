//! The connections commands on the real migrations, on every dialect the
//! harness offers (SQLite, and Postgres when `FZ_TEST_POSTGRES_URL` is set).
//! Each command runs through the executor as the owner, the way the server
//! runs it after the policy checks (issue #142).

use cratefield_core::{Database, DbError, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{
    Command, CommandContext, CommandError, DefaultPolicy, Executed, Executor, exec::Execute,
};
use ledgers_connect::ConnectModule;
use ledgers_connect::allocation::AllocationRule;
use ledgers_connect::commands::*;
use ledgers_connect::model::{InvoiceRecord, SyncOutput, UsageRecord};
use ledgers_connect::secret::ConnectKey;
use ledgers_db::DbModule;
use ledgers_documents::DocumentsStoreModule;
use ledgers_ledger::LedgerModule;
use sea_query::Value;
use std::sync::Arc;

const COMPANY: &str = "01TESTCOMPANY0000000000000A";
const OTHER: &str = "01OTHERCOMPANY000000000000B";
const NOW: &str = "2026-08-02T09:00:00Z";
const OWNER: &str = "owner-token";
const OWNER_ID: &str = "owner-person";
const KEY_SECRET: &str = "0123456789abcdef0123456789abcdef-test-only-key";
const TOKEN: &str = "cfat_NEVER_RETURNED_TOKEN_42";

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![
        Box::new(DbModule),
        Box::new(LedgerModule),
        Box::new(DocumentsStoreModule),
        Box::new(BillsModule),
        Box::new(ConnectModule),
    ]
}

struct Owner;

#[async_trait::async_trait]
impl Authenticator for Owner {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        if credential == OWNER {
            Ok(Principal::Person {
                person_id: OWNER_ID.to_owned(),
                role: Role::Owner,
            })
        } else {
            Err(CommandError::Unauthenticated)
        }
    }
}

fn context(company: &str, request: &str) -> CommandContext {
    CommandContext {
        company_id: company.to_owned(),
        request_id: request.to_owned(),
        event_id: format!("{request}-event"),
        now: NOW.to_owned(),
    }
}

async fn run_in<C: Command>(
    company: &str,
    db: &dyn Database,
    cmd: &C,
    request: &str,
    input: &C::Input,
) -> Result<C::Output, CommandError> {
    let executor = Executor::new(db, Owner, DefaultPolicy);
    match executor
        .run(
            cmd,
            OWNER.to_owned(),
            context(company, request),
            input,
            Execute::Run,
        )
        .await?
    {
        Executed::Done(output) => Ok(output),
        _ => panic!("expected the command to run to completion"),
    }
}

async fn exec(db: &dyn Database, sql: &str, values: Vec<Value>) {
    db.execute(&Statement::with_values(sql.to_owned(), values))
        .await
        .unwrap_or_else(|err| panic!("{err}\n{sql}"));
}

async fn count(db: &dyn Database, sql: &str) -> i64 {
    let rows = db
        .query(&Statement::new(sql.to_owned()))
        .await
        .unwrap_or_else(|err| panic!("{err}\n{sql}"));
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or_else(|| panic!("no count from {sql}"))
}

async fn seed_units(db: &dyn Database, company: &str, codes: &[(&str, &str)]) {
    for (code, name) in codes {
        exec(
            db,
            "INSERT INTO dimension_value (company_id, kind, code, name, own_books, created_at) VALUES (?, 'unit', ?, ?, 1, ?)",
            vec![Value::from(company), Value::from(*code), Value::from(*name), Value::from(NOW)],
        )
        .await;
    }
}

fn key() -> ConnectKey {
    ConnectKey::from_secret(KEY_SECRET).expect("long enough")
}

fn create_input(id: &str, name: &str) -> CreateInput {
    CreateInput {
        id: id.to_owned(),
        connector: "cloudflare".to_owned(),
        name: name.to_owned(),
        config: serde_json::json!({"account_id": "0123456789abcdef0123456789abcdef", "expense_account": "6100", "vat_rate_bp": 0}),
        default_unit: "FZ".to_owned(),
        sealed_credential: key().seal(TOKEN, id).expect("sealed"),
        created_by: OWNER_ID.to_owned(),
        demo: false,
        token: None,
    }
}

async fn create(db: &dyn Database, company: &str, id: &str, name: &str) -> ConnectionView {
    run_in(
        company,
        db,
        &ConnectionsCreate,
        &format!("create-{id}"),
        &create_input(id, name),
    )
    .await
    .expect("created")
}

fn invoice(external: &str, period: &str, total: i64, vendor: &str) -> InvoiceRecord {
    InvoiceRecord {
        external_id: external.to_owned(),
        vendor_name: vendor.to_owned(),
        invoice_number: Some(format!("INV-{external}")),
        issued_on: format!("{period}-01"),
        period: period.to_owned(),
        currency: "USD".to_owned(),
        total_minor: total,
        description: format!("{vendor} {period}"),
        hosted_url: None,
    }
}

fn usage(period: &str, id: &str, quantity: i64) -> UsageRecord {
    UsageRecord {
        period: period.to_owned(),
        resource_kind: "worker".to_owned(),
        resource_id: id.to_owned(),
        resource_name: id.to_owned(),
        quantity,
    }
}

fn sync_input(connection: &str, fetched: SyncOutput) -> SyncInput {
    SyncInput {
        connection_id: connection.to_owned(),
        since: "2026-07-01".to_owned(),
        until: "2026-08-31".to_owned(),
        fetched: Some(fetched),
        fetch_error: None,
        created_by: OWNER_ID.to_owned(),
    }
}

fn sample_answer() -> SyncOutput {
    SyncOutput {
        invoices: vec![
            invoice("cloudflare-billing:a", "2026-07", 1_001, "Cloudflare, Inc."),
            invoice("cloudflare-billing:b", "2026-08", 2_500, "Cloudflare, Inc."),
        ],
        usage: vec![
            usage("2026-07", "ledgers-web", 700),
            usage("2026-07", "ledgers-api", 200),
            usage("2026-07", "fz-dashboard", 100),
            usage("2026-08", "ledgers-web", 1),
            usage("2026-08", "fz-dashboard", 2),
        ],
        notes: vec!["R2 operations and storage are not read".to_owned()],
    }
}

async fn rules_set(
    db: &dyn Database,
    connection: &str,
    rules: Vec<(&str, &str)>,
) -> AllocationSetOutput {
    run_in(
        COMPANY,
        db,
        &AllocationRulesSet,
        &format!("rules-{connection}-{}", rules.len()),
        &AllocationSetInput {
            connection_id: connection.to_owned(),
            rules: rules
                .into_iter()
                .map(|(pattern, unit)| AllocationRule {
                    pattern: pattern.to_owned(),
                    unit_code: unit.to_owned(),
                })
                .collect(),
            default_unit: None,
        },
    )
    .await
    .expect("rules set")
}

#[test]
fn every_connections_command_is_classified_for_the_registry() {
    // The registry's human-only flag is what keeps agents off the writes.
    let commands = ledgers_commands::default_commands();
    let human_only = |name: &str| {
        commands
            .iter()
            .find(|command| command.name == name)
            .map(|command| command.human_only)
    };
    for name in [
        "connections.create",
        "connections.update",
        "connections.delete",
        "connections.sync_now",
        "allocation_rules.set",
    ] {
        assert_eq!(human_only(name), Some(true), "{name} is human only");
    }
    for name in [
        "connections.list",
        "connections.runs",
        "allocation_rules.list",
        "reports.costs_by_unit",
    ] {
        assert_eq!(
            human_only(name),
            Some(false),
            "{name} is a read agents may run"
        );
    }
}

#[test]
fn the_credential_is_write_only_and_the_list_never_shows_it() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            let view = create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            assert!(view.has_credential);
            let listed = run_in(COMPANY, &*kit.db, &ConnectionsList, "list-1", &ListInput {})
                .await
                .unwrap();
            let json = serde_json::to_string(&listed).unwrap();
            assert!(!json.contains(TOKEN), "the token is never returned");
            assert!(!json.contains("v1."), "nor its sealed form");
            assert_eq!(listed.connections.len(), 1);
            assert!(listed.connections[0].has_credential);
            // The sealed form is stored, bound to the connection id.
            let sealed = ledgers_connect::store::sealed_credential(&*kit.db, COMPANY, "conn-cf")
                .await
                .unwrap()
                .expect("stored");
            assert_eq!(key().open(&sealed, "conn-cf").unwrap(), TOKEN);
        });
    }
}

#[test]
fn create_refuses_bad_input_before_anything_is_written() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            let mut secret_key = create_input("c1", "Bad keys");
            secret_key.config = serde_json::json!({"account_id": "0123456789abcdef0123456789abcdef", "expense_account": "6100", "api_token": "x"});
            assert!(
                matches!(
                    run_in(COMPANY, &*kit.db, &ConnectionsCreate, "bad-1", &secret_key).await,
                    Err(CommandError::InvalidInput(_))
                ),
                "an unknown config key (a secret smuggled in) is refused"
            );

            let mut no_unit = create_input("c2", "No unit");
            no_unit.default_unit = "NOPE".to_owned();
            assert!(matches!(
                run_in(COMPANY, &*kit.db, &ConnectionsCreate, "bad-2", &no_unit).await,
                Err(CommandError::InvalidInput(_))
            ));

            let mut unknown = create_input("c3", "Unknown connector");
            unknown.connector = "salesforce".to_owned();
            assert!(
                run_in(COMPANY, &*kit.db, &ConnectionsCreate, "bad-3", &unknown)
                    .await
                    .is_err()
            );

            create(&*kit.db, COMPANY, "conn-one", "Same name").await;
            assert!(
                matches!(
                    run_in(
                        COMPANY,
                        &*kit.db,
                        &ConnectionsCreate,
                        "bad-4",
                        &create_input("conn-two", "Same name")
                    )
                    .await,
                    Err(CommandError::InvalidInput(_))
                ),
                "a second connection with the same name is refused"
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_connection").await,
                1
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_credential").await,
                1
            );
        });
    }
}

#[test]
fn a_sync_imports_draft_bills_split_by_usage_and_a_repeat_creates_nothing() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(
                &*kit.db,
                COMPANY,
                &[("FZ", "Factory Zero"), ("LED", "Ledgers")],
            )
            .await;
            create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            rules_set(&*kit.db, "conn-cf", vec![("ledgers-*", "LED")]).await;

            let first = run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-1",
                &sync_input("conn-cf", sample_answer()),
            )
            .await
            .unwrap();
            assert_eq!(first.status, "succeeded");
            assert_eq!(first.invoices_found, 2);
            assert_eq!(first.bills_created, 2);
            assert_eq!(count(&*kit.db, "SELECT COUNT(*) AS n FROM bill").await, 2);
            assert_eq!(
                count(
                    &*kit.db,
                    "SELECT COUNT(*) AS n FROM vendor WHERE name = 'Cloudflare, Inc.'"
                )
                .await,
                1,
                "one vendor for both invoices"
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM document").await,
                2,
                "one source document per invoice"
            );

            // Each invoice's splits sum to its net amount (total, no VAT configured).
            let rows = kit
                .db
                .query(&Statement::new("SELECT i.total_minor AS total, CAST(SUM(s.amount_minor) AS BIGINT) AS parts FROM connector_invoice i JOIN connector_split s ON s.invoice_id = i.id GROUP BY i.id, i.total_minor".to_owned()))
                .await
                .unwrap();
            assert_eq!(rows.rows.len(), 2);
            for row in &rows.rows {
                assert_eq!(
                    row.get::<i64>("total"),
                    row.get::<i64>("parts"),
                    "the split sums to the invoice"
                );
            }

            // July: ledgers-web and ledgers-api are LED (900 of 1000 usage), fz-dashboard is unmapped (FZ, 100).
            // 1001 * 900 / 1000 = 900.9 and 1001 * 100 / 1000 = 100.1: largest remainder gives LED the spare cent.
            let july_led = count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split s JOIN connector_invoice i ON i.id = s.invoice_id WHERE s.unit_code = 'LED' AND i.period = '2026-07'").await;
            assert_eq!(july_led, 901);

            let again = run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-2",
                &sync_input("conn-cf", sample_answer()),
            )
            .await
            .unwrap();
            assert_eq!(
                again.bills_created, 0,
                "re-syncing the same invoices creates nothing"
            );
            assert_eq!(count(&*kit.db, "SELECT COUNT(*) AS n FROM bill").await, 2);
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_invoice").await,
                2
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM document").await,
                2
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_run").await,
                2,
                "each run is logged"
            );
        });
    }
}

#[test]
fn an_unmapped_resource_books_to_the_default_then_mapping_it_resplits_the_draft() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(
                &*kit.db,
                COMPANY,
                &[("FZ", "Factory Zero"), ("SQZ", "Sqeezin")],
            )
            .await;
            create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            let mut answer = sample_answer();
            answer.usage = vec![
                usage("2026-07", "sqeezin-worker", 300),
                usage("2026-07", "mystery-worker", 100),
            ];
            answer.invoices = vec![invoice(
                "cloudflare-billing:x",
                "2026-07",
                1_000,
                "Cloudflare, Inc.",
            )];
            let run = run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-u",
                &sync_input("conn-cf", answer),
            )
            .await
            .unwrap();
            assert!(
                run.notes.iter().any(
                    |n| n.contains("no allocation rule") && n.contains("worker:mystery-worker")
                )
            );

            let listed = run_in(
                COMPANY,
                &*kit.db,
                &AllocationRulesList,
                "alloc-1",
                &AllocationListInput::default(),
            )
            .await
            .unwrap();
            let queue = &listed.connections[0].unmapped;
            assert_eq!(queue.len(), 2);
            let sqz = queue
                .iter()
                .find(|r| r.resource_id == "sqeezin-worker")
                .unwrap();
            assert_eq!(
                sqz.suggested_unit.as_deref(),
                Some("SQZ"),
                "a name suggestion, never a mapping"
            );

            // Before mapping: everything is FZ (default), the unmapped one included.
            let before = count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split WHERE unit_code = 'FZ'").await;
            assert_eq!(before, 1_000);

            let set = rules_set(
                &*kit.db,
                "conn-cf",
                vec![("sqeezin-*", "SQZ"), ("mystery-worker", "SQZ")],
            )
            .await;
            assert_eq!(set.resplit, 1);
            assert_eq!(set.kept_frozen, 0);
            let after_sqz = count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split WHERE unit_code = 'SQZ'").await;
            assert_eq!(
                after_sqz, 1_000,
                "mapping both resources moves the whole invoice to SQZ"
            );
            assert_eq!(count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split WHERE unit_code = 'FZ'").await, 0);

            // The draft's lines follow the new split, and still total the net.
            let lines = kit
                .db
                .query(&Statement::new(
                    "SELECT lines_json, net_minor FROM bill".to_owned(),
                ))
                .await
                .unwrap();
            let json = lines.rows[0].get::<String>("lines_json").unwrap();
            assert!(json.contains("\"unit_code\":\"SQZ\""));
            assert!(!json.contains("\"unit_code\":\"FZ\""));
            assert_eq!(lines.rows[0].get::<i64>("net_minor"), Some(1_000));
        });
    }
}

#[test]
fn a_posted_bill_keeps_its_split_when_the_rules_change() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(
                &*kit.db,
                COMPANY,
                &[("FZ", "Factory Zero"), ("LED", "Ledgers")],
            )
            .await;
            create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            let mut answer = sample_answer();
            answer.usage = vec![usage("2026-07", "ledgers-web", 1)];
            answer.invoices = vec![invoice(
                "cloudflare-billing:p",
                "2026-07",
                500,
                "Cloudflare, Inc.",
            )];
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-p",
                &sync_input("conn-cf", answer),
            )
            .await
            .unwrap();
            exec(
                &*kit.db,
                "UPDATE bill SET posted_entry_id = 'entry-1'",
                vec![],
            )
            .await;
            let set = rules_set(&*kit.db, "conn-cf", vec![("ledgers-*", "LED")]).await;
            assert_eq!(set.resplit, 0);
            assert_eq!(set.kept_frozen, 1);
            assert_eq!(count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split WHERE unit_code = 'FZ'").await, 500);
        });
    }
}

#[test]
fn a_failed_fetch_is_a_failed_run_and_writes_no_invoice() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            let failed = SyncInput {
                connection_id: "conn-cf".to_owned(),
                since: "2026-07-01".to_owned(),
                until: "2026-08-31".to_owned(),
                fetched: None,
                fetch_error: Some(
                    "api.cloudflare.com refused the token (403); check the token's scopes"
                        .to_owned(),
                ),
                created_by: OWNER_ID.to_owned(),
            };
            let run = run_in(COMPANY, &*kit.db, &ConnectionsSyncNow, "sync-f", &failed)
                .await
                .unwrap();
            assert_eq!(run.status, "failed");
            assert!(run.error_message.unwrap().contains("refused"));
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_invoice").await,
                0
            );
            assert_eq!(count(&*kit.db, "SELECT COUNT(*) AS n FROM bill").await, 0);
            let runs = run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsRuns,
                "runs-f",
                &RunsInput {
                    connection_id: "conn-cf".to_owned(),
                    limit: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(runs.runs.len(), 1);
        });
    }
}

#[test]
fn companies_do_not_see_each_others_connections() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            seed_units(&*kit.db, OTHER, &[("FZ", "Other")]).await;
            create(&*kit.db, COMPANY, "conn-mine", "Mine").await;
            create(&*kit.db, OTHER, "conn-theirs", "Theirs").await;
            let mine = run_in(COMPANY, &*kit.db, &ConnectionsList, "iso-1", &ListInput {})
                .await
                .unwrap();
            assert_eq!(
                mine.connections
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                ["conn-mine"]
            );
            let theirs = run_in(OTHER, &*kit.db, &ConnectionsList, "iso-2", &ListInput {})
                .await
                .unwrap();
            assert_eq!(
                theirs
                    .connections
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                ["conn-theirs"]
            );
            // The other company cannot sync or re-map this connection.
            assert!(
                run_in(
                    OTHER,
                    &*kit.db,
                    &ConnectionsSyncNow,
                    "iso-3",
                    &sync_input("conn-mine", sample_answer())
                )
                .await
                .is_err()
            );
            assert!(
                run_in(
                    OTHER,
                    &*kit.db,
                    &ConnectionsDelete,
                    "iso-4",
                    &DeleteInput {
                        id: "conn-mine".to_owned()
                    }
                )
                .await
                .is_err()
            );
            assert_eq!(
                count(
                    &*kit.db,
                    "SELECT COUNT(*) AS n FROM connector_connection WHERE deleted_at IS NULL"
                )
                .await,
                2
            );
        });
    }
}

#[test]
fn delete_removes_the_token_and_keeps_the_books() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            create(&*kit.db, COMPANY, "conn-del", "To delete").await;
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-d",
                &sync_input("conn-del", sample_answer()),
            )
            .await
            .unwrap();
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsDelete,
                "del-1",
                &DeleteInput {
                    id: "conn-del".to_owned(),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_credential").await,
                0,
                "the token is gone"
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM bill").await,
                2,
                "the drafted bills stay"
            );
            assert_eq!(
                count(&*kit.db, "SELECT COUNT(*) AS n FROM connector_run").await,
                1,
                "the history stays"
            );
            let listed = run_in(COMPANY, &*kit.db, &ConnectionsList, "list-d", &ListInput {})
                .await
                .unwrap();
            assert!(listed.connections.is_empty());
        });
    }
}

#[test]
fn the_cost_report_totals_each_unit_per_month_and_source() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(
                &*kit.db,
                COMPANY,
                &[("FZ", "Factory Zero"), ("LED", "Ledgers")],
            )
            .await;
            create(&*kit.db, COMPANY, "conn-cf", "Cloudflare (test)").await;
            rules_set(&*kit.db, "conn-cf", vec![("ledgers-*", "LED")]).await;
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-r",
                &sync_input("conn-cf", sample_answer()),
            )
            .await
            .unwrap();
            let report = run_in(
                COMPANY,
                &*kit.db,
                &ReportsCostsByUnit,
                "report-1",
                &CostsInput {
                    from: "2026-07".to_owned(),
                    to: "2026-08".to_owned(),
                },
            )
            .await
            .unwrap();
            let total: i64 = report.lines.iter().map(|line| line.amount_minor).sum();
            assert_eq!(total, 1_001 + 2_500, "every invoice is accounted for once");
            assert!(report.lines.iter().all(|l| l.connector == "cloudflare"));
            let july_led = report
                .lines
                .iter()
                .find(|l| l.period == "2026-07" && l.unit_code == "LED")
                .unwrap();
            assert_eq!(july_led.amount_minor, 901);
            let bad = run_in(
                COMPANY,
                &*kit.db,
                &ReportsCostsByUnit,
                "report-2",
                &CostsInput {
                    from: "2026-08".to_owned(),
                    to: "2026-07".to_owned(),
                },
            )
            .await;
            assert!(matches!(bad, Err(CommandError::InvalidInput(_))));
        });
    }
}

#[test]
fn the_run_log_cannot_be_rewritten() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(&*kit.db, COMPANY, &[("FZ", "Factory Zero")]).await;
            create(&*kit.db, COMPANY, "conn-log", "Log").await;
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-l",
                &sync_input("conn-log", sample_answer()),
            )
            .await
            .unwrap();
            let update: Result<u64, DbError> = kit
                .db
                .execute(&Statement::new(
                    "UPDATE connector_run SET status = 'succeeded'".to_owned(),
                ))
                .await;
            assert!(update.is_err(), "connector_run is append-only");
            let delete: Result<u64, DbError> = kit
                .db
                .execute(&Statement::new("DELETE FROM connector_run".to_owned()))
                .await;
            assert!(delete.is_err(), "connector_run is append-only");
        });
    }
}

#[test]
fn an_update_to_the_default_unit_resplits_the_drafts_and_a_bad_unit_is_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            seed_units(
                &*kit.db,
                COMPANY,
                &[("FZ", "Factory Zero"), ("LED", "Ledgers")],
            )
            .await;
            create(&*kit.db, COMPANY, "conn-up", "Update").await;
            let mut answer = sample_answer();
            answer.usage = Vec::new();
            answer.invoices = vec![invoice(
                "cloudflare-billing:u",
                "2026-07",
                800,
                "Cloudflare, Inc.",
            )];
            run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsSyncNow,
                "sync-up",
                &sync_input("conn-up", answer),
            )
            .await
            .unwrap();
            let bad = run_in(
                COMPANY,
                &*kit.db,
                &AllocationRulesSet,
                "bad-unit",
                &AllocationSetInput {
                    connection_id: "conn-up".to_owned(),
                    rules: Vec::new(),
                    default_unit: Some("GHOST".to_owned()),
                },
            )
            .await;
            assert!(matches!(bad, Err(CommandError::InvalidInput(_))));
            let moved = run_in(
                COMPANY,
                &*kit.db,
                &ConnectionsUpdate,
                "up-1",
                &UpdateInput {
                    id: "conn-up".to_owned(),
                    default_unit: Some("LED".to_owned()),
                    ..UpdateInput::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(moved.default_unit, "LED");
            assert_eq!(count(&*kit.db, "SELECT CAST(COALESCE(SUM(amount_minor), 0) AS BIGINT) AS n FROM connector_split WHERE unit_code = 'LED'").await, 800);
        });
    }
}

#[test]
fn invoice_sources_are_content_addressed() {
    let a = invoice("x", "2026-07", 100, "V");
    let b = invoice("x", "2026-07", 101, "V");
    let (sha_a, bytes_a) = invoice_source(&a);
    let (sha_b, _) = invoice_source(&b);
    assert_eq!(sha_a.len(), 64);
    assert_ne!(sha_a, sha_b);
    assert_eq!(sha_a, invoice_source(&a).0);
    assert!(!bytes_a.is_empty());
    let _ = Arc::new(());
}
