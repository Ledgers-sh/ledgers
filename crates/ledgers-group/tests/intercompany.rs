//! Intercompany agreements on the real ledger, on every dialect the harness
//! offers (SQLite always, Postgres with `FZ_TEST_POSTGRES_URL`).
//!
//! Two companies in one group: the operating company (the licensee) pays the
//! IP company (the licensor) a 5% royalty on its revenue. Each test drives the
//! commands through the real executor, the way the server does, and the
//! posting port is either the executor itself or a wrapper that injects a
//! failure on the second side.

use async_trait::async_trait;
use cratefield_core::{Database, DbError, Module, Rows, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{Actor, AuditContext};
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{
    Command, CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
};
use ledgers_db::DbModule;
use ledgers_group::GroupModule;
use ledgers_group::accrual::{Step, external_id};
use ledgers_group::commands::GroupViewInput;
use ledgers_group::commands::{
    AgreementIdInput, AgreementsCreate, AgreementsCreateInput, AgreementsGet, AgreementsGetOutput,
    AgreementsRunPeriod, AgreementsTerminate, GroupsCreate, GroupsCreateInput, GroupsList,
    GroupsListInput, MemberInput, PeriodsInput, ReconciliationInput, RunPeriodInput,
};
use ledgers_group::model::AccountsInput;
use ledgers_group::model::{Basis, GroupKind, Kind, Side, WhtBasis};
use ledgers_group::orchestrate::{self, SideExecutor};
use ledgers_group::reports::{
    GroupViewOutput, GroupViewReport, ReconciliationOutput, ReconciliationReport,
};
use ledgers_ledger::journal::{JournalDraft, JournalKind};
use ledgers_ledger::surface::{
    PostEntry, PostEntryInput, PostEntryOutput, SerializableAmount, SerializableEntryDraft,
    SerializableLine,
};
use ledgers_ledger::{FiscalYearSpec, LedgerModule};
use sea_query::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const OPS: &str = "01ICOPSCOMPANY00000000000A";
const IP: &str = "01ICIPCOMPANY000000000000B";
const OTHER: &str = "01ICOTHERCOMPANY0000000000C";
const GROUP: &str = "01ICGROUP000000000000000G";
const AGREEMENT: &str = "01ICAGREEMENT00000000000A";
const NOW: &str = "2026-03-10T09:00:00Z";
const OWNER: &str = "person-owner";
const TOKEN: &str = "bearer-owner";
const SGD: &str = "SGD";

struct Auth(BTreeMap<String, Principal>);

impl Auth {
    fn owner() -> Self {
        Auth(BTreeMap::from([(
            TOKEN.to_owned(),
            Principal::Person {
                person_id: OWNER.to_owned(),
                role: Role::Owner,
            },
        )]))
    }
}

#[async_trait]
impl Authenticator for Auth {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        self.0
            .get(credential)
            .cloned()
            .ok_or(CommandError::Unauthenticated)
    }
}

fn modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(DbModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(LedgerModule),
        Box::new(GroupModule),
    ]
}

/// Every call gets its own event id, as the server's id generator gives.
static EVENTS: AtomicUsize = AtomicUsize::new(0);

fn ctx(company: &str, request: &str) -> CommandContext {
    CommandContext {
        company_id: company.to_owned(),
        request_id: request.to_owned(),
        event_id: format!("{request}-event-{}", EVENTS.fetch_add(1, Ordering::SeqCst)),
        now: NOW.to_owned(),
    }
}

