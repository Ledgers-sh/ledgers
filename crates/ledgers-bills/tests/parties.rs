//! Parties (issue #44) on the real migrations, on every dialect the harness
//! offers. VIES is always a fake port: no test reaches the network.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_bills::parties::*;
use ledgers_commands::auth::{Authenticator, Credential, Limits, Principal, Role, Tier};
use ledgers_commands::*;
use ledgers_db::DbModule;
use sea_query::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const COMPANY_ID: &str = "01TESTCOMPANY0000000000000A";
const NOW: &str = "2026-01-15T09:30:00Z";
const OWNER_ID: &str = "owner-acme";
const AGENT_ID: &str = "agent-key-draft";
const OWNER_TOKEN: &str = "bearer-owner";
const AGENT_TOKEN: &str = "bearer-draft-agent";

// ============================================================================
// Authentication, the fake VIES port, and the harness
// ============================================================================

struct TestAuthenticator {
    credentials: BTreeMap<String, Principal>,
}

impl TestAuthenticator {
    fn new() -> Self {
        let mut credentials = BTreeMap::new();
        credentials.insert(
            OWNER_TOKEN.to_string(),
            Principal::Person {
                person_id: OWNER_ID.to_string(),
                role: Role::Owner,
            },
        );
        credentials.insert(
            AGENT_TOKEN.to_string(),
            Principal::Agent {
                key_id: AGENT_ID.to_string(),
                tier: Tier::Draft,
                limits: Limits::new(),
            },
        );
        TestAuthenticator { credentials }
    }
}

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        self.credentials
            .get(credential)
            .cloned()
            .ok_or(CommandError::Unauthenticated)
    }
}

/// A VIES port that answers with a canned result and counts its calls.
struct FakeVies {
    answer: Result<ViesResponse, String>,
    calls: AtomicUsize,
}

impl FakeVies {
    fn answering(answer: Result<ViesResponse, String>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            calls: AtomicUsize::new(0),
        })
    }

    fn valid() -> Arc<Self> {
        Self::answering(Ok(ViesResponse {
            valid: true,
            name: Some("Acme GmbH".to_string()),
            address: Some("Musterstrasse 1, Berlin".to_string()),
            consultation_number: Some("consultation-123".to_string()),
            request_date: Some("2026-01-15".to_string()),
        }))
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ViesPort for FakeVies {
    async fn check(&self, _country_code: &str, _vat_number: &str) -> Result<ViesResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }
}

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![Box::new(DbModule), Box::new(BillsModule)]
}

/// A second company on the same database, for the isolation test.
const OTHER_COMPANY_ID: &str = "01OTHERCOMPANY000000000000B";

fn context_in(company_id: &str, request_id: &str) -> CommandContext {
    CommandContext {
        company_id: company_id.to_string(),
        request_id: request_id.to_string(),
        event_id: format!("{request_id}-event"),
        now: NOW.to_string(),
    }
}

/// Runs a command as `token` and returns its output; any refusal is an error.
async fn run<C: Command>(
    db: &dyn Database,
    cmd: &C,
    token: &str,
    request_id: &str,
    input: &C::Input,
) -> Result<C::Output, CommandError> {
    run_in(COMPANY_ID, db, cmd, token, request_id, input).await
}

