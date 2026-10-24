//! The party directory (clients and suppliers with full metadata): the
//! metadata checks, the CSV round trip, the per-row dry run, and the rule that
//! an agent key never writes bank details from a file. Runs on every dialect
//! the harness offers, on the real migrations.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::BillsModule;
use ledgers_bills::parties::*;
use ledgers_bills::party_csv::*;
use ledgers_bills::party_fields::{TaxId, slug_of};
use ledgers_commands::auth::{Authenticator, Credential, Limits, Principal, Role, Tier};
use ledgers_commands::*;
use ledgers_db::DbModule;
use sea_query::Value;
use std::collections::BTreeMap;

const COMPANY_ID: &str = "01TESTCOMPANY0000000000000A";
const NOW: &str = "2026-01-15T09:30:00Z";
const OWNER_ID: &str = "owner-acme";
const AGENT_ID: &str = "agent-key-draft";
const OWNER_TOKEN: &str = "bearer-owner";
const AGENT_TOKEN: &str = "bearer-draft-agent";

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

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![Box::new(DbModule), Box::new(BillsModule)]
}

fn context(request_id: &str) -> CommandContext {
    CommandContext {
        company_id: COMPANY_ID.to_string(),
        request_id: request_id.to_string(),
        event_id: format!("{request_id}-event"),
        now: NOW.to_string(),
        is_person: false,
    }
}

async fn run<C: Command>(
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
            context(request_id),
            input,
            Execute::Run,
        )
        .await?
    {
        Executed::Done(output) => Ok(output),
        _ => panic!("expected the command to run to completion"),
    }
}

async fn count(db: &dyn Database, sql: &str) -> i64 {
    db.query(&Statement::with_values(
        sql.to_string(),
        vec![Value::from(COMPANY_ID)],
    ))
    .await
    .expect("count query")
    .rows
    .first()
    .and_then(|row| row.get::<i64>("n"))
    .expect("a count")
}

/// The problem slug of a refused command.
fn refusal<T: std::fmt::Debug>(result: &Result<T, CommandError>) -> Option<&'static str> {
    match result {
        Err(error) => slug_of(error),
        Ok(_) => None,
    }
}

fn blank_create(id: &str, name: &str) -> CreateInput {
    CreateInput {
        id: Some(id.to_string()),
        name: name.to_string(),
        ..Default::default()
    }
}

// A canonical file: the export of the parties below, byte for byte.
const CANONICAL_CSV: &str = "id,name,trading_name,roles,vat_id,tax_ids,tax_number,registration_number,website,email,phone,currency,language,payment_terms_days,default_account,default_tax_code,default_unit,tags,notes,personal_data,addresses,contacts,iban,bic,bank_reason
party-acme,Acme GmbH,Acme,both,DE136695976,\"[{\"\"country\"\":\"\"FR\"\",\"\"kind\"\":\"\"vat\"\",\"\"value\"\":\"\"FR12345678901\"\"}]\",,HRB 12345,https://acme.example,billing@acme.example,'+49 30 1234567,EUR,de,30,4000,DE19,,berlin;retail,\"Pays on time, asks for PDFs.\nSecond line \"\"quoted\"\".\",false,\"[{\"\"kind\"\":\"\"billing\"\",\"\"line1\"\":\"\"Musterstrasse 1\"\",\"\"line2\"\":null,\"\"postal_code\"\":\"\"10115\"\",\"\"city\"\":\"\"Berlin\"\",\"\"country\"\":\"\"DE\"\"}]\",\"[{\"\"name\"\":\"\"Ada\"\",\"\"email\"\":\"\"ada@acme.example\"\",\"\"phone\"\":null,\"\"role\"\":\"\"Buyer\"\",\"\"primary\"\":true}]\",,,
party-zeta,Zeta Supplies Ltd,,vendor,,,,,,,,,,,,,,,,false,,,,,
";

/// A change to a create input that makes one field malformed.
type Mutate = Box<dyn Fn(&mut CreateInput)>;

