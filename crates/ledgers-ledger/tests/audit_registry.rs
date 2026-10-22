//! The audit registry test: every mutating or denied command emits
//! exactly one audit event (issue #9's acceptance criterion).
//!
//! Three checks, in order of what they would catch:
//!
//! 1. **Coverage.** Every `pub async fn` in the four command modules is
//!    in `COMMANDS` or `EXEMPT`, and every entry names a function that
//!    exists. A new command cannot be added without saying what it owes
//!    the audit log.
//! 2. **Behaviour.** Every `Mutating` / `Refused` / `HumanOnly` command is
//!    actually run, and the audit chain is asserted to have gained
//!    exactly one event naming it — with the outcome the registry kind
//!    promises. `HumanOnly` commands are run twice, once with an agent
//!    key (`denied`) and once with a person (`allowed`).
//! 3. **The chain still verifies.** `ledgers_audit::verify_db` checks
//!    every hash and every `seq` at the end, so the events this test
//!    produced are provably a well-formed chain, not just present.
//!
//! SQLite only: the statements under test are the same on both
//! dialects, and ledgers-audit's own matrix covers the Postgres leg of
//! the same chain.

mod common;

use cratefield_core::{Database, Statement};
use ledgers_audit::{Actor, AuditContext, AuditEvent, Outcome, verify_db};
use ledgers_ledger::account::{AccountDraft, AccountType};
use ledgers_ledger::close::CloseCheckRegistry;
use ledgers_ledger::entry::{EntryDraft, Line, Side};
use ledgers_ledger::fiscal::FiscalYearSpec;
use ledgers_ledger::journal::{JournalDraft, JournalKind};
use ledgers_ledger::registry::{COMMANDS, CommandKind, EXEMPT};
use ledgers_ledger::{AccountsError, Entry, EntryJournal, commands, period, posting};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;

use common::{COMPANY_ID, FY, NOW, POSTED_AT};

/// The four modules whose public async functions are commands.
const SOURCES: [(&str, &str); 4] = [
    ("commands.rs", include_str!("../src/commands.rs")),
    ("posting.rs", include_str!("../src/posting.rs")),
    ("period.rs", include_str!("../src/period.rs")),
    ("journal.rs", include_str!("../src/journal.rs")),
];

#[test]
fn every_public_command_is_registered_and_every_entry_names_a_real_function() {
    let mut declared: Vec<(String, String)> = Vec::new();
    for (name, source) in SOURCES {
        // Only the part before the unit tests: a `#[cfg(test)]` helper
        // is not a command anybody can call.
        let head = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_else(|| panic!("{name} has no test module to bound the scan"));
        let mut rest = head;
        while let Some(found) = rest.find("pub async fn ") {
            let after = &rest[found + "pub async fn ".len()..];
            let len = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let function = format!("{}::{}", name.trim_end_matches(".rs"), &after[..len]);
            declared.push((function.clone(), name.to_owned()));
            rest = &after[len..];
        }
    }
    assert!(
        declared.len() >= 19,
        "the scan found only {} public async fns — the pattern is wrong, not the crate: {declared:?}",
        declared.len()
    );

    let registered: Vec<&str> = COMMANDS.iter().map(|spec| spec.function).collect();
    let exempt: Vec<&str> = EXEMPT.iter().map(|(function, _)| *function).collect();

    for (function, source) in &declared {
        assert!(
            registered.contains(&function.as_str()) || exempt.contains(&function.as_str()),
            "{function} ({source}) is a public command with no registry entry: add it to \
             COMMANDS with the audit obligation it carries, or to EXEMPT with a reason"
        );
    }
    for spec in COMMANDS {
        assert!(
            declared
                .iter()
                .any(|(function, _)| function == spec.function),
            "{} is registered but declares no such function",
            spec.function
        );
        assert!(
            spec.name.contains('.') && !spec.name.contains(' '),
            "{} is not a dotted command name: {}",
            spec.function,
            spec.name
        );
    }
    for (function, reason) in EXEMPT {
        assert!(
            !reason.trim().is_empty(),
            "{function} is exempt from the registry with no reason given"
        );
    }
}