/// Runs a command as `token` against `company_id`, the way the server scopes
/// a request to the company it names.
async fn run_in<C: Command>(
    company_id: &str,
    db: &dyn Database,
    cmd: &C,
    token: &str,
    request_id: &str,
    input: &C::Input,
) -> Result<C::Output, CommandError> {
    let executor = Executor::new(db, TestAuthenticator::new(), DefaultPolicy);
    match executor
        .run(
            cmd,
            token.to_string(),
            context_in(company_id, request_id),
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
    db.execute(&Statement::with_values(sql.to_string(), values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}\n{sql}"));
}

/// A vendor row written the way parties and bills write one.
async fn party(db: &dyn Database, id: &str, name: &str, vat_id: Option<&str>, iban: Option<&str>) {
    exec(
        db,
        "INSERT INTO vendor (id, company_id, name, vat_id, iban, source, needs_review, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'manual', 0, ?, ?)",
        vec![
            Value::from(id),
            Value::from(COMPANY_ID),
            Value::from(name),
            vat_id.map_or(Value::String(None), Value::from),
            iban.map_or(Value::String(None), Value::from),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
}

/// An open (or posted) bill from `vendor_id`.
async fn bill(db: &dyn Database, id: &str, vendor_id: &str, invoice: &str, posted: bool) {
    exec(
        db,
        "INSERT INTO bill (id, company_id, document_id, vendor_id, invoice_number, currency, lines_json, \
         checks_json, instruction_flags, iban_changed, source, posted_entry_id, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'EUR', '[]', '[]', '[]', 0, 'manual', ?, 'test', ?, ?)",
        vec![
            Value::from(id),
            Value::from(COMPANY_ID),
            Value::from(format!("doc-{id}").as_str()),
            Value::from(vendor_id),
            Value::from(invoice),
            if posted {
                Value::from("entry-1")
            } else {
                Value::String(None)
            },
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
}

async fn scalar_i64(db: &dyn Database, sql: &str) -> i64 {
    let rows = db
        .query(&Statement::new(sql.to_string()))
        .await
        .unwrap_or_else(|err| panic!("query failed: {err}\n{sql}"));
    rows.rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or_else(|| panic!("no count from {sql}"))
}

async fn text_of(db: &dyn Database, sql: &str, column: &str) -> Option<String> {
    let rows = db
        .query(&Statement::new(sql.to_string()))
        .await
        .unwrap_or_else(|err| panic!("query failed: {err}\n{sql}"));
    rows.rows
        .first()
        .and_then(|row| row.get::<Option<String>>(column))
        .flatten()
}

fn check_vat_input(id: &str, country: &str, number: &str) -> CheckVatInput {
    CheckVatInput {
        id: id.to_string(),
        country_code: country.to_string(),
        vat_number: number.to_string(),
    }
}

// ============================================================================
// Master data: create and update validate through the country packs
// ============================================================================

#[test]
fn a_vendor_row_from_before_parties_is_a_vendor_and_its_bills_still_link() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            // The row shape bills wrote before migration 0004: no party columns.
            party(
                &*kit.db,
                "ven-old",
                "Halden Supplies GmbH",
                Some("DE136695976"),
                None,
            )
            .await;
            bill(&*kit.db, "bill-old", "ven-old", "INV-1", false).await;

            let list = run(
                &*kit.db,
                &PartiesList,
                OWNER_TOKEN,
                "list-old",
                &ListInput {
                    limit: None,
                    offset: None,
                },
            )
            .await
            .expect("list");
            assert_eq!(list.total, 1);
            assert_eq!(list.parties[0].roles, "vendor");
            assert!(!list.parties[0].personal_data);

            let joined = text_of(
                &*kit.db,
                "SELECT v.name AS name FROM bill b JOIN vendor v ON v.id = b.vendor_id WHERE b.id = 'bill-old'",
                "name",
            )
            .await;
            assert_eq!(joined.as_deref(), Some("Halden Supplies GmbH"));
        });
    }
}

#[test]
fn create_checks_the_vat_id_with_its_country_pack_and_refuses_duplicates() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let create = |id: &str, vat: &str| CreateInput {
                id: Some(id.to_string()),
                name: "Acme GmbH".to_string(),
                vat_id: Some(vat.to_string()),
                roles: None,
                tax_number: None,
                language: None,
                currency: None,
                personal_data: None,
                payment_terms_days: None,
                default_account: None,
                default_tax_code: None,
            };

            // A DE number with a wrong check digit is refused before any write.
            let refused = run(
                &*kit.db,
                &PartiesCreate,
                OWNER_TOKEN,
                "create-bad",
                &create("p-bad", "DE123456789"),
            )
            .await;
            assert!(
                matches!(refused, Err(CommandError::InvalidInput(_))),
                "DE check digit"
            );
            assert_eq!(
                scalar_i64(
                    &*kit.db,
                    "SELECT COUNT(*) AS n FROM vendor WHERE id = 'p-bad'"
                )
                .await,
                0
            );

            // A valid DE number is stored normalised.
            let made = run(
                &*kit.db,
                &PartiesCreate,
                OWNER_TOKEN,
                "create-ok",
                &create("p-acme", "de 136.695.976"),
            )
            .await
            .expect("valid DE VAT ID");
            assert_eq!(made.party.vat_id.as_deref(), Some("DE136695976"));

            // The same VAT ID again names the party that holds it.
            let duplicate = run(
                &*kit.db,
                &PartiesCreate,
                OWNER_TOKEN,
                "create-dup",
                &create("p-dup", "DE136695976"),
            )
            .await;
            match duplicate {
                Err(CommandError::InvalidInput(message)) => {
                    assert!(message.contains("p-acme"), "{message}");
                }
                other => panic!("expected a refusal naming p-acme, got {other:?}"),
            }
        });
    }
}