/// Each malformed metadata field, the change that makes it malformed, and the slug it is refused with.
fn malformed_cases() -> Vec<(&'static str, Mutate, &'static str)> {
    let mut cases = malformed_identity_cases();
    cases.extend(malformed_tax_and_contact_cases());
    cases
}

/// The malformed identity, contact-detail and terms fields.
fn malformed_identity_cases() -> Vec<(&'static str, Mutate, &'static str)> {
    vec![
        (
            "website",
            Box::new(|i: &mut CreateInput| {
                i.website = Some("acme.example".into());
            }),
            "party-website-invalid",
        ),
        (
            "phone",
            Box::new(|i: &mut CreateInput| {
                i.phone = Some("call me".into());
            }),
            "party-phone-invalid",
        ),
        (
            "email",
            Box::new(|i: &mut CreateInput| {
                i.email = Some("nobody at x".into());
            }),
            "party-email-invalid",
        ),
        (
            "currency",
            Box::new(|i: &mut CreateInput| {
                i.currency = Some("EURO".into());
            }),
            "party-currency-invalid",
        ),
        (
            "language",
            Box::new(|i: &mut CreateInput| {
                i.language = Some("123".into());
            }),
            "party-language-invalid",
        ),
        (
            "unit",
            Box::new(|i: &mut CreateInput| {
                i.default_unit = Some("berlin 1".into());
            }),
            "party-unit-invalid",
        ),
        (
            "registration",
            Box::new(|i: &mut CreateInput| {
                i.registration_number = Some("HRB#1".into());
            }),
            "party-registration-invalid",
        ),
        (
            "notes",
            Box::new(|i: &mut CreateInput| {
                i.notes = Some("x".repeat(4001));
            }),
            "party-notes-too-long",
        ),
        (
            "tags",
            Box::new(|i: &mut CreateInput| {
                i.tags = vec!["a,b".into()];
            }),
            "party-tags-invalid",
        ),
    ]
}

/// The malformed tax IDs and contacts.
fn malformed_tax_and_contact_cases() -> Vec<(&'static str, Mutate, &'static str)> {
    vec![
        (
            "tax id",
            Box::new(|i: &mut CreateInput| {
                i.tax_ids = vec![TaxId {
                    country: "DE".into(),
                    kind: "vat".into(),
                    value: "DE123456789".into(),
                }];
            }),
            "party-tax-id-invalid",
        ),
        (
            "tax id kind",
            Box::new(|i: &mut CreateInput| {
                i.tax_ids = vec![TaxId {
                    country: "DE".into(),
                    kind: "gst".into(),
                    value: "1".into(),
                }];
            }),
            "party-tax-id-invalid",
        ),
        (
            "tax id twice",
            Box::new(|i: &mut CreateInput| {
                i.tax_ids = vec![
                    TaxId {
                        country: "FR".into(),
                        kind: "tax".into(),
                        value: "9".into(),
                    },
                    TaxId {
                        country: "FR".into(),
                        kind: "tax".into(),
                        value: "9".into(),
                    },
                ];
            }),
            "party-tax-id-duplicate",
        ),
        (
            "primary VAT repeated",
            Box::new(|i: &mut CreateInput| {
                i.vat_id = Some("FR12345678901".into());
                i.tax_ids = vec![TaxId {
                    country: "FR".into(),
                    kind: "vat".into(),
                    value: "12345678901".into(),
                }];
            }),
            "party-tax-id-duplicate",
        ),
        (
            "two primary contacts",
            Box::new(|i: &mut CreateInput| {
                i.contacts = vec![
                    Contact {
                        name: "A".into(),
                        primary: true,
                        ..Default::default()
                    },
                    Contact {
                        name: "B".into(),
                        primary: true,
                        ..Default::default()
                    },
                ];
            }),
            "party-contact-primary-duplicate",
        ),
        (
            "contact email",
            Box::new(|i: &mut CreateInput| {
                i.contacts = vec![Contact {
                    name: "A".into(),
                    email: Some("a@@b".into()),
                    ..Default::default()
                }];
            }),
            "party-contact-invalid",
        ),
        (
            "payment terms",
            Box::new(|i: &mut CreateInput| {
                i.payment_terms_days = Some(-3);
            }),
            "party-payment-terms-invalid",
        ),
    ]
}