/// Runs a command as the owner in `company`, the way the executor does.
async fn run<C: Command>(
    db: &dyn Database,
    company: &str,
    cmd: &C,
    request: &str,
    input: &C::Input,
) -> Result<C::Output, CommandError> {
    let executor = Executor::new(db, Auth::owner(), DefaultPolicy);
    match executor
        .run(
            cmd,
            TOKEN.to_owned(),
            ctx(company, request),
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
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
}

/// A company with the generic chart, its general journal and the 2026 year.
async fn company(db: &dyn Database, id: &str, name: &str, country_pack: &str) {
    exec(
        db,
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        vec![Value::from(id), Value::from(name), Value::from(SGD), Value::from(NOW), Value::from(NOW)],
    )
    .await;
    exec(
        db,
        "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, 'country_pack', ?, ?)",
        vec![Value::from(id), Value::from(country_pack), Value::from(NOW)],
    )
    .await;
    let audit = AuditContext::new(
        Actor::Person {
            person_id: OWNER.to_owned(),
        },
        "seed",
        "seed-event",
    );
    let chart = ledgers_ledger::generic_sme_template().expect("chart");
    ledgers_ledger::commands::import_template(db, id, &chart, NOW.to_owned(), &audit)
        .await
        .expect("chart");
    ledgers_ledger::journal::create_journal(
        db,
        id,
        JournalDraft {
            code: "GN".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        &format!("gl-{id}"),
        NOW,
        &audit,
    )
    .await
    .expect("journal");
    ledgers_ledger::period::open_fiscal_year(
        db,
        id,
        FiscalYearSpec {
            label: 2026,
            start_month: 1,
            has_adjustment_period: false,
        },
        NOW.to_owned(),
        &audit,
    )
    .await
    .expect("fiscal year");
}

/// Revenue of `minor` into the company, posted on `date` (sales 4010 against bank 1020).
async fn revenue(db: &dyn Database, company_id: &str, date: &str, minor: i64, tag: &str) {
    let line = |account: &str, side: &str| SerializableLine {
        account_number: account.to_owned(),
        side: side.to_owned(),
        base: SerializableAmount {
            minor: minor.to_string(),
            currency: SGD.to_owned(),
        },
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    let input = PostEntryInput {
        journal_id: format!("gl-{company_id}"),
        fiscal_year: 2026,
        entry: SerializableEntryDraft {
            journal_id: format!("gl-{company_id}"),
            document_date: date.to_owned(),
            posting_date: date.to_owned(),
            tax_point_date: None,
            base_currency: SGD.to_owned(),
            description: format!("Sales {tag}"),
            source_documents: Vec::new(),
            external_id: Some(format!("rev-{company_id}-{tag}")),
            source_app: Some("test".to_owned()),
            reverses: None,
            lines: vec![line("1020", "debit"), line("4010", "credit")],
        },
    };
    run(db, company_id, &PostEntry, &format!("rev-{tag}"), &input)
        .await
        .expect("revenue posts");
}

fn accounts(royalty: &str, ic: &str, wht: Option<&str>) -> AccountsInput {
    AccountsInput {
        royalty_account: royalty.to_owned(),
        ic_account: ic.to_owned(),
        wht_account: wht.map(str::to_owned),
        bank_account: "1020".to_owned(),
        unit: None,
    }
}

/// The group with the two companies, through the command.
async fn group(db: &dyn Database) {
    let input = GroupsCreateInput {
        id: GROUP.to_owned(),
        name: "Factory Zero Group (test)".to_owned(),
        members: vec![
            MemberInput {
                company_id: IP.to_owned(),
                kind: GroupKind::IpHolding,
                ownership_bp: 10_000,
            },
            MemberInput {
                company_id: OPS.to_owned(),
                kind: GroupKind::Operating,
                ownership_bp: 10_000,
            },
        ],
        created_by: OWNER.to_owned(),
    };
    run(db, OPS, &GroupsCreate, "group-create", &input)
        .await
        .expect("group creates");
}

/// A 5%-of-revenue licence between the two companies, both SGD and both in SG.
fn licence_input(id: &str) -> AgreementsCreateInput {
    AgreementsCreateInput {
        id: id.to_owned(),
        group_id: GROUP.to_owned(),
        licensor_company_id: IP.to_owned(),
        licensee_company_id: OPS.to_owned(),
        kind: Kind::Licence,
        basis: Basis::PctOfRevenueBp,
        rate_bp: Some(500),
        fixed_minor: None,
        currency: SGD.to_owned(),
        treaty_override_bp: None,
        notes: Some("Licence of the Factory Zero software (test)".to_owned()),
        licensor: accounts("4060", "1080", None),
        licensee: accounts("5120", "2060", None),
        created_by: OWNER.to_owned(),
    }
}

async fn world(db: &dyn Database) {
    company(db, OPS, "Factory Zero Pte. Ltd. (test)", "sg").await;
    company(db, IP, "Factory Zero IP Pte. Ltd. (test)", "sg").await;
    company(db, OTHER, "Unrelated Pte. Ltd. (test)", "sg").await;
    group(db).await;
    run(
        db,
        OPS,
        &AgreementsCreate,
        "agreement-create",
        &licence_input(AGREEMENT),
    )
    .await
    .expect("agreement creates");
}

/// Sides that post through the executor, optionally failing one company.
struct Sides<'a> {
    db: &'a dyn Database,
    fail: Option<&'a str>,
}

#[async_trait]
impl SideExecutor for Sides<'_> {
    async fn post(&self, company_id: &str, input: &PostEntryInput) -> Result<String, String> {
        if self.fail == Some(company_id) {
            return Err("injected failure on this side".to_owned());
        }
        let request = format!("side-post-{}", EVENTS.fetch_add(1, Ordering::SeqCst));
        let out: PostEntryOutput = run(self.db, company_id, &PostEntry, &request, input)
            .await
            .map_err(|err| err.to_string())?;
        Ok(out.entry.entry_id)
    }

    async fn reverse(&self, company_id: &str, external: &str, date: &str) -> Result<(), String> {
        let original = ledgers_ledger::query::entry_by_external_id(self.db, company_id, external)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("no entry {external}"))?;
        let mirror: Vec<SerializableLine> = original
            .lines
            .iter()
            .map(|l| SerializableLine {
                account_number: l.account_number.clone(),
                side: if l.side == ledgers_ledger::Side::Debit {
                    "credit"
                } else {
                    "debit"
                }
                .to_owned(),
                base: SerializableAmount {
                    minor: l.base.minor().to_string(),
                    currency: SGD.to_owned(),
                },
                fx: None,
                tax_code: None,
                dimensions: BTreeMap::new(),
                memo: None,
            })
            .collect();
        let input = PostEntryInput {
            journal_id: format!("gl-{company_id}"),
            fiscal_year: 2026,
            entry: SerializableEntryDraft {
                journal_id: format!("gl-{company_id}"),
                document_date: date.to_owned(),
                posting_date: date.to_owned(),
                tax_point_date: None,
                base_currency: SGD.to_owned(),
                description: "Reversal".to_owned(),
                source_documents: Vec::new(),
                external_id: None,
                source_app: Some("test".to_owned()),
                reverses: Some(original.id.clone()),
                lines: mirror,
            },
        };
        let request = format!("side-reverse-{}", EVENTS.fetch_add(1, Ordering::SeqCst));
        run(self.db, company_id, &PostEntry, &request, &input)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

#[allow(dead_code)]
fn periods(list: &[&str]) -> PeriodsInput {
    PeriodsInput {
        id: AGREEMENT.to_owned(),
        periods: list.iter().map(|p| (*p).to_owned()).collect(),
    }
}

async fn reconciliation(db: &dyn Database) -> ReconciliationOutput {
    let report = ReconciliationReport {
        accessible: vec![OPS.to_owned(), IP.to_owned()],
    };
    run(
        db,
        OPS,
        &report,
        "recon",
        &ReconciliationInput {
            id: GROUP.to_owned(),
        },
    )
    .await
    .expect("reconciliation")
}

async fn doc_statuses(db: &dyn Database, period: &str) -> Vec<(String, String, i64)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT side, status, gross_minor FROM intercompany_document WHERE agreement_id = ? AND period = ? ORDER BY side"
                .to_owned(),
            vec![Value::from(AGREEMENT), Value::from(period)],
        ))
        .await
        .expect("docs");
    rows.rows
        .iter()
        .map(|r| {
            (
                r.get::<String>("side").unwrap(),
                r.get::<String>("status").unwrap(),
                r.get::<i64>("gross_minor").unwrap(),
            )
        })
        .collect()
}