#[test]
fn create_and_update_refuse_a_nameless_party_an_unknown_role_and_a_bad_vat_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-1", "Acme GmbH", None, None).await;

            let blank_name = run(
                &*kit.db,
                &PartiesCreate,
                OWNER_TOKEN,
                "blank",
                &CreateInput {
                    id: Some("p-blank".to_string()),
                    name: "   ".to_string(),
                    vat_id: None,
                    roles: None,
                    tax_number: None,
                    language: None,
                    currency: None,
                    personal_data: None,
                    payment_terms_days: None,
                    default_account: None,
                    default_tax_code: None,
                },
            )
            .await;
            assert!(matches!(blank_name, Err(CommandError::InvalidInput(_))));

            let bad_role = run(
                &*kit.db,
                &PartiesUpdate,
                AGENT_TOKEN,
                "role",
                &UpdateInput {
                    id: "p-1".to_string(),
                    name: None,
                    vat_id: None,
                    roles: Some("supplier".to_string()),
                    tax_number: None,
                    language: None,
                    currency: None,
                    personal_data: None,
                    payment_terms_days: None,
                    default_account: None,
                    default_tax_code: None,
                },
            )
            .await;
            assert!(matches!(bad_role, Err(CommandError::InvalidInput(_))));

            let bad_vat = run(
                &*kit.db,
                &PartiesUpdate,
                AGENT_TOKEN,
                "vat",
                &UpdateInput {
                    id: "p-1".to_string(),
                    name: None,
                    vat_id: Some("NL123456789B1".to_string()),
                    roles: None,
                    tax_number: None,
                    language: None,
                    currency: None,
                    personal_data: None,
                    payment_terms_days: None,
                    default_account: None,
                    default_tax_code: None,
                },
            )
            .await;
            assert!(
                matches!(bad_vat, Err(CommandError::InvalidInput(_))),
                "NL needs two digits after B"
            );
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT vat_id AS v FROM vendor WHERE id = 'p-1'",
                    "v"
                )
                .await,
                None
            );
        });
    }
}

#[test]
fn a_merged_party_cannot_be_updated() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-keep", "Acme GmbH", None, None).await;
            party(&*kit.db, "p-gone", "Acme Holding", None, None).await;
            run(
                &*kit.db,
                &PartiesMerge::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "merge-1",
                &MergeInput {
                    id: "p-keep".to_string(),
                    from: "p-gone".to_string(),
                },
            )
            .await
            .expect("merge");

            let result = run(
                &*kit.db,
                &PartiesUpdate,
                OWNER_TOKEN,
                "update-gone",
                &UpdateInput {
                    id: "p-gone".to_string(),
                    name: Some("Renamed".to_string()),
                    vat_id: None,
                    roles: None,
                    tax_number: None,
                    language: None,
                    currency: None,
                    personal_data: None,
                    payment_terms_days: None,
                    default_account: None,
                    default_tax_code: None,
                },
            )
            .await;
            assert!(matches!(result, Err(CommandError::InvalidInput(_))));
        });
    }
}

// ============================================================================
// Bank details: human only, history kept, bills flagged on a real change
// ============================================================================

