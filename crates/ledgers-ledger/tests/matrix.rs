//! The account scenario matrix: [`scenarios`] over every backing engine
//! the environment provides — in-memory SQLite always, a throwaway
//! Postgres 16 database when `FZ_TEST_POSTGRES_URL` names one (CI's
//! service container). One test definition, every dialect: the parity
//! loop the testing kit documents, the same shape as
//! `ledgers-storage`'s conformance matrix.

use cratefield_core::Database;
use cratefield_testing::TestHarness;
use ledgers_ledger::account::{AccountDraft, AccountType, NormalBalance};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::{LedgerModule, chart, commands};

/// The single-company fixture, invented per AGENTS.md (never a real firm).
const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const NOW: &str = "2026-02-01T09:00:00Z";
const LATER: &str = "2026-03-01T09:00:00Z";

/// A small lint-clean chart declared inline (child listed before its
/// parent in file order, so passing imports prove real ordering).
const INLINE_CHART: &str = r#"
id = "inline"
name = "template.inline.name"
version = 1

[[accounts]]
number = "1010"
name = "account.inline.bank"
type = "asset"
parent = "1000"
tags = ["bank"]

[[accounts]]
number = "1000"
name = "account.inline.assets"
type = "asset"

[[accounts]]
number = "2000"
name = "account.inline.fx-gain"
type = "income"
tags = ["fx-gain"]

[[accounts]]
number = "3000"
name = "account.inline.fx-loss"
type = "expense"
tags = ["fx-loss"]

[[accounts]]
number = "4000"
name = "account.inline.suspense"
type = "liability"
tags = ["suspense"]

[[accounts]]
number = "5000"
name = "account.inline.retained-earnings"
type = "equity"
tags = ["retained-earnings"]

[[accounts]]
number = "6000"
name = "account.inline.rounding"
type = "expense"
tags = ["rounding"]
"#;

#[test]
fn account_scenarios_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule)]) {
        // The scenarios panic with their own diagnosis on failure. Fresh
        // module instances per dialect: all_dialects runs the factory
        // once per leg.
        pollster::block_on(scenarios(&*kit.db));
    }
}

/// The account lifecycle, end to end over one database: create (with
/// parent checks), find, list ordering, derived postable state, archive
/// and its idempotence, the no-delete contract, and template import
/// (parents first, one shot only).
///
/// # Panics
///
/// Panics when any step returns the wrong result — exactly what a
/// conformance run should surface.
pub async fn scenarios(db: &dyn Database) {
    insert_find_and_derived_state(db).await;
    listing_order_and_archived_filter(db).await;
    archive_is_idempotent(db).await;
    deleting_is_not_an_operation(db).await;
    creating_under_a_missing_parent_fails(db).await;
    imports_are_parents_first_and_one_shot(db).await;
}