async fn ledger_count(db: &dyn Database, sql: &str) -> i64 {
    let rows = db
        .query(&Statement::new(sql.to_owned()))
        .await
        .expect("count");
    rows.rows
        .first()
        .and_then(|r| r.get::<i64>("n"))
        .unwrap_or(0)
}

/// Each scenario, once per dialect the harness offers.
fn each_dialect(
    scenario: impl Fn(&dyn Database) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>>,
) {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(scenario(&*kit.db));
    }
}

#[test]
fn run_period_drafts_both_sides_once_and_a_second_run_adds_nothing() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            let first = run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-1",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-01".to_owned()],
                },
            )
            .await
            .expect("run");
            let second = run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-2",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-01".to_owned()],
                },
            )
            .await
            .expect("second run");
            assert_eq!(first.periods[0].docs[0].gross_minor, 50_000);
            assert_eq!(first.periods[0].docs[1].gross_minor, 50_000);
            assert_eq!(
                first.periods, second.periods,
                "a second run keeps the drafts"
            );
            assert_eq!(doc_statuses(db, "2026-01").await.len(), 2);
            assert_eq!(
                ledger_count(db, "SELECT COUNT(*) AS n FROM intercompany_document").await,
                2
            );
        })
    });
}

#[test]
fn post_then_settle_clears_the_books_and_the_reconciliation_shows_the_open_month() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            revenue(db, OPS, "2026-02-28", 2_000_000, "feb").await;
            for period in ["2026-01", "2026-02"] {
                run(
                    db,
                    OPS,
                    &AgreementsRunPeriod,
                    &format!("run-{period}"),
                    &RunPeriodInput {
                        id: AGREEMENT.to_owned(),
                        periods: vec![period.to_owned()],
                    },
                )
                .await
                .expect("run");
            }
            let sides = Sides { db, fail: None };
            let posted = orchestrate::post_period(db, &sides, AGREEMENT, "2026-01", "call-p1", NOW)
                .await
                .expect("post jan");
            assert!(
                posted
                    .docs
                    .iter()
                    .all(|d| d.status == ledgers_group::model::DocStatus::Posted)
            );
            orchestrate::post_period(db, &sides, AGREEMENT, "2026-02", "call-p2", NOW)
                .await
                .expect("post feb");

            let before = reconciliation(db).await;
            assert_eq!(before.pairs.len(), 1);
            assert_eq!(before.pairs[0].receivable_minor, 150_000);
            assert_eq!(before.pairs[0].payable_minor, 150_000);
            assert_eq!(before.pairs[0].income_minor, 150_000);
            assert_eq!(before.pairs[0].expense_minor, 150_000);
            assert_eq!(before.pairs[0].status, "balanced");

            let settled =
                orchestrate::settle_period(db, &sides, AGREEMENT, "2026-01", "call-s1", NOW)
                    .await
                    .expect("settle jan");
            assert!(
                settled
                    .docs
                    .iter()
                    .all(|d| d.status == ledgers_group::model::DocStatus::Settled)
            );

            let after = reconciliation(db).await;
            let pair = &after.pairs[0];
            assert_eq!(
                pair.receivable_minor, 100_000,
                "only February is still owed"
            );
            assert_eq!(pair.payable_minor, 100_000);
            assert_eq!(pair.balance_difference_minor, 0);
            assert_eq!(pair.income_difference_minor, 0);
            assert_eq!(pair.status, "balanced");
            assert_eq!(pair.open_periods, vec!["2026-02".to_owned()]);

            // Settling again changes nothing: the period is already settled.
            let again =
                orchestrate::settle_period(db, &sides, AGREEMENT, "2026-01", "call-s2", NOW)
                    .await
                    .expect("settle is idempotent");
            assert_eq!(again.docs, settled.docs);
        })
    });
}