/// A complete party, with every metadata field the directory holds.
fn complete_party() -> CreateInput {
    let mut input = blank_create("p-full", "Acme GmbH");
    input.trading_name = Some("Acme".to_string());
    input.vat_id = Some("DE136695976".to_string());
    input.tax_ids = vec![TaxId {
        country: "fr".to_string(),
        kind: "vat".to_string(),
        value: "FR12345678901".to_string(),
    }];
    input.registration_number = Some("HRB 12345".to_string());
    input.website = Some("https://acme.example/en".to_string());
    input.email = Some("billing@acme.example".to_string());
    input.phone = Some("+49 30 1234567".to_string());
    input.currency = Some("eur".to_string());
    input.language = Some("de-DE".to_string());
    input.default_unit = Some("berlin-1".to_string());
    input.tags = vec![
        "Retail".to_string(),
        "retail".to_string(),
        "berlin".to_string(),
    ];
    input.notes = Some("Pays on time.".to_string());
    input.contacts = vec![
        Contact {
            name: "Ada".to_string(),
            email: Some("ada@acme.example".to_string()),
            primary: true,
            ..Default::default()
        },
        Contact {
            name: "Grace".to_string(),
            ..Default::default()
        },
    ];
    input
}

#[test]
fn a_complete_party_is_stored_with_its_metadata() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let created = run(
                db,
                &PartiesCreate,
                OWNER_TOKEN,
                "create-full",
                &complete_party(),
            )
            .await
            .expect("a complete party is accepted");
            assert_eq!(created.party.currency.as_deref(), Some("EUR"));
            assert_eq!(created.party.language.as_deref(), Some("de-DE"));
            assert_eq!(created.party.tags, vec!["retail", "berlin"]);
            assert_eq!(created.party.tax_ids[0].value, "FR12345678901");
            assert_eq!(created.party.tax_ids[0].country, "FR");

            let got = run(
                db,
                &PartiesGet::new(false),
                OWNER_TOKEN,
                "get-full",
                &GetInput {
                    id: "p-full".to_string(),
                },
            )
            .await
            .expect("get");
            assert_eq!(got.party.trading_name.as_deref(), Some("Acme"));
            assert_eq!(
                got.party.website.as_deref(),
                Some("https://acme.example/en")
            );
            assert_eq!(got.party.default_unit.as_deref(), Some("berlin-1"));
            assert_eq!(got.party.notes.as_deref(), Some("Pays on time."));
            assert!(got.party.contacts[0].primary);
            assert!(!got.party.contacts[1].primary);
        });
    }
}

#[test]
fn each_malformed_metadata_field_is_refused_with_its_slug_and_nothing_is_written() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            for (index, (label, change, slug)) in malformed_cases().iter().enumerate() {
                let mut bad = blank_create(&format!("p-bad-{index}"), "Bad GmbH");
                change(&mut bad);
                let refused = run(
                    db,
                    &PartiesCreate,
                    OWNER_TOKEN,
                    &format!("bad-{index}"),
                    &bad,
                )
                .await;
                assert_eq!(refusal(&refused), Some(*slug), "{label}: {refused:?}");
            }
            let stored = count(
                db,
                "SELECT COUNT(*) AS n FROM vendor WHERE company_id = ? AND id LIKE 'p-bad-%'",
            )
            .await;
            assert_eq!(stored, 0, "a refused party is never written");
        });
    }
}