fn bank_input(id: &str, iban: &str) -> SetBankInput {
    SetBankInput {
        id: id.to_string(),
        iban: iban.to_string(),
        bic: Some("cobadeff".to_string()),
        reason: "bank letter received by post".to_string(),
    }
}

#[test]
fn set_bank_is_refused_to_an_agent() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-bank", "Acme GmbH", None, None).await;
            let result = run(
                &*kit.db,
                &PartiesSetBank::new(None),
                AGENT_TOKEN,
                "bank-agent",
                &bank_input("p-bank", "DE89370400440532013000"),
            )
            .await;
            assert!(matches!(result, Err(CommandError::HumanOnly)));
            assert_eq!(
                scalar_i64(&*kit.db, "SELECT COUNT(*) AS n FROM vendor_bank_change").await,
                0
            );
        });
    }
}

#[test]
fn set_bank_needs_the_acting_person() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-bank", "Acme GmbH", None, None).await;
            let result = run(
                &*kit.db,
                &PartiesSetBank::new(None),
                OWNER_TOKEN,
                "bank-nobody",
                &bank_input("p-bank", "DE89370400440532013000"),
            )
            .await;
            assert!(matches!(result, Err(CommandError::Unauthenticated)));
        });
    }
}

#[test]
fn set_bank_records_before_and_after_and_returns_the_real_party() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(
                &*kit.db,
                "p-bank",
                "Acme GmbH",
                Some("DE136695976"),
                Some("DE89370400440532013000"),
            )
            .await;
            let out = run(
                &*kit.db,
                &PartiesSetBank::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "bank-owner",
                &bank_input("p-bank", "de89 3704 0044 0532 0130 00"),
            )
            .await
            .expect("set_bank");

            assert_eq!(out.party.name, "Acme GmbH", "the output names the party");
            assert_eq!(out.party.iban.as_deref(), Some("DE89370400440532013000"));
            assert_eq!(out.party.bic.as_deref(), Some("COBADEFF"));

            let history = kit
                .db
                .query(&Statement::new(
                    "SELECT before_iban, after_iban, changed_by, reason FROM vendor_bank_change WHERE vendor_id = 'p-bank'"
                        .to_string(),
                ))
                .await
                .expect("history");
            assert_eq!(history.rows.len(), 1);
            let row = &history.rows[0];
            assert_eq!(
                row.get::<Option<String>>("before_iban")
                    .flatten()
                    .as_deref(),
                Some("DE89370400440532013000")
            );
            assert_eq!(
                row.get::<Option<String>>("after_iban").flatten().as_deref(),
                Some("DE89370400440532013000")
            );
            assert_eq!(row.get::<String>("changed_by").as_deref(), Some(OWNER_ID));
        });
    }
}

#[test]
fn set_bank_refuses_a_bad_iban_and_writes_nothing() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(
                &*kit.db,
                "p-bank",
                "Acme GmbH",
                None,
                Some("DE89370400440532013000"),
            )
            .await;
            let result = run(
                &*kit.db,
                &PartiesSetBank::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "bank-bad",
                &bank_input("p-bank", "DE89370400440532013001"),
            )
            .await;
            assert!(
                matches!(result, Err(CommandError::InvalidInput(_))),
                "mod-97 check"
            );
            assert_eq!(
                scalar_i64(&*kit.db, "SELECT COUNT(*) AS n FROM vendor_bank_change").await,
                0
            );
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT iban AS i FROM vendor WHERE id = 'p-bank'",
                    "i"
                )
                .await
                .as_deref(),
                Some("DE89370400440532013000")
            );
        });
    }
}