#[test]
fn a_failure_on_the_second_side_reverses_the_first_and_fails_both() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-jan",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-01".to_owned()],
                },
            )
            .await
            .expect("run");

            // The licensee (side B) refuses. The licensor (side A) posted first.
            let broken = Sides {
                db,
                fail: Some(OPS),
            };
            let err = orchestrate::post_period(db, &broken, AGREEMENT, "2026-01", "call-f1", NOW)
                .await
                .expect_err("side B fails");
            assert!(
                err.to_string().contains("licensee") || err.to_string().contains("licensor"),
                "{err}"
            );
            let statuses = doc_statuses(db, "2026-01").await;
            assert!(
                statuses.iter().all(|(_, status, _)| status == "failed"),
                "{statuses:?}"
            );

            // Neither side is left owing: side A's entry is in the books and reversed.
            let mid = reconciliation(db).await;
            assert_eq!(
                mid.pairs[0].receivable_minor, 0,
                "the licensor's entry was reversed"
            );
            assert_eq!(mid.pairs[0].payable_minor, 0);
            assert_eq!(mid.pairs[0].balance_difference_minor, 0);
            let posted_entries = ledger_count(
                db,
                "SELECT COUNT(*) AS n FROM journal_entry WHERE external_id LIKE 'ic-%'",
            )
            .await;
            assert_eq!(
                posted_entries, 1,
                "only the failed attempt's entry exists, reversed"
            );

            // A healthy retry starts again with a new attempt and posts both sides.
            let healthy = Sides { db, fail: None };
            let retried =
                orchestrate::post_period(db, &healthy, AGREEMENT, "2026-01", "call-f2", NOW)
                    .await
                    .expect("retry posts");
            assert!(
                retried
                    .docs
                    .iter()
                    .all(|d| d.status == ledgers_group::model::DocStatus::Posted)
            );
            let done = reconciliation(db).await;
            assert_eq!(done.pairs[0].receivable_minor, 50_000);
            assert_eq!(done.pairs[0].payable_minor, 50_000);
            assert_eq!(done.pairs[0].status, "balanced");
        })
    });
}