#[test]
fn an_update_checks_the_same_fields_and_keeps_what_it_does_not_name() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            run(
                db,
                &PartiesCreate,
                OWNER_TOKEN,
                "create-for-update",
                &complete_party(),
            )
            .await
            .expect("create");
            let mut update = UpdateInput {
                id: "p-full".to_string(),
                website: Some("http://".to_string()),
                ..Default::default()
            };
            let refused = run(db, &PartiesUpdate, OWNER_TOKEN, "update-bad", &update).await;
            assert_eq!(refusal(&refused), Some("party-website-invalid"));
            update.website = None;
            update.name = Some("Acme GmbH & Co KG".to_string());
            let renamed = run(db, &PartiesUpdate, OWNER_TOKEN, "update-name", &update)
                .await
                .expect("an update that names only the name");
            assert_eq!(
                renamed.party.website.as_deref(),
                Some("https://acme.example/en")
            );
            assert_eq!(renamed.party.name, "Acme GmbH & Co KG");
        });
    }
}

#[test]
fn a_csv_export_imports_back_unchanged_and_the_canonical_file_round_trips() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            // An empty company: the canonical file creates both parties.
            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-canonical",
                &ImportInput {
                    csv: CANONICAL_CSV.to_string(),
                },
            )
            .await
            .expect("the canonical file checks");
            assert!(check.valid, "{:?}", check.rows);
            assert_eq!(check.creates, 2);

            run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-canonical",
                &ImportInput {
                    csv: CANONICAL_CSV.to_string(),
                },
            )
            .await
            .expect("the canonical file imports");

            let exported = run(db, &PartiesExport, OWNER_TOKEN, "export-1", &ExportInput {})
                .await
                .expect("export");
            assert_eq!(exported.rows, 2);
            assert_eq!(exported.csv, CANONICAL_CSV, "export of what was imported");

            // Importing the export again updates both parties and changes nothing.
            let again = run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-again",
                &ImportInput {
                    csv: exported.csv.clone(),
                },
            )
            .await
            .expect("re-import of an export");
            assert_eq!((again.created, again.updated), (0, 2));
            let after = run(db, &PartiesExport, OWNER_TOKEN, "export-2", &ExportInput {})
                .await
                .expect("export");
            assert_eq!(after.csv, exported.csv, "a re-import changes nothing");
        });
    }
}

#[test]
fn a_dry_run_reports_each_bad_row_and_the_import_writes_nothing() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let file = "id,name,vat_id,email\n\
                        ok-1,Good Ltd,,ok@good.example\n\
                        bad-vat,Bad VAT GmbH,DE123456789,\n\
                        bad-mail,Bad Mail Ltd,,nobody at x\n";
            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-bad",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await
            .expect("a check reports rather than refuses");
            assert!(!check.valid);
            assert_eq!(check.errors, 2);
            assert_eq!(check.rows.len(), 3);
            assert_eq!(check.rows[0].line, 2);
            assert!(check.rows[0].errors.is_empty(), "the good row is valid");
            assert_eq!(check.rows[1].line, 3);
            assert_eq!(check.rows[1].errors[0].slug, "party-vat-invalid");
            assert_eq!(check.rows[2].line, 4);
            assert_eq!(check.rows[2].errors[0].slug, "party-email-invalid");

            let refused = run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-bad",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await;
            assert_eq!(refusal(&refused), Some("party-import-invalid"));
            assert_eq!(
                count(db, "SELECT COUNT(*) AS n FROM vendor WHERE company_id = ?").await,
                0
            );
        });
    }
}

#[test]
fn an_agent_key_never_writes_bank_details_from_a_file() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let file = "id,name,iban,bic,bank_reason\n\
                        bank-1,Bank Party GmbH,DE89370400440532013000,,Supplier moved banks\n";
            let agent = run(
                db,
                &PartiesImportCheck::new(None),
                AGENT_TOKEN,
                "check-agent-bank",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await
            .expect("the check reports");
            assert!(!agent.valid);
            assert_eq!(agent.rows[0].errors[0].slug, "party-bank-needs-person");

            let refused = run(
                db,
                &PartiesImport::new(None),
                AGENT_TOKEN,
                "import-agent-bank",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await;
            assert_eq!(refusal(&refused), Some("party-import-invalid"));
            assert_eq!(
                count(db, "SELECT COUNT(*) AS n FROM vendor WHERE company_id = ?").await,
                0
            );
            assert_eq!(
                count(
                    db,
                    "SELECT COUNT(*) AS n FROM vendor_bank_change WHERE company_id = ?"
                )
                .await,
                0
            );

            // A person may import the same details, with the reason in the history.
            let person = run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-person-bank",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await
            .expect("a person imports bank details with a reason");
            assert_eq!(person.created, 1);
            assert_eq!(
                count(
                    db,
                    "SELECT COUNT(*) AS n FROM vendor_bank_change WHERE company_id = ?"
                )
                .await,
                1
            );
            let stored = run(
                db,
                &PartiesGet::new(false),
                OWNER_TOKEN,
                "get-bank-party",
                &GetInput {
                    id: "bank-1".to_string(),
                },
            )
            .await
            .expect("get");
            assert_eq!(
                stored.bank_changes[0].reason.as_deref(),
                Some("Supplier moved banks")
            );
            assert_eq!(stored.bank_changes[0].changed_by, OWNER_ID);
        });
    }
}