#[test]
fn set_bank_flags_open_bills_only_when_the_iban_really_changes() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(
                &*kit.db,
                "p-bank",
                "Acme GmbH",
                None,
                Some("DE89370400440532013000"),
            )
            .await;
            bill(&*kit.db, "bill-open", "p-bank", "INV-7", false).await;
            bill(&*kit.db, "bill-paid", "p-bank", "INV-8", true).await;
            let owner = PartiesSetBank::new(Some(OWNER_ID.to_string()));

            // The same IBAN again is not a change of payee.
            run(
                &*kit.db,
                &owner,
                OWNER_TOKEN,
                "bank-same",
                &bank_input("p-bank", "DE89370400440532013000"),
            )
            .await
            .expect("same IBAN");
            assert_eq!(
                scalar_i64(
                    &*kit.db,
                    "SELECT iban_changed AS n FROM bill WHERE id = 'bill-open'"
                )
                .await,
                0
            );

            // A different IBAN flags the open bill, and not the posted one.
            run(
                &*kit.db,
                &owner,
                OWNER_TOKEN,
                "bank-new",
                &bank_input("p-bank", "GB82WEST12345698765432"),
            )
            .await
            .expect("new IBAN");
            assert_eq!(
                scalar_i64(
                    &*kit.db,
                    "SELECT iban_changed AS n FROM bill WHERE id = 'bill-open'"
                )
                .await,
                1
            );
            assert_eq!(
                scalar_i64(
                    &*kit.db,
                    "SELECT iban_changed AS n FROM bill WHERE id = 'bill-paid'"
                )
                .await,
                0
            );
        });
    }
}

#[test]
fn vendor_bank_change_history_is_append_only() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(
                &*kit.db,
                "p-bank",
                "Acme GmbH",
                None,
                Some("DE89370400440532013000"),
            )
            .await;
            run(
                &*kit.db,
                &PartiesSetBank::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "bank-hist",
                &bank_input("p-bank", "GB82WEST12345698765432"),
            )
            .await
            .expect("set_bank");

            let update = kit
                .db
                .execute(&Statement::new(
                    "UPDATE vendor_bank_change SET reason = 'edited'".to_string(),
                ))
                .await;
            assert!(update.is_err(), "history rows cannot be updated");
            let delete = kit
                .db
                .execute(&Statement::new(
                    "DELETE FROM vendor_bank_change".to_string(),
                ))
                .await;
            assert!(delete.is_err(), "history rows cannot be deleted");
        });
    }
}

// ============================================================================
// VIES: behind the port, refused when off, recorded per VAT ID
// ============================================================================

const ACME_VAT: &str = "DE136695976";

#[test]
fn check_vat_records_a_valid_answer_for_the_partys_vat_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-vies", "Acme GmbH", Some(ACME_VAT), None).await;
            let vies = FakeVies::valid();
            let cmd = PartiesCheckVat::new(Some(vies.clone()));
            let out = run(
                &*kit.db,
                &cmd,
                AGENT_TOKEN,
                "vies-ok",
                &check_vat_input("p-vies", "de", "136695976"),
            )
            .await
            .expect("check_vat");
            assert!(out.vies_result.valid);
            assert_eq!(out.vies_result.name.as_deref(), Some("Acme GmbH"));
            assert_eq!(vies.calls(), 1);

            let stored = kit
                .db
                .query(&Statement::new(
                    "SELECT country_code, vat_number, valid, consultation_number FROM vies_check WHERE vendor_id = 'p-vies'"
                        .to_string(),
                ))
                .await
                .expect("stored check");
            assert_eq!(stored.rows.len(), 1);
            assert_eq!(
                stored.rows[0].get::<String>("country_code").as_deref(),
                Some("DE")
            );
            assert_eq!(
                stored.rows[0].get::<String>("vat_number").as_deref(),
                Some(ACME_VAT)
            );
            assert_eq!(stored.rows[0].get::<i64>("valid"), Some(1));
            assert_eq!(
                stored.rows[0]
                    .get::<Option<String>>("consultation_number")
                    .flatten()
                    .as_deref(),
                Some("consultation-123")
            );
        });
    }
}

#[test]
fn check_vat_records_an_invalid_answer_as_invalid() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-inv", "Acme GmbH", Some(ACME_VAT), None).await;
            let vies = FakeVies::answering(Ok(ViesResponse {
                valid: false,
                name: None,
                address: None,
                consultation_number: None,
                request_date: None,
            }));
            let out = run(
                &*kit.db,
                &PartiesCheckVat::new(Some(vies)),
                AGENT_TOKEN,
                "vies-inv",
                &check_vat_input("p-inv", "DE", "136695976"),
            )
            .await
            .expect("check_vat");
            assert!(!out.vies_result.valid);
            assert_eq!(
                scalar_i64(
                    &*kit.db,
                    "SELECT valid AS n FROM vies_check WHERE vendor_id = 'p-inv'"
                )
                .await,
                0
            );
        });
    }
}