/// The audit events of one company, oldest first.
async fn events(db: &dyn Database, company_id: &str) -> Vec<AuditEvent> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT payload FROM audit_event WHERE company_id = ? ORDER BY seq".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .expect("read the audit chain");
    rows.rows
        .iter()
        .map(|row| {
            let payload = row.get::<String>("payload").expect("payload is text");
            serde_json::from_str::<AuditEvent>(&payload).expect("payload is an audit event")
        })
        .collect()
}

/// The events naming `command` in the tail after `from`, for the
/// registry assertion.
fn named<'a>(after: &'a [AuditEvent], from: usize, command: &str) -> Vec<&'a AuditEvent> {
    after[from..]
        .iter()
        .filter(|event| event.command == command)
        .collect()
}

/// Runs one registry case against a fresh company and asserts it wrote
/// exactly one event with `expected`. The `match` on the command name
/// is deliberate: a new registry entry fails to compile here rather
/// than slipping through unasserted.
async fn assert_one_event(
    db: &dyn Database,
    company_id: &str,
    name: &str,
    expected: Outcome,
    run: impl Future<Output = Result<(), String>>,
) {
    let before = events(db, company_id).await.len();
    run.await
        .unwrap_or_else(|err| panic!("{name} failed: {err}"));
    let after = events(db, company_id).await;
    let found = named(&after, before, name);
    assert_eq!(
        found.len(),
        1,
        "{name} must write exactly one event naming itself; the chain gained {} in total: {after:?}",
        after.len() - before
    );
    assert_eq!(
        found[0].outcome, expected,
        "{name} recorded the wrong outcome"
    );
    assert!(
        found[0].company_id == company_id
            && found[0].seq > i64::try_from(before).expect("a small count"),
        "{name} recorded an event for the wrong company, or one from before the run"
    );
    assert_eq!(found[0].input_hash.len(), 64, "{name} hashed no input");
}