#[test]
fn a_bank_row_without_a_reason_and_a_malformed_file_are_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let no_reason =
                "id,name,iban,bank_reason\nbank-2,No Reason GmbH,DE89370400440532013000,\n";
            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-no-reason",
                &ImportInput {
                    csv: no_reason.to_string(),
                },
            )
            .await
            .expect("check");
            assert_eq!(check.rows[0].errors[0].slug, "party-bank-reason-required");

            let unknown = "id,name,surprise\nx,Y,1\n";
            let refused = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-unknown-column",
                &ImportInput {
                    csv: unknown.to_string(),
                },
            )
            .await;
            assert_eq!(refusal(&refused), Some("party-csv-invalid"));

            let unclosed = "id,name\nq-1,\"Unclosed\n";
            let refused = run(
                db,
                &PartiesImportCheck::new(None),
                OWNER_TOKEN,
                "check-unclosed",
                &ImportInput {
                    csv: unclosed.to_string(),
                },
            )
            .await;
            assert_eq!(refusal(&refused), Some("party-csv-invalid"));

            let too_many: String = std::iter::once(
                "id,name
"
                .to_string(),
            )
            .chain((0..=500).map(|i| {
                format!(
                    "many-{i},Party {i}
"
                )
            }))
            .collect();
            let refused = run(
                db,
                &PartiesImportCheck::new(None),
                OWNER_TOKEN,
                "check-too-many",
                &ImportInput { csv: too_many },
            )
            .await;
            assert_eq!(refusal(&refused), Some("party-import-too-many-rows"));
        });
    }
}

#[test]
fn a_party_listed_twice_in_a_file_is_refused_on_its_second_row() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let file = "id,name\nsame,First\nsame,Second\n";
            let check = run(
                db,
                &PartiesImportCheck::new(None),
                OWNER_TOKEN,
                "check-twice",
                &ImportInput {
                    csv: file.to_string(),
                },
            )
            .await
            .expect("check");
            assert!(check.rows[0].errors.is_empty());
            assert!(
                !check.rows[1].errors.is_empty(),
                "the second row is refused"
            );
        });
    }
}

#[test]
fn validate_names_the_slug_of_each_refused_field() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let checked = run(
                db,
                &PartiesValidate,
                OWNER_TOKEN,
                "validate-slugs",
                &ValidateInput {
                    website: Some("acme.example".to_string()),
                    email: Some("nobody at x".to_string()),
                    contacts: vec![
                        Contact {
                            name: "A".to_string(),
                            primary: true,
                            ..Default::default()
                        },
                        Contact {
                            name: "B".to_string(),
                            primary: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            )
            .await
            .expect("validate reports rather than refuses");
            assert!(!checked.valid);
            let slug_of_field = |field: &str| {
                checked
                    .errors
                    .iter()
                    .find(|e| e.field == field)
                    .and_then(|e| e.slug.clone())
            };
            assert_eq!(
                slug_of_field("website").as_deref(),
                Some("party-website-invalid")
            );
            assert_eq!(
                slug_of_field("email").as_deref(),
                Some("party-email-invalid")
            );
            assert_eq!(
                slug_of_field("contacts").as_deref(),
                Some("party-contact-primary-duplicate")
            );
        });
    }
}

