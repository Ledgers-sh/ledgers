//! The D1 migration set (`d1/migrations`, applied by
//! `wrangler d1 migrations apply`) is the native schema.
//!
//! The files are the SQLite migrations the composition's modules declare. A
//! file keeps its number once it exists: a deployed D1 database has applied
//! it, so numbers are never reused or shifted. A migration with no file yet
//! takes the next free number, after the existing ones. Two checks keep them
//! honest:
//!
//! - the files on disk are byte-for-byte the modules' migrations, numbered as
//!   above;
//! - applying the files to SQLite gives the same schema (`sqlite_master`) as
//!   the native path applying the modules.
//!
//! Regenerate the files after a module gains a migration with
//! `LEDGERS_WRITE_D1_MIGRATIONS=1 cargo test -p ledgers-server --test d1_migrations`.

use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::{Config, Database, Harness};
use cratefield_runtime_native::{DirBlob, Native};
use ledgers_server::{dispatch::registry, harness};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The Worker's D1 migrations directory (`migrations_dir` in wrangler.toml).
const D1_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/d1/migrations");

/// A company id, so the composition builds without development mode.
struct CompanyConfig;

impl Config for CompanyConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "LEDGERS_COMPANY_ID" => Some("01TESTCOMPANY0000000000000".to_owned()),
            "LEDGERS_DOCUMENT_SIGNING_KEY" => {
                Some("d1-test-document-signing-key-0123456789".to_owned())
            }
            _ => None,
        }
    }
}

/// One planned D1 migration: its key (`module_migration`, the file name
/// without its number) and the SQL it must contain.
struct Planned {
    key: String,
    sql: &'static str,
}

/// The key of a numbered file: `0014_ledgers_policy_agent_key.sql` is
/// `ledgers_policy_agent_key`.
fn key_of(file: &str) -> &str {
    file.split_once('_')
        .map_or(file, |(_, key)| key)
        .trim_end_matches(".sql")
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The SQLite migrations of every module, in the order the native server
/// applies them.
fn planned(harness: &Harness) -> Vec<Planned> {
    let mut out = Vec::new();
    for module in harness.modules() {
        for migration in module.migrations().sqlite {
            out.push(Planned {
                key: format!("{}_{}", slug(module.name()), slug(migration.name)),
                sql: migration.sql,
            });
        }
    }
    out
}

/// The file each planned migration must be, with its number: a file already
/// on disk keeps its number, a new migration takes the next one.
fn expected(plan: &[Planned], on_disk: &[PathBuf]) -> Vec<(String, &'static str)> {
    let names: Vec<String> = on_disk
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let mut next = names
        .iter()
        .filter_map(|name| name.get(..4)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let mut out: Vec<(String, &'static str)> = plan
        .iter()
        .map(|step| {
            let kept = names.iter().find(|name| key_of(name) == step.key);
            let file = kept.cloned().unwrap_or_else(|| {
                let file = format!("{next:04}_{}.sql", step.key);
                next += 1;
                file
            });
            (file, step.sql)
        })
        .collect();
    out.sort();
    out
}

fn build(db: Arc<SqliteDatabase>, blob_dir: &str) -> Harness {
    let native = Native::new()
        .db_arc(db as Arc<dyn Database>)
        .blob_arc(DirBlob::arc(blob_dir));
    harness(&registry(), &CompanyConfig, native).expect("the composition builds")
}

fn files_on_disk() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(D1_DIR)
        .expect("the D1 migrations directory exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    files
}

#[test]
fn d1_files_are_the_module_migrations_in_order() {
    let dir = Path::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/d1-blobs"));
    let db = Arc::new(SqliteDatabase::in_memory().expect("in-memory sqlite"));
    let plan = planned(&build(db, &dir.display().to_string()));
    assert!(!plan.is_empty(), "the modules declare SQLite migrations");

    if std::env::var("LEDGERS_WRITE_D1_MIGRATIONS").as_deref() == Ok("1") {
        let wanted = expected(&plan, &files_on_disk());
        for old in files_on_disk() {
            let name = old.file_name().unwrap().to_string_lossy().into_owned();
            if !wanted.iter().any(|(file, _)| *file == name) {
                fs::remove_file(old).expect("remove a stale migration");
            }
        }
        for (file, sql) in &wanted {
            fs::write(Path::new(D1_DIR).join(file), sql).expect("write a D1 migration");
        }
    }

    let on_disk = files_on_disk();
    let wanted = expected(&plan, &on_disk);
    let names: Vec<String> = on_disk
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let wanted_names: Vec<String> = wanted.iter().map(|(file, _)| file.clone()).collect();
    assert_eq!(
        names, wanted_names,
        "the D1 files differ from the modules' migrations; regenerate with LEDGERS_WRITE_D1_MIGRATIONS=1"
    );
    for (path, (file, sql)) in on_disk.iter().zip(&wanted) {
        let text = fs::read_to_string(path).expect("readable migration");
        assert_eq!(
            &text, sql,
            "{file} is not the module's migration; regenerate with LEDGERS_WRITE_D1_MIGRATIONS=1"
        );
    }
}

type SchemaRow = (String, String, String, Option<String>);

const SCHEMA_QUERY: &str = "SELECT type, name, tbl_name, sql FROM sqlite_master \
     WHERE name NOT LIKE 'harness_migrations%' AND name NOT LIKE 'sqlite_%' \
     ORDER BY type, name";

#[test]
fn d1_schema_equals_the_native_schema() {
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let native_path = target.join("native-schema.db");
    let _ = fs::remove_file(&native_path);
    let blobs = target.join("schema-blobs");

    // The native path: the modules' migrations, applied as `native::run` does.
    let native_db = Arc::new(
        SqliteDatabase::open(&native_path.display().to_string()).expect("native database"),
    );
    let composition = build(native_db.clone(), &blobs.display().to_string());
    for module in composition.modules() {
        native_db
            .apply_migrations(module.name(), module.migrations().sqlite)
            .expect("native migrations apply");
    }
    drop(native_db);
    let native = rusqlite::Connection::open(&native_path).expect("reopen native database");
    let native_schema = schema(&native);

    // The D1 path: the files, applied in order.
    let d1 = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
    for path in files_on_disk() {
        let sql = fs::read_to_string(&path).expect("readable migration");
        d1.execute_batch(&sql)
            .unwrap_or_else(|err| panic!("{} fails on SQLite: {err}", path.display()));
    }
    let d1_schema = schema(&d1);

    assert!(
        native_schema.iter().any(|row| row.1 == "company"),
        "the native schema has the company table"
    );
    assert_eq!(
        d1_schema, native_schema,
        "D1 schema differs from the native schema"
    );
    let _ = fs::remove_file(&native_path);
}

fn schema(conn: &rusqlite::Connection) -> Vec<SchemaRow> {
    let mut stmt = conn.prepare(SCHEMA_QUERY).expect("schema query");
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })
    .expect("schema rows")
    .collect::<Result<Vec<_>, _>>()
    .expect("schema row values")
}