async fn insert_find_and_derived_state(db: &dyn Database) {
    let parent = commands::create_account(
        db,
        COMPANY_ID,
        AccountDraft {
            number: "1000".to_owned(),
            name_key: "account.inline.assets".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: vec!["bank".to_owned()],
        },
        NOW.to_owned(),
    )
    .await
    .expect("create the parent account");
    assert!(!parent.has_children, "a fresh account is a leaf");
    assert!(parent.is_postable());
    assert_eq!(parent.account.normal_balance, NormalBalance::Debit);

    let child = commands::create_account(
        db,
        COMPANY_ID,
        AccountDraft {
            number: "1010".to_owned(),
            name_key: "account.inline.bank".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: Some("1000".to_owned()),
            tags: Vec::new(),
        },
        NOW.to_owned(),
    )
    .await
    .expect("create the child account");

    // The derived bit flipped for the parent, with no stored flag to
    // maintain: adding the child made it a heading atomically.
    let re_read = commands::show_account(db, COMPANY_ID, "1000")
        .await
        .expect("the parent still exists");
    assert!(re_read.has_children);
    assert!(re_read.is_heading());
    assert!(!re_read.is_postable());
    assert!(child.is_leaf(), "the child itself is postable");

    // Duplicate numbers are rejected before the primary key has to be.
    let duplicate = commands::create_account(
        db,
        COMPANY_ID,
        AccountDraft {
            number: "1010".to_owned(),
            name_key: "account.inline.bank-2".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: None,
            tags: Vec::new(),
        },
        NOW.to_owned(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        duplicate,
        AccountsError::AccountExists {
            number: "1010".to_owned()
        }
    );

    let absent = commands::show_account(db, COMPANY_ID, "9999")
        .await
        .unwrap_err();
    assert_eq!(
        absent,
        AccountsError::AccountNotFound {
            number: "9999".to_owned()
        }
    );
}

async fn listing_order_and_archived_filter(db: &dyn Database) {
    commands::create_account(
        db,
        COMPANY_ID,
        AccountDraft {
            number: "1020".to_owned(),
            name_key: "account.inline.cash".to_owned(),
            account_type: AccountType::Asset,
            normal_balance: None,
            currency: None,
            parent: Some("1000".to_owned()),
            tags: Vec::new(),
        },
        NOW.to_owned(),
    )
    .await
    .expect("create a third account");

    let all = commands::list_accounts(db, COMPANY_ID, true)
        .await
        .expect("list with archived");
    let numbers: Vec<&str> = all.iter().map(|e| e.account.number.as_str()).collect();
    assert_eq!(numbers, ["1000", "1010", "1020"], "ordered by number");
    assert!(
        all.iter()
            .all(|entry| entry.account.company_id == COMPANY_ID),
        "rows stay company-scoped"
    );

    commands::archive_account(db, COMPANY_ID, "1020", LATER.to_owned())
        .await
        .expect("archive the third account");
    let live = commands::list_accounts(db, COMPANY_ID, false)
        .await
        .expect("list without archived");
    let numbers: Vec<&str> = live.iter().map(|e| e.account.number.as_str()).collect();
    assert_eq!(numbers, ["1000", "1010"]);
    let with_archived = commands::list_accounts(db, COMPANY_ID, true)
        .await
        .expect("list with archived");
    assert_eq!(
        with_archived.len(),
        3,
        "archived rows are kept, not deleted"
    );
}

async fn archive_is_idempotent(db: &dyn Database) {
    let once = commands::archive_account(db, COMPANY_ID, "1020", LATER.to_owned())
        .await
        .expect("archiving an archived account succeeds");
    assert!(once.account.archived);
    let twice = commands::archive_account(db, COMPANY_ID, "1020", LATER.to_owned())
        .await
        .expect("the second archive is idempotent");
    assert_eq!(once, twice, "the account comes back unchanged");

    let missing = commands::archive_account(db, COMPANY_ID, "8888", LATER.to_owned())
        .await
        .unwrap_err();
    assert_eq!(
        missing,
        AccountsError::AccountNotFound {
            number: "8888".to_owned()
        }
    );
}

async fn deleting_is_not_an_operation(db: &dyn Database) {
    let err = commands::delete_account(db, COMPANY_ID, "1010")
        .await
        .unwrap_err();
    assert_eq!(err, AccountsError::DeleteUnsupported);
    // And nothing was removed behind the error.
    let still_there = commands::show_account(db, COMPANY_ID, "1010")
        .await
        .expect("the account survived the delete attempt");
    assert!(!still_there.account.archived);
}

async fn creating_under_a_missing_parent_fails(db: &dyn Database) {
    let err = commands::create_account(
        db,
        COMPANY_ID,
        AccountDraft {
            number: "7000".to_owned(),
            name_key: "account.inline.orphan".to_owned(),
            account_type: AccountType::Expense,
            normal_balance: None,
            currency: None,
            parent: Some("6969".to_owned()),
            tags: Vec::new(),
        },
        NOW.to_owned(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        AccountsError::ParentNotFound {
            parent: "6969".to_owned()
        }
    );
    assert!(
        commands::show_account(db, COMPANY_ID, "7000")
            .await
            .is_err(),
        "the rejected account was never written"
    );
}

async fn imports_are_parents_first_and_one_shot(db: &dyn Database) {
    // A second company so the import lands in a fresh ledger whatever
    // the earlier scenarios wrote (also proof of company scoping).
    const SECOND_COMPANY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB";
    let chart = chart::parse_chart(INLINE_CHART).expect("inline chart parses");
    let report = commands::import_template(db, SECOND_COMPANY, &chart, NOW.to_owned())
        .await
        .expect("import the chart");
    assert_eq!(report.template_id, "inline");
    assert_eq!(
        report.created,
        ["1000", "1010", "2000", "3000", "4000", "5000", "6000"],
        "every account reported, sorted"
    );

    // Parents really landed before children: the child resolves to a
    // parent that exists, and the tree derives as declared.
    let child = commands::show_account(db, SECOND_COMPANY, "1010")
        .await
        .expect("the child was imported");
    assert_eq!(child.account.parent.as_deref(), Some("1000"));
    assert!(child.is_postable());
    let parent = commands::show_account(db, SECOND_COMPANY, "1000")
        .await
        .expect("the parent was imported");
    assert!(parent.is_heading());

    // All five system accounts are in place, as leaves.
    for tag in ledgers_ledger::lint::REQUIRED_SYSTEM_TAGS {
        let carriers: Vec<_> = commands::list_accounts(db, SECOND_COMPANY, true)
            .await
            .expect("list the imported chart")
            .into_iter()
            .filter(|entry| entry.account.has_tag(tag))
            .collect();
        assert_eq!(carriers.len(), 1, "exactly one {tag} account");
        assert!(carriers[0].is_postable(), "the {tag} account is a leaf");
    }

    // Imports are for fresh ledgers: the same chart again is refused.
    let err = commands::import_template(db, SECOND_COMPANY, &chart, LATER.to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(err, AccountsError::AccountExists { .. }),
        "{err:?}"
    );

    // A structurally broken chart never touches the database.
    let mut broken = chart.clone();
    broken.accounts[0].parent = Some("9999".to_owned());
    let err = commands::import_template(db, SECOND_COMPANY, &broken, LATER.to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(err, AccountsError::TemplateRejected { .. }),
        "{err:?}"
    );
}