#[test]
fn withholding_is_taken_off_the_payable_and_the_licensor_holds_it_as_a_receivable() {
    each_dialect(|db| {
        Box::pin(async move {
            // Licensee in the Netherlands, licensor in Germany: both EUR, so the
            // residence rule sees two different countries and a treaty rate applies.
            company(db, OPS, "Factory Zero NL (test)", "nl").await;
            company(db, IP, "Factory Zero DE (test)", "de").await;
            for (id, currency) in [(OPS, "EUR"), (IP, "EUR")] {
                exec(
                    db,
                    "UPDATE company SET base_currency = ? WHERE id = ?",
                    vec![Value::from(currency), Value::from(id)],
                )
                .await;
            }
            group(db).await;
            let mut input = licence_input(AGREEMENT);
            input.currency = "EUR".to_owned();
            input.treaty_override_bp = Some(1000);
            input.licensor = accounts("4060", "1080", Some("1090"));
            input.licensee = accounts("5120", "2060", Some("2070"));
            let created = run(db, OPS, &AgreementsCreate, "agreement-eur", &input)
                .await
                .expect("agreement with withholding");
            assert_eq!(created.licensee.wht_bp, 1000);
            assert_eq!(created.licensee.wht_basis, WhtBasis::TreatyOverride);

            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-wht",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-01".to_owned()],
                },
            )
            .await
            .expect("run");
            let sides = Sides { db, fail: None };
            orchestrate::post_period(db, &sides, AGREEMENT, "2026-01", "call-w1", NOW)
                .await
                .expect("post");
            let licensee_draft = db
                .query(&Statement::with_values(
                    "SELECT gross_minor, wht_minor, net_minor FROM intercompany_document WHERE agreement_id = ? AND side = 'licensee'"
                        .to_owned(),
                    vec![Value::from(AGREEMENT)],
                ))
                .await
                .expect("doc");
            let row = licensee_draft.rows.first().expect("row");
            assert_eq!(row.get::<i64>("gross_minor"), Some(50_000));
            assert_eq!(row.get::<i64>("wht_minor"), Some(5_000));
            assert_eq!(row.get::<i64>("net_minor"), Some(45_000));

            // The licensee's payable is the net; its withholding payable holds the tax.
            let r = reconciliation(db).await;
            assert_eq!(
                r.pairs[0].payable_minor, 50_000,
                "net payable plus withholding payable"
            );
            assert_eq!(
                r.pairs[0].receivable_minor, 50_000,
                "gross receivable plus withholding receivable"
            );

            orchestrate::settle_period(db, &sides, AGREEMENT, "2026-01", "call-w2", NOW)
                .await
                .expect("settle");
            let settled = reconciliation(db).await;
            assert_eq!(settled.pairs[0].balance_difference_minor, 0);
            assert_eq!(
                settled.pairs[0].receivable_minor, 5_000,
                "the tax stays with the tax office, as a receivable"
            );
        })
    });
}

#[test]
fn a_zero_period_settles_without_posting_anything() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-zero",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-03".to_owned()],
                },
            )
            .await
            .expect("run");
            let sides = Sides { db, fail: None };
            let out = orchestrate::post_period(db, &sides, AGREEMENT, "2026-03", "call-z", NOW)
                .await
                .expect("zero post");
            assert!(
                out.docs
                    .iter()
                    .all(|d| d.status == ledgers_group::model::DocStatus::Settled)
            );
            assert!(out.docs.iter().all(|d| d.accrual_entry_id.is_none()));
            assert_eq!(
                ledger_count(
                    db,
                    "SELECT COUNT(*) AS n FROM journal_entry WHERE external_id LIKE 'ic-%'"
                )
                .await,
                0
            );
        })
    });
}

