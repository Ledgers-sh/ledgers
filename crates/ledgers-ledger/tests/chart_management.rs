//! Chart management rules (accounts.create / update / archive / restore):
//! numbers and uniqueness, class and parent rules, custom names against
//! i18n names, the zero-balance archive rule, the posting refusal for
//! archived accounts, and the before/after record every change leaves.
//!
//! Each case runs on every available dialect through the harness kit.

mod common;

use common::{COMPANY_ID, NOW, count, open_period, person};
use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_db::{DbModule, RetryPolicy};
use ledgers_ledger::account::{Account, AccountDraft, AccountType, NormalBalance};
use ledgers_ledger::commands::{self, AccountChanges};
use ledgers_ledger::entry::Side;
use ledgers_ledger::i18n::Locale;
use ledgers_ledger::journal::{self, JournalDraft, JournalKind};
use ledgers_ledger::posting::{PostContext, post_entry};
use ledgers_ledger::{AccountsError, Entry, EntryDraft, JournalError, LedgerModule, Line};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use time::{Date, Month};

const APRIL: &str = "2026-04";

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the table")
}

fn eur_major(major: i128) -> Amount {
    Amount::from_major(major, eur()).expect("a representable amount")
}

fn draft(number: &str, name_key: &str, account_type: AccountType) -> AccountDraft {
    AccountDraft {
        number: number.to_owned(),
        name_key: name_key.to_owned(),
        account_type,
        normal_balance: None,
        currency: None,
        parent: None,
        tags: Vec::new(),
        display_name: None,
    }
}

fn harness_factory() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![
        Box::new(ledgers_storage::StorageModule),
        Box::new(LedgerModule),
        Box::new(DbModule),
    ]
}

/// A `#[test]` that runs `case` on every available dialect.
macro_rules! on_every_dialect {
    ($test:ident, $case:ident) => {
        #[test]
        fn $test() {
            for kit in TestHarness::all_dialects(harness_factory) {
                pollster::block_on($case(&*kit.db));
            }
        }
    };
}

/// The company, a small chart with one template account whose name is an
/// i18n key (1010 cash, under the 1000 heading, from the generic chart) and
/// an income account, the period, and the scenario journal.
async fn fixtures(db: &dyn Database) -> String {
    db.execute(&Statement::new(format!(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) \
         VALUES ('{COMPANY_ID}', 'Test Company', 'EUR', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )))
    .await
    .expect("create the company");
    let heading = draft(
        "1000",
        "account.generic_sme.current_assets",
        AccountType::Asset,
    );
    commands::create_account(db, COMPANY_ID, heading, NOW.to_owned(), &person())
        .await
        .expect("the heading");
    let mut cash = draft("1010", "account.generic_sme.cash", AccountType::Asset);
    cash.parent = Some("1000".to_owned());
    commands::create_account(db, COMPANY_ID, cash, NOW.to_owned(), &person())
        .await
        .expect("the template cash account");
    commands::create_account(
        db,
        COMPANY_ID,
        draft("4000", "account.inline.sales", AccountType::Income),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("income");
    open_period(db, COMPANY_ID, APRIL).await;
    let journal = journal::create_journal(
        db,
        COMPANY_ID,
        JournalDraft {
            code: "GC".to_owned(),
            kind: JournalKind::General,
            name: "General journal".to_owned(),
        },
        "01JGCCHART00000000000000000",
        NOW,
        &person(),
    )
    .await
    .expect("create the journal");
    journal.id
}

fn april(tenth: u8) -> Date {
    Date::from_calendar_date(2026, Month::April, tenth).expect("a calendar date")
}

/// A balanced entry moving `major` euros from `debit` to `credit`.
fn transfer(journal_id: &str, debit: &str, credit: &str, major: i128, tag: &str) -> EntryDraft {
    let line = |account: &str, side: Side| Line {
        account_number: account.to_owned(),
        side,
        base: eur_major(major),
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    };
    EntryDraft {
        journal_id: journal_id.to_owned(),
        document_date: april(10),
        posting_date: april(10),
        tax_point_date: None,
        base_currency: eur(),
        description: format!("chart case {tag}"),
        source_documents: Vec::new(),
        external_id: None,
        source_app: None,
        reverses: None,
        lines: vec![line(debit, Side::Debit), line(credit, Side::Credit)],
    }
}

async fn post(db: &dyn Database, draft: EntryDraft, tag: &str) -> Result<(), JournalError> {
    let entry = Entry::from_draft(draft, COMPANY_ID).expect("the draft validates");
    post_entry(
        db,
        &entry,
        &PostContext {
            fiscal_year: 2026,
            entry_id: format!("01ENTRYCHART{tag}00000000000000000"),
            recorded_at: "2026-04-10T10:00:00Z".to_owned(),
            outbox_id: format!("01OUTBOXCHART{tag}00000000000000000"),
        },
        RetryPolicy::default(),
        |_| {},
        &person(),
    )
    .await
    .map(|_| ())
}

/// The chart-change rows of one account, oldest first:
/// `(command, actor, before_json, after_json)`.
async fn changes_of(
    db: &dyn Database,
    number: &str,
) -> Vec<(String, String, Option<String>, String)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT command, actor, before_json, after_json FROM account_change \
             WHERE company_id = ? AND number = ? ORDER BY changed_at, event_id"
                .to_owned(),
            vec![SeaValue::from(COMPANY_ID), SeaValue::from(number)],
        ))
        .await
        .expect("read the change rows");
    rows.rows
        .iter()
        .map(|row| {
            (
                row.get::<String>("command").expect("command"),
                row.get::<String>("actor").expect("actor"),
                row.get::<Option<String>>("before_json").flatten(),
                row.get::<String>("after_json").expect("after"),
            )
        })
        .collect()
}