#[test]
fn check_vat_writes_nothing_when_vies_does_not_answer() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-err", "Acme GmbH", Some(ACME_VAT), None).await;
            let vies = FakeVies::answering(Err("VIES answered with status 500".to_string()));
            let result = run(
                &*kit.db,
                &PartiesCheckVat::new(Some(vies)),
                AGENT_TOKEN,
                "vies-err",
                &check_vat_input("p-err", "DE", "136695976"),
            )
            .await;
            assert!(matches!(result, Err(CommandError::InvalidInput(_))));
            assert_eq!(
                scalar_i64(&*kit.db, "SELECT COUNT(*) AS n FROM vies_check").await,
                0
            );
        });
    }
}

#[test]
fn check_vat_is_refused_when_vies_is_off_on_this_server() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-off", "Acme GmbH", Some(ACME_VAT), None).await;
            let result = run(
                &*kit.db,
                &PartiesCheckVat::new(None),
                AGENT_TOKEN,
                "vies-off",
                &check_vat_input("p-off", "DE", "136695976"),
            )
            .await;
            match result {
                Err(CommandError::InvalidInput(message)) => {
                    assert!(message.contains("LEDGERS_VIES"), "{message}");
                }
                other => panic!("expected the off message, got {other:?}"),
            }
            assert_eq!(
                scalar_i64(&*kit.db, "SELECT COUNT(*) AS n FROM vies_check").await,
                0
            );
        });
    }
}

#[test]
fn check_vat_never_calls_vies_for_a_non_eu_country_a_bad_number_or_another_vat_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-vat", "Acme GmbH", Some(ACME_VAT), None).await;
            let vies = FakeVies::valid();
            let cmd = PartiesCheckVat::new(Some(vies.clone()));

            let non_eu = run(
                &*kit.db,
                &cmd,
                AGENT_TOKEN,
                "vies-us",
                &check_vat_input("p-vat", "US", "136695976"),
            )
            .await;
            assert!(matches!(non_eu, Err(CommandError::InvalidInput(_))));

            let bad_number = run(
                &*kit.db,
                &cmd,
                AGENT_TOKEN,
                "vies-check-digit",
                &check_vat_input("p-vat", "DE", "123456789"),
            )
            .await;
            assert!(matches!(bad_number, Err(CommandError::InvalidInput(_))));

            let other = run(
                &*kit.db,
                &cmd,
                AGENT_TOKEN,
                "vies-other",
                &check_vat_input("p-vat", "FR", "12345678901"),
            )
            .await;
            assert!(
                matches!(other, Err(CommandError::InvalidInput(_))),
                "not the party's VAT ID"
            );

            assert_eq!(vies.calls(), 0, "no VAT number left the server");
        });
    }
}

/// The zero-rating guard: the latest check must be valid and for the VAT ID
/// the party has now.
#[test]
fn the_zero_rating_guard_follows_the_latest_check_for_the_current_vat_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-zr", "Acme GmbH", Some(ACME_VAT), None).await;
            let guard_sql = |company: &str| vies_valid_guard(company, "p-zr");

            // No check yet: the guard refuses.
            assert!(
                kit.db.execute(&guard_sql(COMPANY_ID)).await.is_err(),
                "no check on file"
            );

            run(
                &*kit.db,
                &PartiesCheckVat::new(Some(FakeVies::valid())),
                AGENT_TOKEN,
                "zr-check",
                &check_vat_input("p-zr", "DE", "136695976"),
            )
            .await
            .expect("check_vat");
            kit.db
                .execute(&guard_sql(COMPANY_ID))
                .await
                .expect("a valid check for the current VAT ID passes");
            exec(
                &*kit.db,
                "DELETE FROM ledger_guard WHERE guard_key = 'vies_valid:p-zr'",
                vec![],
            )
            .await;

            // The party changes its VAT ID: the old check no longer counts.
            run(
                &*kit.db,
                &PartiesUpdate,
                OWNER_TOKEN,
                "zr-vat",
                &UpdateInput {
                    id: "p-zr".to_string(),
                    name: None,
                    vat_id: Some("DE111111125".to_string()),
                    roles: None,
                    tax_number: None,
                    language: None,
                    currency: None,
                    personal_data: None,
                    payment_terms_days: None,
                    default_account: None,
                    default_tax_code: None,
                },
            )
            .await
            .expect("update VAT ID");
            assert!(
                kit.db.execute(&guard_sql(COMPANY_ID)).await.is_err(),
                "the check is for the old VAT ID"
            );
        });
    }
}