#[test]
fn an_agreement_in_another_currency_is_refused_with_the_reason() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let mut input = licence_input("01ICAGREEMENT00000000000B");
            input.currency = "EUR".to_owned();
            let err = run(db, OPS, &AgreementsCreate, "eur", &input)
                .await
                .expect_err("refused");
            let text = err.to_string();
            assert!(
                text.contains("one currency") && text.contains("TODO"),
                "{text}"
            );
        })
    });
}

#[test]
fn a_company_outside_the_group_cannot_be_a_party() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let mut input = licence_input("01ICAGREEMENT00000000000C");
            input.licensor_company_id = OTHER.to_owned();
            let err = run(db, OPS, &AgreementsCreate, "outsider", &input)
                .await
                .expect_err("refused");
            assert!(err.to_string().contains("not a member"), "{err}");
        })
    });
}

#[test]
fn the_group_view_shows_only_the_companies_it_is_given() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let view: GroupViewOutput = run(
                db,
                IP,
                &GroupViewReport {
                    accessible: vec![IP.to_owned()],
                },
                "view",
                &GroupViewInput {
                    id: GROUP.to_owned(),
                    period: "2026-01".to_owned(),
                    as_of: "2026-01-31".to_owned(),
                },
            )
            .await
            .expect("view");
            assert_eq!(view.members.len(), 1);
            assert_eq!(view.members[0].company.id, IP);
            assert!(view.consolidated.contains("Ledgers Cloud"));
        })
    });
}

#[test]
fn a_person_who_sees_only_one_company_is_not_shown_the_other_in_lists() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let list = run(
                db,
                IP,
                &GroupsList {
                    accessible: vec![IP.to_owned()],
                },
                "list",
                &GroupsListInput {},
            )
            .await
            .expect("list");
            assert_eq!(list.groups.len(), 1);
            assert_eq!(
                list.groups[0].members.len(),
                1,
                "the other company is hidden"
            );
            assert!(list.groups[0].has_hidden_members);
            assert!(!format!("{:?}", list.groups[0]).contains(OPS));
        })
    });
}

#[test]
fn the_agreement_read_shows_the_own_side_and_hides_an_unseen_counterparty() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let read: AgreementsGetOutput = run(
                db,
                IP,
                &AgreementsGet {
                    accessible: vec![IP.to_owned()],
                },
                "get",
                &AgreementIdInput {
                    id: AGREEMENT.to_owned(),
                },
            )
            .await
            .expect("get");
            assert_eq!(read.agreement.side, Side::Licensor);
            assert_eq!(read.agreement.counterparty_id, None);
            assert!(read.agreement.counterparty_hidden);
            assert_eq!(read.agreement.accounts.royalty, "4060");
        })
    });
}

#[test]
fn terminated_agreements_run_no_more_periods() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            run(
                db,
                OPS,
                &AgreementsTerminate {
                    accessible: vec![OPS.to_owned(), IP.to_owned()],
                },
                "terminate",
                &AgreementIdInput {
                    id: AGREEMENT.to_owned(),
                },
            )
            .await
            .expect("terminate");
            let err = run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-after",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-04".to_owned()],
                },
            )
            .await
            .expect_err("refused");
            assert!(err.to_string().contains("terminated"), "{err}");
        })
    });
}

/// Counts every query the wrapped database answers.
struct Counting<'a> {
    inner: &'a dyn Database,
    queries: Arc<AtomicUsize>,
}

#[async_trait]
impl Database for Counting<'_> {
    async fn execute(&self, stmt: &Statement) -> Result<u64, DbError> {
        self.inner.execute(stmt).await
    }

    async fn query(&self, stmt: &Statement) -> Result<Rows, DbError> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        self.inner.query(stmt).await
    }

    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), DbError> {
        self.inner.batch_atomic(stmts).await
    }
}

