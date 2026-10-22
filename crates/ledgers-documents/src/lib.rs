#![forbid(unsafe_code)]
//! Source documents (docs/PLAN.md section 6, issue #20): uploaded files
//! stored once per SHA-256, with the sniffed type, the uploader and the
//! retention class; signed download links that expire; and the commands
//! that register and read them.
//!
//! - [`sniff`] decides the file type from its bytes, never from the upload's
//!   `Content-Type` or the file name, and refuses what the app cannot read.
//! - [`sign`] makes the short-lived download links (HMAC-SHA256).
//! - [`commands`] holds `documents.register`, `documents.list`, `documents.get`
//!   and `documents.download_url`.

pub mod commands;
pub mod module;
pub mod sign;
pub mod sniff;
pub mod store;

pub use module::DocumentsStoreModule;
pub use store::{Document, RetentionClass};

/// The schema migrations of the document store, in apply order.
pub const SQLITE_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "documents",
        include_str!("../migrations/sqlite/0001_documents.sql"),
    )];

/// The Postgres set: the same portable DDL as SQLite.
pub const POSTGRES_MIGRATIONS: &[cratefield_core::SqlMigration] =
    &[cratefield_core::SqlMigration::new(
        "0001",
        "documents",
        include_str!("../migrations/postgres/0001_documents.sql"),
    )];

const _: () = cratefield_core::assert_migration_set(SQLITE_MIGRATIONS);
const _: () = cratefield_core::assert_migration_set(POSTGRES_MIGRATIONS);