/// One party read back by id, through `parties.get`.
async fn get_party(db: &dyn Database, id: &str) -> PartyView {
    run(
        db,
        &PartiesGet::new(false),
        OWNER_TOKEN,
        &format!("get-{id}"),
        &GetInput { id: id.to_string() },
    )
    .await
    .expect("get")
    .party
}

/// One CSV cell, quoted the way the export quotes it.
fn csv_quote(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// One full-width row of a file: the cells named in `cells`, the rest blank,
/// with the roles and personal data every export writes.
fn full_row(cells: &[(&str, &str)]) -> String {
    let mut row: Vec<String> = COLUMNS
        .iter()
        .map(|column| match *column {
            "roles" => "vendor".to_string(),
            "personal_data" => "false".to_string(),
            _ => String::new(),
        })
        .collect();
    for (column, value) in cells {
        let index = COLUMNS
            .iter()
            .position(|c| c == column)
            .expect("a known column");
        row[index] = (*value).to_string();
    }
    row.iter()
        .map(|cell| csv_quote(cell))
        .collect::<Vec<_>>()
        .join(",")
}

/// A whole file: the header of every column, then the rows.
fn full_file(rows: &[String]) -> String {
    format!("{}\n{}\n", COLUMNS.join(","), rows.join("\n"))
}

#[test]
fn formula_cells_import_as_their_values_and_export_as_escaped_cells() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            // The file holds the escaped form the export writes. Each party's
            // stored value is the one the escape stands for. Rows are in name
            // order of the stored values, which is the order the export writes.
            let hyper_name = "=HYPERLINK(\"https://evil.example\",\"Invoice\")";
            let hyper_file = format!("'{hyper_name}");
            let rows = vec![
                full_row(&[
                    ("id", "party-apos"),
                    ("name", "''hello"),
                    ("notes", "''hello"),
                ]),
                full_row(&[
                    ("id", "party-plus"),
                    ("name", "'+Plus Ltd"),
                    ("notes", "'-12.50"),
                ]),
                full_row(&[
                    ("id", "party-hyper"),
                    ("name", &hyper_file),
                    ("notes", "''=x"),
                ]),
                full_row(&[
                    ("id", "party-plain"),
                    ("name", "Acme GmbH"),
                    ("notes", "O'Brien"),
                ]),
            ];
            let file = full_file(&rows);

            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-formula",
                &ImportInput { csv: file.clone() },
            )
            .await
            .expect("the escaped file checks");
            assert!(check.valid, "{:?}", check.rows);
            assert_eq!(check.creates, 4);

            run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-formula",
                &ImportInput { csv: file.clone() },
            )
            .await
            .expect("the escaped file imports");

            // The stored values are the unescaped ones.
            let hyper = get_party(db, "party-hyper").await;
            assert_eq!(hyper.name, hyper_name);
            assert_eq!(hyper.notes.as_deref(), Some("'=x"));
            let apos = get_party(db, "party-apos").await;
            assert_eq!(apos.name, "'hello");
            assert_eq!(apos.notes.as_deref(), Some("'hello"));
            let plus = get_party(db, "party-plus").await;
            assert_eq!(plus.name, "+Plus Ltd");
            assert_eq!(plus.notes.as_deref(), Some("-12.50"));
            let plain = get_party(db, "party-plain").await;
            assert_eq!(plain.notes.as_deref(), Some("O'Brien"));

            // The export writes the escaped form, in name order of the stored values.
            let exported = run(
                db,
                &PartiesExport,
                OWNER_TOKEN,
                "export-formula",
                &ExportInput {},
            )
            .await
            .expect("export");
            assert_eq!(exported.csv, file);

            // Importing the export again changes nothing.
            run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-formula-again",
                &ImportInput {
                    csv: exported.csv.clone(),
                },
            )
            .await
            .expect("re-import of the export");
            let again = run(
                db,
                &PartiesExport,
                OWNER_TOKEN,
                "export-formula-again",
                &ExportInput {},
            )
            .await
            .expect("export");
            assert_eq!(again.csv, exported.csv);
        });
    }
}