#[test]
fn the_reconciliation_reads_a_constant_number_of_rows_however_many_agreements_there_are() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            let first = Arc::new(AtomicUsize::new(0));
            let counted = Counting {
                inner: db,
                queries: first.clone(),
            };
            let report = ReconciliationReport {
                accessible: vec![OPS.to_owned(), IP.to_owned()],
            };
            let executor = Executor::new(&counted, Auth::owner(), DefaultPolicy);
            executor
                .run(
                    &report,
                    TOKEN.to_owned(),
                    ctx(OPS, "count-1"),
                    &ReconciliationInput {
                        id: GROUP.to_owned(),
                    },
                    Execute::Run,
                )
                .await
                .expect("reconciliation");
            let one = first.load(Ordering::SeqCst);

            for extra in ["01ICAGREEMENT00000000000D", "01ICAGREEMENT00000000000E"] {
                run(db, OPS, &AgreementsCreate, extra, &licence_input(extra))
                    .await
                    .expect("another agreement");
            }
            let second = Arc::new(AtomicUsize::new(0));
            let counted = Counting {
                inner: db,
                queries: second.clone(),
            };
            let executor = Executor::new(&counted, Auth::owner(), DefaultPolicy);
            let out = executor
                .run(
                    &report,
                    TOKEN.to_owned(),
                    ctx(OPS, "count-3"),
                    &ReconciliationInput {
                        id: GROUP.to_owned(),
                    },
                    Execute::Run,
                )
                .await
                .expect("reconciliation");
            let three = second.load(Ordering::SeqCst);
            match out {
                Executed::Done(out) => assert_eq!(out.pairs.len(), 3),
                _ => panic!("expected output"),
            }
            assert_eq!(one, three, "the query count does not grow with agreements");
        })
    });
}

#[test]
fn the_group_list_reads_a_constant_number_of_rows_however_many_groups_there_are() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            let queries = Arc::new(AtomicUsize::new(0));
            let counted = Counting {
                inner: db,
                queries: queries.clone(),
            };
            let list = GroupsList {
                accessible: vec![IP.to_owned(), OPS.to_owned()],
            };
            let executor = Executor::new(&counted, Auth::owner(), DefaultPolicy);
            executor
                .run(
                    &list,
                    TOKEN.to_owned(),
                    ctx(OPS, "list-1"),
                    &GroupsListInput {},
                    Execute::Run,
                )
                .await
                .expect("list");
            let one = queries.swap(0, Ordering::SeqCst);

            for extra in ["01ICGROUP000000000000000H", "01ICGROUP000000000000000J"] {
                let input = GroupsCreateInput {
                    id: extra.to_owned(),
                    name: format!("Second group {extra}"),
                    members: vec![
                        MemberInput {
                            company_id: IP.to_owned(),
                            kind: GroupKind::IpHolding,
                            ownership_bp: 10_000,
                        },
                        MemberInput {
                            company_id: OPS.to_owned(),
                            kind: GroupKind::Operating,
                            ownership_bp: 10_000,
                        },
                    ],
                    created_by: OWNER.to_owned(),
                };
                run(db, OPS, &GroupsCreate, extra, &input)
                    .await
                    .expect("group");
            }
            let executor = Executor::new(&counted, Auth::owner(), DefaultPolicy);
            let out = executor
                .run(
                    &list,
                    TOKEN.to_owned(),
                    ctx(OPS, "list-3"),
                    &GroupsListInput {},
                    Execute::Run,
                )
                .await
                .expect("list");
            let three = queries.load(Ordering::SeqCst);
            match out {
                Executed::Done(out) => assert_eq!(out.groups.len(), 3),
                _ => panic!("expected output"),
            }
            assert_eq!(one, three, "one query per list, not one per group");
        })
    });
}

#[test]
fn an_external_id_names_its_agreement_side_step_and_attempt() {
    assert_eq!(
        external_id(AGREEMENT, "2026-01", Side::Licensee, Step::Settle, "call"),
        format!("ic-{AGREEMENT}-2026-01-licensee-settle-call")
    );
}

