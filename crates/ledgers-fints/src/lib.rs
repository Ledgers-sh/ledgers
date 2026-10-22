#![forbid(unsafe_code)]
//! `FinTS` bank feed for Ledgers: read-only PIN/TAN access via fints-rs 0.2.0.
//!
//! See docs/adr/0006-fints-bank-feed.md and docs/bank-sync-fints.md.

mod error;
pub mod feed;
mod pin;

#[cfg(test)]
mod tests;

pub use error::FintsFeedError;
pub use feed::{
    AccountBalance, BankIdentifier, FintsFeed, FintsFeedConfig, ProductIdSource,
    generate_external_id,
};
pub use pin::{DefaultPinSource, PinSource, SecurePin};

#[cfg(feature = "keychain")]
pub use pin::KeychainPinSource;
