//! The storage conformance matrix: [`suite::assert_storage_conformance`]
//! over every backing engine the environment provides — in-memory SQLite
//! always, a throwaway Postgres 16 database when `FZ_TEST_POSTGRES_URL`
//! names one (CI's service container). One test definition, every
//! dialect: the parity loop the testing kit documents.
//!
//! Below it, the portable-SQL gate on the migrations themselves: every
//! `migrations/<dialect>/*.sql` under the workspace's crates must pass
//! `cratefield_core::lint_portable_sql` (the same lint `fz doctor` and
//! the Postgres migration runner apply), and the token the plan calls
//! out — `NOW()` — is proven to fail it once, so the gate can never
//! silently rot into a yes.

use cratefield_testing::TestHarness;
use ledgers_storage::{StorageModule, suite};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn storage_conformance_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(StorageModule)]) {
        // The suite panics with its own diagnosis on failure. Fresh module
        // instances per dialect: all_dialects runs the factory once per leg.
        pollster::block_on(suite::assert_storage_conformance(&*kit.db));
    }
}

/// Collects every `migrations/<dialect>/<file>.sql` under `dir`,
/// recursively — the per-crate layout the `fz` CLI collects (a `sqlite`
/// set per crate, a `postgres` override where the SQL truly differs).
fn collect_migration_sql(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_migration_sql(&path, out);
        } else if is_migration_sql(&path) {
            out.push(path);
        }
    }
}

/// Whether `path` is `<crate>/migrations/<dialect>/<file>.sql`: a `.sql`
/// file one directory below a directory called `migrations`.
fn is_migration_sql(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("sql"))
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == "migrations")
}

#[test]
fn migrations_pass_the_portable_sql_lint() {
    // From this test file, `../../crates` is every crate in the workspace:
    // a new crate's migrations join the gate by existing, with nothing to
    // register. std-only on purpose — no glob crate for four directories.
    let crates_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let mut files = Vec::new();
    collect_migration_sql(&crates_root, &mut files);

    // The anti-vacuity guard: a broken walk (relocated root, changed
    // layout) would lint nothing and pass forever.
    assert!(
        files.len() >= 2,
        "found only {} migration .sql files under {} — the walk itself broke, and a \
         silent miss would make this test vacuous",
        files.len(),
        crates_root.display(),
    );

    let mut findings = Vec::new();
    for path in &files {
        let sql = fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        for (token, why) in cratefield_core::lint_portable_sql(&sql) {
            findings.push(format!("{}: `{token}` — {why}", path.display()));
        }
    }
    assert!(
        findings.is_empty(),
        "migrations left the portable-SQL subset (one set must run on SQLite, D1 and \
         Postgres):\n  {}",
        findings.join("\n  "),
    );
}

/// A migration shaped like the mistake the lint exists to catch: a
/// timestamp column whose DEFAULT calls the dialect function `NOW()`.
/// Inline and deliberately not under any `migrations/` directory — it is
/// a fixture, not something the workspace should ever apply.
const NOW_DEFAULT_MIGRATION: &str = "CREATE TABLE example_event (
    id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL DEFAULT NOW()
);
";

/// The same table the Ledgers way: an ISO-8601 `TEXT` timestamp with no
/// DEFAULT, so the value is always bound from code.
const ISO8601_TEXT_MIGRATION: &str = "CREATE TABLE example_event (
    id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL
);
";

#[test]
fn a_now_migration_fails_the_portable_sql_lint() {
    let findings = cratefield_core::lint_portable_sql(NOW_DEFAULT_MIGRATION);
    let Some((_, explanation)) = findings.iter().find(|(token, _)| *token == "NOW()") else {
        panic!(
            "a DEFAULT NOW() migration must fail the portable-SQL lint; if it stopped \
             flagging, every migration could quietly grow dialect defaults. Fix the \
             migration: bind an ISO-8601 UTC timestamp from code instead of calling \
             NOW() in SQL"
        );
    };
    assert!(
        !explanation.is_empty(),
        "the NOW() finding must explain the fix: bind an ISO-8601 timestamp from code"
    );

    // The convention that passes: same table, no DEFAULT — the timestamp
    // arrives as a bound ISO-8601 value, identical on every backend.
    assert!(
        cratefield_core::lint_portable_sql(ISO8601_TEXT_MIGRATION).is_empty(),
        "an ISO-8601 TEXT timestamp column must lint clean; if it does not, the lint \
         is rejecting the documented convention — write the column as plain TEXT and \
         bind ISO-8601 UTC values from code"
    );
}
