#![forbid(unsafe_code)]
//! Authentication, identity, and session management.
//!
//! This crate provides:
//! - **Agent key management** via `keys`: issue, rotate, revoke agent keys,
//!   and authenticate bearer credentials through the harness [`cratefield_core::ApiKeys`].
//! - **Identity provider trait** via `identity`: the extension point for
//!   plugging in cloud SSO or custom person authentication.
//! - **People session primitives** via `session`: short-lived, secure cookies
//!   for authenticated persons.
//!
//! All flows are wasm-clean (no OS entropy or I/O in the lib itself).

pub mod identity;
pub mod keys;
pub mod session;

pub use identity::{IdentityProvider, OwnerPasskeys};
pub use keys::{
    AgentKeyDirectory, AgentKeyIssue, AgentKeys, Clock, KeyAuthenticator, KeyDirectoryEntry,
};
pub use session::SessionBuilder;