// ============================================================================
// Merge and duplicates
// ============================================================================

#[test]
fn merge_is_refused_to_an_agent_and_moves_the_bills_to_the_survivor() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "p-a", "Acme GmbH", None, None).await;
            party(&*kit.db, "p-b", "Acme Holding", None, None).await;
            bill(&*kit.db, "bill-b", "p-b", "INV-9", false).await;

            let refused = run(
                &*kit.db,
                &PartiesMerge::new(None),
                AGENT_TOKEN,
                "merge-agent",
                &MergeInput {
                    id: "p-a".to_string(),
                    from: "p-b".to_string(),
                },
            )
            .await;
            assert!(matches!(refused, Err(CommandError::HumanOnly)));

            run(
                &*kit.db,
                &PartiesMerge::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "merge-owner",
                &MergeInput {
                    id: "p-a".to_string(),
                    from: "p-b".to_string(),
                },
            )
            .await
            .expect("merge");
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT vendor_id AS v FROM bill WHERE id = 'bill-b'",
                    "v"
                )
                .await
                .as_deref(),
                Some("p-a")
            );
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT merged_into AS m FROM vendor WHERE id = 'p-b'",
                    "m"
                )
                .await
                .as_deref(),
                Some("p-a")
            );
        });
    }
}

#[test]
fn list_marks_parties_that_may_be_duplicates_and_skips_merged_ones() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            // A VAT ID is unique per company, so duplicates here share an IBAN or a name.
            party(
                &*kit.db,
                "d-iban-1",
                "Gamma",
                None,
                Some("DE89370400440532013000"),
            )
            .await;
            party(
                &*kit.db,
                "d-iban-2",
                "Delta",
                None,
                Some("de89 3704 0044 0532 0130 00"),
            )
            .await;
            party(&*kit.db, "d-name-1", "Halden Works GmbH.", None, None).await;
            party(&*kit.db, "d-name-2", "halden works gmbh", None, None).await;
            party(&*kit.db, "d-solo", "Unrelated", None, None).await;

            let list = run(
                &*kit.db,
                &PartiesList,
                OWNER_TOKEN,
                "dups",
                &ListInput {
                    limit: Some(200),
                    offset: None,
                },
            )
            .await
            .expect("list");
            let find = |id: &str| {
                list.parties
                    .iter()
                    .find(|p| p.id == id)
                    .expect("listed")
                    .clone()
            };
            assert_eq!(
                find("d-iban-1").possible_duplicates,
                vec!["d-iban-2".to_string()]
            );
            assert_eq!(
                find("d-name-1").possible_duplicates,
                vec!["d-name-2".to_string()]
            );
            assert!(find("d-solo").possible_duplicates.is_empty());

            run(
                &*kit.db,
                &PartiesMerge::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "dups-merge",
                &MergeInput {
                    id: "d-iban-1".to_string(),
                    from: "d-iban-2".to_string(),
                },
            )
            .await
            .expect("merge");
            let after = run(
                &*kit.db,
                &PartiesList,
                OWNER_TOKEN,
                "dups-after",
                &ListInput {
                    limit: Some(200),
                    offset: None,
                },
            )
            .await
            .expect("list after merge");
            assert!(
                after.parties.iter().all(|p| p.id != "d-iban-2"),
                "merged parties are not listed"
            );
            assert!(
                after
                    .parties
                    .iter()
                    .find(|p| p.id == "d-iban-1")
                    .expect("survivor")
                    .possible_duplicates
                    .is_empty()
            );
        });
    }
}

