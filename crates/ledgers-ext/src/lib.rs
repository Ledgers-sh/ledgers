#![forbid(unsafe_code)]
//! Extension traits for the managed service — open-core interfaces with
//! working defaults, reusable by the cloud at composition time.
//!
//! The core exposes six extension points:
//!
//! - [`scope::EntityScope`]: which company a request acts on, and isolation.
//! - [`meter::UsageMeter`]: count processed documents and metered actions;
//!   `admit()` returns `Allow` or `Pause(reason)`.
//! - [`intake::DocumentIntake`]: where documents come from.
//! - [`bank::BankFeed`]: pull transaction data from a bank.
//! - [`einvoice::EInvoiceTransport`]: send/receive e-invoices.
//! - [`filing::Filing`]: submit tax returns.
//!
//! Each trait has an open default implementation in its module and a
//! contract test suite exported behind the `testing` cargo feature.

pub mod bank;
pub mod einvoice;
pub mod filing;
pub mod intake;
pub mod meter;
pub mod scope;

pub use bank::{BankFeed, BankTransaction, StatementImportOnly};
pub use einvoice::{Delivery, EInvoice, EInvoiceTransport, FileExchange};
pub use filing::{Filing, FilingReceipt, ManualFiling, TaxReturn};
pub use intake::{DocumentIntake, IncomingDocument, IntakeSource, UploadAndFolder};
pub use meter::{Admission, MeteredAction, Unmetered, UsageMeter};
pub use scope::{CompanyId, EntityScope, SingleCompany};