on_every_dialect!(
    create_refuses_a_duplicate_number_and_a_bad_number,
    create_refuses_a_duplicate_number_and_a_bad_number_case
);

async fn create_refuses_a_duplicate_number_and_a_bad_number_case(db: &dyn Database) {
    fixtures(db).await;
    let duplicate = commands::create_account(
        db,
        COMPANY_ID,
        draft("1010", "account.inline.dup", AccountType::Asset),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("1010 already exists");
    assert_eq!(
        duplicate,
        AccountsError::AccountExists {
            number: "1010".to_owned()
        }
    );
    let bad = commands::create_account(
        db,
        COMPANY_ID,
        draft("1", "account.inline.bad", AccountType::Asset),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("a one-digit number is not a valid account number");
    assert!(matches!(bad, AccountsError::InvalidNumber { .. }), "{bad}");
}

on_every_dialect!(
    a_child_shares_its_parents_class_and_a_parent_with_postings_takes_no_children,
    a_child_shares_its_parents_class_and_a_parent_with_postings_takes_no_children_case
);

async fn a_child_shares_its_parents_class_and_a_parent_with_postings_takes_no_children_case(
    db: &dyn Database,
) {
    let journal_id = fixtures(db).await;
    // Class mismatch: a liability cannot sit under an asset heading.
    let mut mismatched = draft("1050", "account.inline.loan", AccountType::Liability);
    mismatched.parent = Some("1000".to_owned());
    let err = commands::create_account(db, COMPANY_ID, mismatched, NOW.to_owned(), &person())
        .await
        .expect_err("a liability under an asset is refused");
    assert!(
        matches!(err, AccountsError::ParentTypeMismatch { .. }),
        "{err}"
    );

    // A leaf with postings: once it holds a balance it cannot take a child.
    post(db, transfer(&journal_id, "1010", "4000", 5, "parent"), "p1")
        .await
        .expect("the bank account takes a posting");
    let mut child = draft("1011", "account.inline.sub-bank", AccountType::Asset);
    child.parent = Some("1010".to_owned());
    let err = commands::create_account(db, COMPANY_ID, child, NOW.to_owned(), &person())
        .await
        .expect_err("1010 already holds postings");
    assert_eq!(
        err,
        AccountsError::ParentHasPostings {
            parent: "1010".to_owned()
        }
    );
}

on_every_dialect!(
    custom_names_are_stored_and_win_over_the_i18n_name_in_every_language,
    custom_names_are_stored_and_win_over_the_i18n_name_in_every_language_case
);

async fn custom_names_are_stored_and_win_over_the_i18n_name_in_every_language_case(
    db: &dyn Database,
) {
    fixtures(db).await;
    // A custom account: its name is the one the person typed.
    let mut custom = draft(
        "1500",
        &Account::custom_name_key("1500"),
        AccountType::Asset,
    );
    custom.display_name = Some("  Petty cash box  ".to_owned());
    let entry = commands::create_account(db, COMPANY_ID, custom, NOW.to_owned(), &person())
        .await
        .expect("create a custom account");
    assert_eq!(
        entry.account.display_name.as_deref(),
        Some("Petty cash box")
    );
    assert_eq!(entry.account.label(Locale::En), "Petty cash box");
    assert_eq!(entry.account.label(Locale::De), "Petty cash box");

    // A template account keeps its i18n name until someone renames it.
    let template = commands::show_account(db, COMPANY_ID, "1010")
        .await
        .expect("the template cash account");
    assert_eq!(template.account.display_name, None);
    assert_eq!(template.account.label(Locale::En), "Cash");
    assert_eq!(template.account.label(Locale::De), "Kasse");

    let renamed = commands::update_account(
        db,
        COMPANY_ID,
        "1010",
        AccountChanges {
            display_name: Some("Front till".to_owned()),
            ..AccountChanges::default()
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("rename the template account");
    for locale in Locale::all() {
        assert_eq!(
            renamed.account.label(locale),
            "Front till",
            "the custom name shows in {locale:?}"
        );
    }
    assert_eq!(
        renamed.account.name_key, "account.generic_sme.cash",
        "the translatable key is kept, so the i18n name is still there for a reset"
    );
}

#[test]
fn blank_and_control_character_names_are_refused() {
    assert!(matches!(
        Account::validate_display_name("   "),
        Err(AccountsError::InvalidDisplayName { .. })
    ));
    assert!(matches!(
        Account::validate_display_name("bad\u{0007}name"),
        Err(AccountsError::InvalidDisplayName { .. })
    ));
    let long = "x".repeat(121);
    assert!(matches!(
        Account::validate_display_name(&long),
        Err(AccountsError::InvalidDisplayName { .. })
    ));
    assert_eq!(
        Account::validate_display_name(&"x".repeat(120))
            .expect("120 is fine")
            .len(),
        120
    );
}

on_every_dialect!(
    an_update_records_before_and_after_with_the_actor_and_changes_nothing_else,
    an_update_records_before_and_after_with_the_actor_and_changes_nothing_else_case
);

async fn an_update_records_before_and_after_with_the_actor_and_changes_nothing_else_case(
    db: &dyn Database,
) {
    fixtures(db).await;
    commands::create_account(
        db,
        COMPANY_ID,
        draft(
            "1500",
            &Account::custom_name_key("1500"),
            AccountType::Asset,
        ),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("create");
    commands::update_account(
        db,
        COMPANY_ID,
        "1500",
        AccountChanges {
            display_name: Some("Till".to_owned()),
            tags: Some(vec!["cash".to_owned()]),
            ..AccountChanges::default()
        },
        "2026-04-11T09:00:00Z".to_owned(),
        &person(),
    )
    .await
    .expect("rename and tag");
    // A call that changes nothing writes no row.
    commands::update_account(
        db,
        COMPANY_ID,
        "1500",
        AccountChanges {
            display_name: Some("Till".to_owned()),
            ..AccountChanges::default()
        },
        "2026-04-12T09:00:00Z".to_owned(),
        &person(),
    )
    .await
    .expect("the same name again");

    let rows = changes_of(db, "1500").await;
    assert_eq!(rows.len(), 2, "one create and one update: {rows:?}");
    let (command, actor, before, after) = &rows[1];
    assert_eq!(command, "account.update");
    assert!(
        actor.starts_with("person:"),
        "the actor is recorded: {actor}"
    );
    let before: serde_json::Value =
        serde_json::from_str(before.as_deref().expect("an update has a before")).expect("json");
    let after: serde_json::Value = serde_json::from_str(after).expect("json");
    assert_eq!(
        before["displayName"],
        serde_json::Value::String("Petty".into())
            .clone()
            .as_str()
            .map_or(serde_json::Value::Null, |_| serde_json::Value::Null)
    );
    assert_eq!(after["displayName"], "Till");
    assert_eq!(after["tags"], serde_json::json!(["cash"]));
    assert_eq!(after["accountType"], "asset", "the class is not touched");
}

on_every_dialect!(
    the_class_is_fixed_once_an_account_has_postings_or_children,
    the_class_is_fixed_once_an_account_has_postings_or_children_case
);

async fn the_class_is_fixed_once_an_account_has_postings_or_children_case(db: &dyn Database) {
    let journal_id = fixtures(db).await;
    commands::create_account(
        db,
        COMPANY_ID,
        draft("1600", "account.inline.spare", AccountType::Asset),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("create");
    // Unposted, a class change is allowed and the normal balance follows.
    let moved = commands::update_account(
        db,
        COMPANY_ID,
        "1600",
        AccountChanges {
            account_type: Some(AccountType::Liability),
            ..AccountChanges::default()
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("an unposted account may change class");
    assert_eq!(moved.account.account_type, AccountType::Liability);
    assert_eq!(moved.account.normal_balance, NormalBalance::Credit);

    // Once it carries a posting, the class is fixed.
    post(db, transfer(&journal_id, "1010", "1600", 3, "class"), "c1")
        .await
        .expect("post to the liability");
    let err = commands::update_account(
        db,
        COMPANY_ID,
        "1600",
        AccountChanges {
            account_type: Some(AccountType::Asset),
            ..AccountChanges::default()
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("the class is fixed after postings");
    assert_eq!(
        err,
        AccountsError::TypeChangeRefused {
            number: "1600".to_owned(),
            reason: "postings",
        }
    );
}

on_every_dialect!(
    an_account_cannot_be_moved_under_its_own_descendant,
    an_account_cannot_be_moved_under_its_own_descendant_case
);

async fn an_account_cannot_be_moved_under_its_own_descendant_case(db: &dyn Database) {
    fixtures(db).await;
    let mut parent = draft("1700", "account.inline.group", AccountType::Asset);
    parent.tags = Vec::new();
    commands::create_account(db, COMPANY_ID, parent, NOW.to_owned(), &person())
        .await
        .expect("group");
    let mut child = draft("1710", "account.inline.member", AccountType::Asset);
    child.parent = Some("1700".to_owned());
    commands::create_account(db, COMPANY_ID, child, NOW.to_owned(), &person())
        .await
        .expect("member under group");
    let err = commands::update_account(
        db,
        COMPANY_ID,
        "1700",
        AccountChanges {
            parent: Some(Some("1710".to_owned())),
            ..AccountChanges::default()
        },
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("the group cannot move under its member");
    assert!(matches!(err, AccountsError::ParentCycle { .. }), "{err}");
}

on_every_dialect!(
    archive_needs_a_zero_balance_and_a_posting_to_an_archived_account_is_refused,
    archive_needs_a_zero_balance_and_a_posting_to_an_archived_account_is_refused_case
);

async fn archive_needs_a_zero_balance_and_a_posting_to_an_archived_account_is_refused_case(
    db: &dyn Database,
) {
    let journal_id = fixtures(db).await;
    commands::create_account(
        db,
        COMPANY_ID,
        draft("3500", "account.inline.spare", AccountType::Expense),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("create the spare expense account");
    post(db, transfer(&journal_id, "3500", "4000", 5, "arch1"), "a1")
        .await
        .expect("post to the spare account");

    // A non-zero balance refuses the archive, and names the account.
    let err = commands::archive_account(db, COMPANY_ID, "3500", NOW.to_owned(), &person())
        .await
        .expect_err("a balance blocks archiving");
    assert_eq!(
        err,
        AccountsError::AccountHasBalance {
            number: "3500".to_owned()
        }
    );
    assert_eq!(
        count(db, "account_change", "command", "account.archive").await,
        0,
        "a refused archive writes no change row"
    );

    // A reversal brings the account to zero; now it archives.
    post(db, transfer(&journal_id, "4000", "3500", 5, "arch2"), "a2")
        .await
        .expect("net the spare account back to zero");
    let archived = commands::archive_account(db, COMPANY_ID, "3500", NOW.to_owned(), &person())
        .await
        .expect("a settled account archives");
    assert!(archived.account.archived);

    // Retired from posting, with the reason named.
    let refused = post(db, transfer(&journal_id, "3500", "4000", 1, "arch3"), "a3")
        .await
        .expect_err("an archived account takes no postings");
    assert_eq!(
        refused,
        JournalError::AccountArchived {
            account: "3500".to_owned()
        }
    );

    // History stays in the books: the account still appears in the
    // balances and its entries are still listed.
    let balances = ledgers_ledger::query::account_balances(db, COMPANY_ID)
        .await
        .expect("balances");
    assert!(
        balances.iter().any(|b| b.account_number == "3500"),
        "the archived account still reports its history"
    );
    let shown = commands::list_accounts(db, COMPANY_ID, true)
        .await
        .expect("list");
    assert!(
        shown
            .iter()
            .any(|e| e.account.number == "3500" && e.account.archived)
    );
    let active = commands::list_accounts(db, COMPANY_ID, false)
        .await
        .expect("list active");
    assert!(!active.iter().any(|e| e.account.number == "3500"));

    // Restoring it lets postings through again, and records the change.
    let restored = commands::restore_account(db, COMPANY_ID, "3500", NOW.to_owned(), &person())
        .await
        .expect("restore");
    assert!(!restored.account.archived);
    post(db, transfer(&journal_id, "3500", "4000", 2, "arch4"), "a4")
        .await
        .expect("a restored account posts again");

    let commands_seen: Vec<String> = changes_of(db, "3500")
        .await
        .into_iter()
        .map(|row| row.0)
        .collect();
    assert_eq!(
        commands_seen,
        vec!["account.create", "account.archive", "account.restore"],
        "every change is recorded, the refused archive is not"
    );
}

on_every_dialect!(
    an_unknown_account_is_reported_as_not_found,
    an_unknown_account_is_reported_as_not_found_case
);

async fn an_unknown_account_is_reported_as_not_found_case(db: &dyn Database) {
    fixtures(db).await;
    let err = commands::archive_account(db, COMPANY_ID, "9999", NOW.to_owned(), &person())
        .await
        .expect_err("no such account");
    assert_eq!(
        err,
        AccountsError::AccountNotFound {
            number: "9999".to_owned()
        }
    );
    let _ = NormalBalance::Debit;
    let _ = AccountChanges::default();
}
