//! The close-check registry: the extension point that decides whether a
//! period or fiscal year may close — docs/adr/0002-open-core-boundary.md
//! names `CloseCheck` among the open-core traits, with this registry as
//! the working open-source default and the private cloud crate supplying extra
//! checks (segregation rules, group preconditions) at composition time.
//!
//! A check answers one question — **what still blocks this close?** —
//! with a list of [`Blocker`]s: what kind of thing is open, how many,
//! and a link a person can follow. [`gather_blockers`] runs every
//! registered check and aggregates the findings in registration order,
//! so a close report is complete ("every blocker type with counts and
//! links"), never first-failure-wins.
//!
//! The registry ships one open check, [`DraftEntriesCloseCheck`]
//! (unposted draft entries in range). The FX revaluation precondition
//! (#42) will arrive the same way — a registered check, not a special
//! case in the close path. The links are opaque relative routes
//! (`/companies/{id}/entries/drafts?period=...`); the server crate owns
//! the real routes and may re-shape them, which is why nothing here
//! parses a link back.

use crate::error::AccountsError;
use crate::journal_port::{BoxFuture, Journal};

/// One thing standing in a close's way: a kind of unfinished work, how
/// much of it there is, and where to look.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Blocker {
    /// The check that found it (`"ledger.drafts"`).
    pub check_id: String,
    /// The blocker type — a stable, machine-readable name
    /// (`"draft-entries"`).
    pub kind: String,
    /// How many instances the check found.
    pub count: i64,
    /// A relative route a person can open to resolve the blocker.
    pub link: String,
}

/// Everything a [`CloseCheck`] needs: the database, the journal port,
/// the company, and the range being closed.
///
/// `period` is the **period id** the close names — the spelling links
/// and audit payloads carry (`"2026-03"`, or the last period id for a
/// fiscal-year close). `starts_on`/`ends_on` are the inclusive
/// `"YYYY-MM-DD"` **date range** the check should scan: that one
/// period's range for a `close_period`, the whole fiscal-year span for
/// a `close_fiscal_year` (a year-end close must catch a draft dated
/// anywhere in the year, not just in December). `Copy`, so one context
/// can be handed to every registered check in turn.
#[derive(Clone, Copy)]
pub struct CloseCheckContext<'a> {
    /// The database the close runs on.
    pub db: &'a dyn cratefield_core::Database,
    /// The journal port (draft counts, balances).
    pub journal: &'a dyn Journal,
    /// The company being closed.
    pub company_id: &'a str,
    /// The period id the close names (for links and payloads).
    pub period: &'a str,
    /// Inclusive first date of the range to scan.
    pub starts_on: &'a str,
    /// Inclusive last date of the range to scan.
    pub ends_on: &'a str,
}

/// A close precondition. Register implementations on a
/// [`CloseCheckRegistry`]; the close commands run them all through
/// [`gather_blockers`] and refuse (or record an override for) whatever
/// they report.
pub trait CloseCheck: Send + Sync {
    /// The check's stable id (`"ledger.drafts"`), namespacing its
    /// blockers in reports.
    fn id(&self) -> &'static str;

    /// The blockers this check finds for the close `ctx` describes; an
    /// empty vector means "nothing blocking from me". Errors are the
    /// check's own failure to read, never "blocked".
    ///
    /// # Errors
    /// [`AccountsError::Database`] when the check's reads fail.
    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>>;
}

/// The ordered set of close checks: it starts empty and the composition
/// registers what its close must prove — the open default is
/// [`DraftEntriesCloseCheck`], registered visibly at the call site.
#[derive(Default)]
pub struct CloseCheckRegistry {
    checks: Vec<Box<dyn CloseCheck>>,
}

impl CloseCheckRegistry {
    /// An empty registry — a close over it has no blockers by
    /// definition; callers that want the open default register
    /// [`DraftEntriesCloseCheck`] themselves so the default is visible
    /// at the composition site.
    #[must_use]
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// Appends one check, after those already registered — registration
    /// order is the order [`gather_blockers`] reports findings in.
    pub fn register(&mut self, check: Box<dyn CloseCheck>) {
        self.checks.push(check);
    }

    /// The registered checks, in registration order.
    #[must_use]
    pub fn checks(&self) -> &[Box<dyn CloseCheck>] {
        &self.checks
    }
}

impl std::fmt::Debug for CloseCheckRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.checks.iter().map(|check| check.id()).collect();
        f.debug_struct("CloseCheckRegistry")
            .field("checks", &ids)
            .finish()
    }
}

/// Runs every check in `checks` against `ctx` and aggregates the
/// findings in registration order. One check's read failure fails the
/// whole gather — a close that could not ask its preconditions does not
/// proceed on the answers it happened to get.
///
/// # Errors
/// [`AccountsError::Database`] when any check's reads fail.
pub async fn gather_blockers(
    checks: &CloseCheckRegistry,
    ctx: CloseCheckContext<'_>,
) -> Result<Vec<Blocker>, AccountsError> {
    let mut blockers = Vec::new();
    for check in checks.checks() {
        blockers.extend(check.blockers(Clone::clone(&ctx)).await?);
    }
    Ok(blockers)
}