#[test]
fn a_guarded_number_is_parsed_after_its_escape_is_removed() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            // `'-12` is the escaped form of `-12`. The escape comes off before
            // the cell is parsed, so the refusal is the one for a negative
            // number of days, not one for a value that is not a whole number.
            let file = full_file(&[full_row(&[
                ("id", "party-terms"),
                ("name", "Terms Ltd"),
                ("payment_terms_days", "'-12"),
            ])]);
            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-terms",
                &ImportInput { csv: file },
            )
            .await
            .expect("the file checks");
            assert!(!check.valid);
            let errors = &check.rows[0].errors;
            assert!(
                errors
                    .iter()
                    .any(|e| e.message == "payment_terms_days must be >= 0"),
                "{errors:?}"
            );
            assert!(
                errors.iter().all(|e| !e.message.contains("whole number")),
                "{errors:?}"
            );
        });
    }
}

/// The Halden Works demo parties, with formula-shaped text in their
/// notes, as an invoice scan could bring in.
fn demo_party_rows() -> Vec<String> {
    vec![
        full_row(&[
            ("id", "party-acme"),
            ("name", "Acme GmbH"),
            ("trading_name", "Acme"),
            ("roles", "both"),
            ("vat_id", "DE136695976"),
            ("currency", "EUR"),
            ("payment_terms_days", "30"),
            ("tags", "berlin;retail"),
            ("notes", "'-12.50 deposit, '=SUM(A1:A9) in the old sheet"),
        ]),
        full_row(&[
            ("id", "party-halden"),
            ("name", "Halden Works GmbH"),
            ("trading_name", "Halden"),
            ("roles", "customer"),
            ("currency", "EUR"),
            ("notes", "'@owner entered from a scan"),
        ]),
        full_row(&[
            ("id", "party-lumen"),
            ("name", "Lumen Logistik AG"),
            ("roles", "both"),
            ("notes", "'+49 30 1234567 is the switchboard"),
        ]),
        full_row(&[
            ("id", "party-tovar"),
            ("name", "Tovar Studio"),
            ("roles", "vendor"),
            ("notes", "''quoted name"),
        ]),
    ]
}

#[test]
fn the_demo_party_set_round_trips_with_identical_values() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let db = &*kit.db;
            let rows = demo_party_rows();
            let file = full_file(&rows);
            let check = run(
                db,
                &PartiesImportCheck::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "check-demo",
                &ImportInput { csv: file.clone() },
            )
            .await
            .expect("the demo set checks");
            assert!(check.valid, "{:?}", check.rows);
            assert_eq!(check.creates, 4);
            run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-demo",
                &ImportInput { csv: file.clone() },
            )
            .await
            .expect("the demo set imports");

            let first = run(
                db,
                &PartiesExport,
                OWNER_TOKEN,
                "export-demo-1",
                &ExportInput {},
            )
            .await
            .expect("export");
            assert_eq!(first.rows, 4);
            // The rows are already in name order, so the export is the file.
            assert_eq!(first.csv, file);

            // Importing the export back changes nothing, and exporting again
            // gives the same text.
            run(
                db,
                &PartiesImport::new(Some(OWNER_ID.to_string())),
                OWNER_TOKEN,
                "import-demo-again",
                &ImportInput {
                    csv: first.csv.clone(),
                },
            )
            .await
            .expect("re-import of the demo export");
            let second = run(
                db,
                &PartiesExport,
                OWNER_TOKEN,
                "export-demo-2",
                &ExportInput {},
            )
            .await
            .expect("export");
            assert_eq!(second.csv, first.csv);

            let acme = run(
                db,
                &PartiesGet::new(false),
                OWNER_TOKEN,
                "get-demo-acme",
                &GetInput {
                    id: "party-acme".to_string(),
                },
            )
            .await
            .expect("get")
            .party;
            assert_eq!(
                acme.notes.as_deref(),
                Some("-12.50 deposit, '=SUM(A1:A9) in the old sheet")
            );
        });
    }
}