/// A documented policy refusal is the expected result of a denied run,
/// so it becomes a pass: what is under assertion is the audit event, not
/// the error. Any other outcome is a real failure.
fn refused<T>(outcome: Result<T, AccountsError>, what: &str) -> Result<(), String> {
    match outcome {
        Ok(_) => Err(format!(
            "{what} was allowed; the registry says it is refused"
        )),
        Err(AccountsError::HumanOnly { .. } | AccountsError::DeleteUnsupported) => Ok(()),
        Err(other) => Err(format!(
            "{what} failed as {other:?}, not as a policy refusal"
        )),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_mutating_or_denied_command_writes_exactly_one_audit_event() {
    let harness = common::harness();
    pollster::block_on(async {
        let db: &dyn Database = harness.db.as_ref();

        // A company of its own per case would need a fixture each; one
        // company, and each case asserts on the tail it caused, keeps
        // the chain order assertions honest and the setup readable.
        let company = COMPANY_ID;

        // Create the company with EUR base currency
        let _ = db
            .execute(&Statement::new(
                format!(
                    "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
                     VALUES ('{company}', 'Test Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
                ),
            ))
            .await;

        // The posting cases need somewhere to post into, and the year
        // they post in.
        common::mount(db, company).await;
        common::open_period(db, company, "2026-04").await;

        for spec in COMMANDS {
            if matches!(spec.kind, CommandKind::Read) {
                continue;
            }
            let person = || {
                AuditContext::new(
                    Actor::Person {
                        person_id: "usr_01HQZX9K3M7VN2P4R6T8W0Y1B3".to_owned(),
                    },
                    format!("req_registry_{}", spec.function),
                    format!("ev_registry_{}_{}", spec.function, counter()),
                )
            };
            let agent = || {
                AuditContext::new(
                    Actor::Agent {
                        key_id: "key_01HQZX9K3M7VN2P4R6T8W0Y1B5".to_owned(),
                        tier: "post-within-limits".to_owned(),
                    },
                    format!("req_registry_{}", spec.function),
                    format!("ev_registry_{}_{}", spec.function, counter()),
                )
            };

            match (spec.name, spec.kind) {
                ("account.create", CommandKind::Mutating) => {
                    let number = format!("8{:03}", 100 + COMMANDS.len() + counter());
                    assert_one_event(db, company, spec.name, Outcome::Allowed, async {
                        commands::create_account(
                            db,
                            company,
                            AccountDraft {
                                number: number.clone(),
                                name_key: "account.inline.registry".to_owned(),
                                account_type: AccountType::Asset,
                                normal_balance: None,
                                currency: None,
                                parent: None,
                                tags: Vec::new(),
                            },
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("account.archive", CommandKind::Mutating) => {
                    let number = format!("7{:03}", 100 + counter());
                    assert_one_event(db, company, spec.name, Outcome::Allowed, async {
                        commands::create_account(
                            db,
                            company,
                            AccountDraft {
                                number: number.clone(),
                                name_key: "account.inline.archive".to_owned(),
                                account_type: AccountType::Asset,
                                normal_balance: None,
                                currency: None,
                                parent: None,
                                tags: Vec::new(),
                            },
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map_err(|err| err.to_string())?;
                        commands::archive_account(db, company, &number, NOW.to_owned(), &person())
                            .await
                            .map(|_| ())
                            .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("account.delete", CommandKind::Refused) => {
                    assert_one_event(db, company, spec.name, Outcome::Denied, async {
                        refused(
                            commands::delete_account(
                                db,
                                company,
                                "8100",
                                NOW.to_owned(),
                                &person(),
                            )
                            .await,
                            "account.delete",
                        )
                    })
                    .await;
                }
                ("account.import_template", CommandKind::Mutating) => {
                    // A template of its own: the bundled charts are
                    // already imported for this company by other legs.
                    let chart = registry_template();
                    let company = format!("{company}-import");
                    common::mount(db, &company).await;
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        commands::import_template(db, &company, &chart, NOW.to_owned(), &person())
                            .await
                            .map(|_| ())
                            .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("journal.create", CommandKind::Mutating) => {
                    let code = format!("RG{}", counter());
                    assert_one_event(db, company, spec.name, Outcome::Allowed, async {
                        ledgers_ledger::journal::create_journal(
                            db,
                            company,
                            JournalDraft {
                                code: code.clone(),
                                kind: JournalKind::General,
                                name: "Registry journal".to_owned(),
                            },
                            &format!("jrn-registry-{code}"),
                            POSTED_AT,
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("journal.post_entry", CommandKind::Mutating) => {
                    let entry_id = format!("ent-registry-{:04}", counter());
                    assert_one_event(db, company, spec.name, Outcome::Allowed, async {
                        let journal_id = registry_journal(db, company).await?;
                        common::open_period(db, company, "2026-04").await;
                        let ctx = posting::PostContext {
                            fiscal_year: FY,
                            entry_id: entry_id.clone(),
                            recorded_at: POSTED_AT.to_owned(),
                            outbox_id: format!("{entry_id}-outbox"),
                        };
                        let entry = registry_entry(company, &journal_id, &entry_id, 2026);
                        posting::post_entry(
                            db,
                            &entry,
                            &ctx,
                            ledgers_db::RetryPolicy::default(),
                            |_| {},
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("fiscal_year.open", CommandKind::Mutating) => {
                    let label = 2030 + i64::try_from(counter()).expect("a small counter");
                    let company = format!("{company}-fy{label}");
                    common::mount(db, &company).await;
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        period::open_fiscal_year(
                            db,
                            &company,
                            FiscalYearSpec {
                                label,
                                start_month: 1,
                                has_adjustment_period: false,
                            },
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("period.soft_close", CommandKind::HumanOnly) => {
                    let (company, label) = registry_year(
                        db,
                        2040 + i64::try_from(counter()).expect("a small counter"),
                    )
                    .await;
                    // The agent key's attempt is refused, and refused
                    // audibly.
                    assert_one_event(db, &company, spec.name, Outcome::Denied, async {
                        refused(
                            period::soft_close_period(
                                db,
                                &company,
                                &format!("{label}-04"),
                                NOW.to_owned(),
                                &agent(),
                            )
                            .await,
                            "the agent's soft close",
                        )
                    })
                    .await;
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        period::soft_close_period(
                            db,
                            &company,
                            &format!("{label}-04"),
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("period.close", CommandKind::HumanOnly) => {
                    let (company, label) = registry_year(
                        db,
                        2040 + i64::try_from(counter()).expect("a small counter"),
                    )
                    .await;
                    assert_one_event(db, &company, spec.name, Outcome::Denied, async {
                        refused(
                            period::close_period(
                                db,
                                &company,
                                &format!("{label}-04"),
                                &EntryJournal,
                                &CloseCheckRegistry::new(),
                                None,
                                NOW.to_owned(),
                                &agent(),
                            )
                            .await,
                            "the agent's close",
                        )
                    })
                    .await;
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        period::close_period(
                            db,
                            &company,
                            &format!("{label}-04"),
                            &EntryJournal,
                            &CloseCheckRegistry::new(),
                            None,
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("period.reopen", CommandKind::HumanOnly) => {
                    let (company, label) = registry_year(
                        db,
                        2040 + i64::try_from(counter()).expect("a small counter"),
                    )
                    .await;
                    // Only a closed period can be reopened, so the
                    // soft close happens before either attempt.
                    period::soft_close_period(
                        db,
                        &company,
                        &format!("{label}-04"),
                        NOW.to_owned(),
                        &common::person(),
                    )
                    .await
                    .unwrap_or_else(|err| panic!("soft close before reopen: {err}"));
                    assert_one_event(db, &company, spec.name, Outcome::Denied, async {
                        refused(
                            period::reopen_period(
                                db,
                                &company,
                                &format!("{label}-04"),
                                "operator reviewed",
                                NOW.to_owned(),
                                &agent(),
                            )
                            .await,
                            "the agent's reopen",
                        )
                    })
                    .await;
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        period::reopen_period(
                            db,
                            &company,
                            &format!("{label}-04"),
                            "operator reviewed",
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                ("fiscal_year.close", CommandKind::HumanOnly) => {
                    let (company, label) = registry_year(
                        db,
                        2040 + i64::try_from(counter()).expect("a small counter"),
                    )
                    .await;
                    assert_one_event(db, &company, spec.name, Outcome::Denied, async {
                        refused(
                            period::close_fiscal_year(
                                db,
                                &company,
                                label,
                                &EntryJournal,
                                &CloseCheckRegistry::new(),
                                None,
                                NOW.to_owned(),
                                &agent(),
                            )
                            .await,
                            "the agent's year-end close",
                        )
                    })
                    .await;
                    // The close posts its own closing entry, so it also
                    // writes a `journal.post_entry` event — the registry
                    // counts by command name, and this case asserts
                    // only that `fiscal_year.close` itself wrote one.
                    assert_one_event(db, &company, spec.name, Outcome::Allowed, async {
                        // The close needs a base currency, which means
                        // at least one posted entry, in a period it can
                        // then close.
                        let journal_id = registry_journal(db, &company).await?;
                        common::open_period(db, &company, &format!("{label}-04")).await;
                        let entry_id = format!("ent-registry-ye-{label}");
                        posting::post_entry(
                            db,
                            &registry_entry(
                                &company,
                                &journal_id,
                                &entry_id,
                                i32::try_from(label).expect("a small label"),
                            ),
                            &posting::PostContext {
                                fiscal_year: label,
                                entry_id: entry_id.clone(),
                                recorded_at: POSTED_AT.to_owned(),
                                outbox_id: format!("{entry_id}-outbox"),
                            },
                            ledgers_db::RetryPolicy::default(),
                            |_| {},
                            &person(),
                        )
                        .await
                        .map_err(|err| err.to_string())?;
                        // Every period but the last must be closed, and
                        // the year must have a successor to carry the
                        // opening balances into.
                        for month in 1..=12 {
                            if month == 12 {
                                continue;
                            }
                            period::close_period(
                                db,
                                &company,
                                &format!("{label}-{month:02}"),
                                &EntryJournal,
                                &CloseCheckRegistry::new(),
                                None,
                                NOW.to_owned(),
                                &person(),
                            )
                            .await
                            .map_err(|err| err.to_string())?;
                        }
                        period::close_fiscal_year(
                            db,
                            &company,
                            label,
                            &EntryJournal,
                            &CloseCheckRegistry::new(),
                            None,
                            NOW.to_owned(),
                            &person(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                    })
                    .await;
                }
                (other, _) => panic!("no registry case for {other}"),
            }
        }

        // The chain this test produced is a real one.
        let verified = verify_db(db, company)
            .await
            .unwrap_or_else(|err| panic!("the chain does not verify: {err}"));
        assert!(verified.events > 0, "the run produced events at all");
    });
}

/// A per-case counter so two registry cases never collide on an id.
fn counter() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Opens a year and its successor on a company of its own, so a close
/// has somewhere to carry the opening balances. Returns the company and
/// the label.
async fn registry_year(db: &dyn Database, label: i64) -> (String, i64) {
    let company = format!("{COMPANY_ID}-r{label}");
    common::mount(db, &company).await;
    // The year-end close needs one leaf carrying the retained-earnings
    // tag to carry the result into; the shared scenario chart has none.
    commands::create_account(
        db,
        &company,
        AccountDraft {
            number: "2300".to_owned(),
            name_key: "account.minimal.retained-earnings".to_owned(),
            account_type: AccountType::Equity,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec![ledgers_ledger::fiscal::RETAINED_EARNINGS_TAG.to_owned()],
        },
        NOW.to_owned(),
        &common::person(),
    )
    .await
    .unwrap_or_else(|err| panic!("the retained-earnings account for {company}: {err}"));
    for year in [label, label + 1] {
        period::open_fiscal_year(
            db,
            &company,
            FiscalYearSpec {
                label: year,
                start_month: 1,
                has_adjustment_period: false,
            },
            NOW.to_owned(),
            &common::person(),
        )
        .await
        .unwrap_or_else(|err| panic!("open fiscal year {year} for {company}: {err}"));
    }
    (company, label)
}

/// A journal the posting cases can post into, created once.
async fn registry_journal(db: &dyn Database, company: &str) -> Result<String, String> {
    let id = format!("jrn-registry-post-{}-{}", counter(), counter());
    ledgers_ledger::journal::create_journal(
        db,
        company,
        JournalDraft {
            code: format!("RGP{}", counter()),
            kind: JournalKind::General,
            name: "Registry posting journal".to_owned(),
        },
        &id,
        POSTED_AT,
        &common::person(),
    )
    .await
    .map(|_| id)
    .map_err(|err| err.to_string())
}

/// One balanced two-line entry against the shared chart.
fn registry_entry(company: &str, journal_id: &str, entry_id: &str, year: i32) -> Entry {
    let eur = Currency::from_code("EUR").expect("EUR is in the table");
    let date = time::Date::from_calendar_date(year, time::Month::April, 10).expect("a date");
    Entry::from_draft(
        EntryDraft {
            journal_id: journal_id.to_owned(),
            document_date: date,
            posting_date: date,
            tax_point_date: None,
            base_currency: eur,
            description: "Registry post".to_owned(),
            source_documents: vec![format!("registry:{entry_id}")],
            external_id: None,
            source_app: None,
            reverses: None,
            lines: vec![
                Line {
                    account_number: "1010".to_owned(),
                    side: Side::Debit,
                    base: Amount::new(1000, eur),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
                Line {
                    account_number: "4000".to_owned(),
                    side: Side::Credit,
                    base: Amount::new(1000, eur),
                    fx: None,
                    tax_code: None,
                    dimensions: std::collections::BTreeMap::new(),
                    memo: None,
                },
            ],
        },
        company,
    )
    .expect("the fixture validates")
}

/// A minimal chart template carrying every required system tag, for the
/// import case: one leaf per tag, no parents, so the lint passes and the
/// post-import "exactly one carrier per tag" check has something to
/// count.
fn registry_template() -> ledgers_ledger::chart::ChartTemplate {
    ledgers_ledger::chart::ChartTemplate {
        id: "registry-sme".to_owned(),
        name: "template.registry.name".to_owned(),
        version: 1,
        accounts: ledgers_ledger::lint::REQUIRED_SYSTEM_TAGS
            .iter()
            .enumerate()
            .map(|(index, tag)| ledgers_ledger::chart::ChartAccount {
                number: format!("9{index:03}0"),
                name: format!("account.inline.{tag}"),
                account_type: AccountType::Asset,
                normal_balance: ledgers_ledger::account::NormalBalance::Debit,
                parent: None,
                currency: None,
                tags: vec![(*tag).to_owned()],
                ebilanz: None,
            })
            .collect(),
    }
}