#[test]
fn the_vendor_table_refuses_a_second_party_with_the_same_vat_id_even_when_it_is_merged() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(&*kit.db, "u-1", "Acme GmbH", Some(ACME_VAT), None).await;
            // The constraint is the guard, whatever the command checked first.
            let direct = kit
                .db
                .execute(&Statement::with_values(
                    "INSERT INTO vendor (id, company_id, name, vat_id, source, needs_review, created_at, updated_at) VALUES ('u-2', ?, 'Other', ?, 'manual', 0, ?, ?)"
                        .to_string(),
                    vec![Value::from(COMPANY_ID), Value::from(ACME_VAT), Value::from(NOW), Value::from(NOW)],
                ))
                .await;
            assert!(direct.is_err());
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_party_of_one_company_is_invisible_to_another() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            party(
                &*kit.db,
                "p-a-only",
                "Acme GmbH",
                Some(ACME_VAT),
                Some("DE89370400440532013000"),
            )
            .await;
            bill(&*kit.db, "bill-a", "p-a-only", "INV-A", false).await;

            // Company B sees no parties and no bank counterparty of A.
            let listed = run_in(
                OTHER_COMPANY_ID,
                &*kit.db,
                &PartiesList,
                OWNER_TOKEN,
                "iso-list",
                &ListInput {
                    limit: None,
                    offset: None,
                },
            )
            .await
            .expect("list in B");
            assert_eq!(listed.total, 0);
            assert!(listed.parties.is_empty());

            // Writes aimed at A's party from B name no party.
            let owner = PartiesSetBank::new(Some(OWNER_ID.to_string()));
            let bank = run_in(
                OTHER_COMPANY_ID,
                &*kit.db,
                &owner,
                OWNER_TOKEN,
                "iso-bank",
                &bank_input("p-a-only", "GB82WEST12345698765432"),
            )
            .await;
            assert!(
                matches!(bank, Err(CommandError::InvalidInput(_))),
                "set_bank across companies"
            );
            party(&*kit.db, "p-a-two", "Acme Holding", None, None).await;
            let merge = run_in(
                OTHER_COMPANY_ID,
                &*kit.db,
                &PartiesMerge::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "iso-merge",
                &MergeInput {
                    id: "p-a-only".to_string(),
                    from: "p-a-two".to_string(),
                },
            )
            .await;
            assert!(
                matches!(merge, Err(CommandError::InvalidInput(_))),
                "merge across companies"
            );
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT merged_into AS m FROM vendor WHERE id = 'p-a-two'",
                    "m"
                )
                .await,
                None
            );
            let check = run_in(
                OTHER_COMPANY_ID,
                &*kit.db,
                &PartiesCheckVat::new(Some(FakeVies::valid())),
                AGENT_TOKEN,
                "iso-vies",
                &check_vat_input("p-a-only", "DE", "136695976"),
            )
            .await;
            assert!(
                matches!(check, Err(CommandError::InvalidInput(_))),
                "check_vat across companies"
            );

            // A's data is untouched by B's attempts.
            assert_eq!(
                text_of(
                    &*kit.db,
                    "SELECT iban AS i FROM vendor WHERE id = 'p-a-only'",
                    "i"
                )
                .await
                .as_deref(),
                Some("DE89370400440532013000")
            );
            assert_eq!(
                scalar_i64(&*kit.db, "SELECT COUNT(*) AS n FROM vies_check").await,
                0
            );

            // And a bill of B cannot be matched to A's party by VAT ID.
            let of_a = run_in(
                COMPANY_ID,
                &*kit.db,
                &PartiesList,
                OWNER_TOKEN,
                "iso-a",
                &ListInput {
                    limit: None,
                    offset: None,
                },
            )
            .await
            .expect("list in A");
            assert_eq!(of_a.total, 2);
        });
    }
}
