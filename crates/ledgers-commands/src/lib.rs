#![forbid(unsafe_code)]
//! The command registry and executor: a unified interface for all
//! operations across CLI, MCP, and HTTP (docs/PLAN.md §4).
//!
//! Every operation is one [`Command`] defined once: name, input and output
//! schemas, the policy tier it needs, whether it moves money, and whether
//! it supports a dry run. The executor runs with authentication, policy
//! checks, and audit logging in one shape — generating the CLI, MCP tools,
//! HTTP routes and the app's command palette from the same definition.
//!
//! The lifecycle of a command is:
//!
//! 1. **Authentication** — a credential yields a [`Principal`] or an error.
//! 2. **Preview** — what the command would do, dry-run only for now.
//! 3. **Policy decision** — is the principal allowed? An [`Executor`] holds
//!    the [`Policy`] that decides.
//! 4. **Execution** (if allowed) — the command writes to the database,
//!    reads back the audit head (in case of retry), and appends the audit
//!    event in one [`batch_atomic`] with the change.
//!
//! A preview that returns an empty [`JournalPreview`] (no debit/credit
//! lines) means the command moves no money. The policy engine uses the
//! preview to check agent limits; a dry run stops after preview. Denied
//! commands record a `Denied` audit event (separate batch). Approvals and
//! passkey step-up are out of scope (separate issue) — documented that.
//!
//! Nothing in this crate depends on [`ledgers-ledger`] — that dependency
//! points the other way in phase 2.

pub mod auth;
pub mod command;
pub mod error;
pub mod exec;
pub mod journal_preview;
pub mod policy;
pub mod registry;

pub use auth::{Authenticator, Credential, Limit, Limits, Principal, Tier};
pub use command::{Command, CommandContext, Plan};
pub use error::{CommandError, PROBLEM_BASE};
pub use exec::{ExecToken, Execute, Executed, Executor};
pub use journal_preview::JournalPreview;
pub use policy::{Decision, DefaultPolicy, Policy};
pub use registry::{CommandDescriptor, CommandMeta, Registry};