/// The open draft-entries check: a period cannot close while entries
/// are still drafted inside its range — they would either strand or
/// post into a period that is already history. Counts the journal's
/// drafts over `starts_on..=ends_on` and links the drafts list filtered
/// to the close's period.
pub struct DraftEntriesCloseCheck;

impl CloseCheck for DraftEntriesCloseCheck {
    fn id(&self) -> &'static str {
        "ledger.drafts"
    }

    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
        Box::pin(async move {
            let count = ctx
                .journal
                .draft_count(ctx.db, ctx.company_id, ctx.starts_on, ctx.ends_on)
                .await?;
            if count == 0 {
                return Ok(Vec::new());
            }
            Ok(vec![Blocker {
                check_id: self.id().to_owned(),
                kind: "draft-entries".to_owned(),
                count,
                link: format!(
                    "/companies/{}/entries/drafts?period={}",
                    ctx.company_id, ctx.period
                ),
            }])
        })
    }
}

/// What a refused close hands back: the company and period, and every
/// blocker every check found. [`std::fmt::Display`] renders the whole
/// list — one line per blocker type, count and link included — so an
/// agent relaying the refusal to a person loses nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CloseReport {
    /// The company whose close was refused.
    pub company_id: String,
    /// The period id the close named.
    pub period: String,
    /// Every blocker, in check registration order.
    pub blockers: Vec<Blocker>,
}

impl CloseReport {
    /// One line per blocker, `kind (check_id): count — link`, under a
    /// header naming the company and period — the readable form the
    /// error surface shows.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "close of {} for {} blocked by {} finding(s)",
            self.period,
            self.company_id,
            self.blockers.len()
        )];
        for blocker in &self.blockers {
            lines.push(format!(
                "  - {} ({}): {} — {}",
                blocker.kind, blocker.check_id, blocker.count, blocker.link
            ));
        }
        lines.join("\n")
    }
}

impl std::fmt::Display for CloseReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_defaults_to_empty_and_keeps_order() {
        let mut registry = CloseCheckRegistry::new();
        assert!(registry.checks().is_empty());
        registry.register(Box::new(DraftEntriesCloseCheck));
        registry.register(Box::new(StubCheck { id: "stub.two" }));
        let ids: Vec<&str> = registry.checks().iter().map(|c| c.id()).collect();
        assert_eq!(ids, ["ledger.drafts", "stub.two"]);
    }

    #[test]
    fn the_report_lists_every_blocker_with_count_and_link() {
        let report = CloseReport {
            company_id: "test-company-000000000000".to_owned(),
            period: "2026-03".to_owned(),
            blockers: vec![
                Blocker {
                    check_id: "ledger.drafts".to_owned(),
                    kind: "draft-entries".to_owned(),
                    count: 2,
                    link: "/companies/test-company-000000000000/entries/drafts?period=2026-03"
                        .to_owned(),
                },
                Blocker {
                    check_id: "stub.fx".to_owned(),
                    kind: "fx-revaluation".to_owned(),
                    count: 1,
                    link: "/companies/test-company-000000000000/fx?period=2026-03".to_owned(),
                },
            ],
        };
        let text = report.summary();
        assert!(text.contains("draft-entries (ledger.drafts): 2"), "{text}");
        assert!(text.contains("fx-revaluation (stub.fx): 1"), "{text}");
        assert!(
            text.contains("/companies/test-company-000000000000/entries/drafts?period=2026-03"),
            "{text}"
        );
        assert_eq!(format!("{report}"), text, "Display delegates to summary");
    }

    /// A second check shape for the ordering test.
    struct StubCheck {
        id: &'static str,
    }

    impl CloseCheck for StubCheck {
        fn id(&self) -> &'static str {
            self.id
        }

        fn blockers<'a>(
            &'a self,
            _ctx: CloseCheckContext<'a>,
        ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{CloseCheck, CloseCheckContext};

    /// Contract test for `CloseCheck` implementations.
    ///
    /// Verifies that:
    /// - The check has a non-empty, stable id.
    /// - The check can be called against an empty context without error.
    ///
    /// # Panics
    /// Panics if the invariants are violated.
    pub async fn close_check<'a>(check: &'a dyn CloseCheck, ctx: CloseCheckContext<'a>) {
        let id = check.id();
        assert!(!id.is_empty(), "CloseCheck id must be non-empty");

        // Should not panic or error on empty context
        let result = check.blockers(ctx).await;
        assert!(result.is_ok(), "blockers() must not error on empty context");

        let blockers = result.unwrap();
        for blocker in blockers {
            assert!(
                !blocker.check_id.is_empty(),
                "blocker check_id must not be empty"
            );
            assert!(!blocker.kind.is_empty(), "blocker kind must not be empty");
            assert!(blocker.count >= 0, "blocker count must be non-negative");
            assert!(!blocker.link.is_empty(), "blocker link must not be empty");
        }
    }
}