#[test]
fn the_withholding_rule_reads_the_packs_residence_and_rate() {
    let sg = ledgers_tax::pack_by_id("sg").expect("sg pack");
    let id = ledgers_tax::pack_by_id("id").expect("id pack");
    let nl = ledgers_tax::pack_by_id("nl").expect("nl pack");
    let de = ledgers_tax::pack_by_id("de").expect("de pack");

    // Singapore pays a non-resident licensor: the Singapore royalty rate, 10%.
    assert_eq!(
        ledgers_group::model::withholding_for(Kind::Licence, Some(&sg), Some(&id), None),
        (1000, WhtBasis::Pack)
    );
    // Indonesia pays a foreign licensor: PPh 26, 20%.
    assert_eq!(
        ledgers_group::model::withholding_for(Kind::Royalty, Some(&id), Some(&sg), None),
        (2000, WhtBasis::Pack)
    );
    // Both in Singapore: the payee is resident, so nothing is withheld.
    assert_eq!(
        ledgers_group::model::withholding_for(Kind::Licence, Some(&sg), Some(&sg), Some(1000)),
        (0, WhtBasis::Resident)
    );
    // No verified rate for the payer: none is applied, and that is said.
    assert_eq!(
        ledgers_group::model::withholding_for(Kind::Licence, Some(&nl), Some(&de), None),
        (0, WhtBasis::NoRateOnFile)
    );
    // A management fee carries no withholding here.
    assert_eq!(
        ledgers_group::model::withholding_for(Kind::ManagementFee, Some(&sg), Some(&id), None),
        (0, WhtBasis::NotApplicable)
    );
    // The NL innovatiebox note is sourced and verified.
    assert!(
        nl.notes.iter().any(|n| n.topic == "innovatiebox"
            && n.verified
            && n.source.contains("belastingdienst"))
    );
}

#[test]
fn a_posted_period_refuses_a_change_of_accounts() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            revenue(db, OPS, "2026-01-31", 1_000_000, "jan").await;
            run(
                db,
                OPS,
                &AgreementsRunPeriod,
                "run-acc",
                &RunPeriodInput {
                    id: AGREEMENT.to_owned(),
                    periods: vec!["2026-01".to_owned()],
                },
            )
            .await
            .expect("run");
            orchestrate::post_period(
                db,
                &Sides { db, fail: None },
                AGREEMENT,
                "2026-01",
                "call-acc",
                NOW,
            )
            .await
            .expect("post");
            let update = ledgers_group::commands::AgreementsUpdateInput {
                id: AGREEMENT.to_owned(),
                kind: Kind::Licence,
                basis: Basis::PctOfRevenueBp,
                rate_bp: Some(500),
                fixed_minor: None,
                currency: SGD.to_owned(),
                treaty_override_bp: None,
                notes: None,
                licensor: accounts("4060", "1090", None),
                licensee: accounts("5120", "2060", None),
            };
            let err = run(
                db,
                OPS,
                &ledgers_group::commands::AgreementsUpdate {
                    accessible: vec![OPS.to_owned(), IP.to_owned()],
                },
                "upd",
                &update,
            )
            .await
            .expect_err("refused");
            assert!(err.to_string().contains("already posted"), "{err}");
        })
    });
}

#[test]
fn periods_are_validated_before_anything_runs() {
    let err =
        ledgers_group::commands::check_periods(&["2026-13".to_owned()]).expect_err("bad month");
    assert!(err.to_string().contains("YYYY-MM"));
    let err = ledgers_group::commands::check_periods(&["2026-01".to_owned(), "2026-01".to_owned()])
        .expect_err("repeat");
    assert!(err.to_string().contains("twice"));
    assert!(ledgers_group::commands::check_periods(&[]).is_err());
    assert_eq!(
        orchestrate::intercompany_id(AGREEMENT, "2026-01"),
        format!("IC-{AGREEMENT}-2026-01")
    );
}

#[test]
fn an_agreement_names_accounts_of_the_right_type_and_only_live_postable_ones() {
    each_dialect(|db| {
        Box::pin(async move {
            world(db).await;
            // The licensee's royalty must be an expense; 1020 is the bank (an asset).
            let mut wrong_type = licence_input("01ICAGREEMENT00000000000F");
            wrong_type.licensee = accounts("1020", "2060", None);
            let err = run(db, OPS, &AgreementsCreate, "wrong-type", &wrong_type)
                .await
                .expect_err("refused");
            assert!(err.to_string().contains("must be a expense account") || err.to_string().contains("expense"), "{err}");

            // An account the chart does not have.
            let mut missing = licence_input("01ICAGREEMENT00000000000G");
            missing.licensor = accounts("4060", "1999", None);
            let err = run(db, OPS, &AgreementsCreate, "missing", &missing)
                .await
                .expect_err("refused");
            assert!(err.to_string().contains("1999 does not exist"), "{err}");
        })
    });
}
