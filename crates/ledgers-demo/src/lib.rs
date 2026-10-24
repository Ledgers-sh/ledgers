#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! The demo company fixture (issue #51): Halden Works `GmbH`, an invented
//! German small manufacturer, and one believable fiscal year of books.
//!
//! This crate is data only: fixtures for every milestone's seeded entries,
//! dimension values, and golden totals the reports must reproduce. The server
//! loads fixtures through the same executor path as real commands
//! (`ledgers-server`'s `demo` module). **Every name, number and reference
//! here is invented.** Nothing is real company data.
//!
//! Later milestones add their fixtures to this shared crate, not private test
//! folders, so integration and data integrity remain testable and shared.
//!
//! Amounts are integer minor units of EUR (cents), debit and credit as
//! separate sides, never floats (docs/PLAN.md section 3).

pub mod factory_zero;
pub mod halden;

use std::collections::BTreeMap;

/// Who posts an entry: an agent key or a person. Agent entries touch only
/// accounts an agent may move on its own; the protected accounts (bank,
/// receivables, payables, VAT, retained earnings) are person-posted, as the
/// policy requires (`ledgers-policy`, protected accounts).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Poster {
    /// An agent key with the post-within-limits tier.
    Agent,
    /// The company's owner, through a person session.
    Person,
}

/// The side of a line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// Debit.
    Debit,
    /// Credit.
    Credit,
}

impl Side {
    /// The wire form the entries API uses (`debit` or `credit`).
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Side::Debit => "debit",
            Side::Credit => "credit",
        }
    }

    /// The opposite side, for a reversal's mirror line.
    #[must_use]
    pub fn flip(self) -> Self {
        match self {
            Side::Debit => Side::Credit,
            Side::Credit => Side::Debit,
        }
    }

    /// The sign of this side: debit `+1`, credit `-1`.
    #[must_use]
    pub fn sign(self) -> i64 {
        match self {
            Side::Debit => 1,
            Side::Credit => -1,
        }
    }
}

/// One line of a demo entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoLine {
    /// The account number in the bundled generic SME chart.
    pub account: &'static str,
    /// Debit or credit.
    pub side: Side,
    /// The amount in EUR minor units (cents), always positive.
    pub minor: i64,
    /// The tax code (for example `19-U`), when the line carries one.
    pub tax_code: Option<&'static str>,
    /// Dimension tags: `(kind, code)`, where kind is `cost_centre` or `project`.
    pub dimensions: Vec<(&'static str, &'static str)>,
    /// A memo, when the line has one.
    pub memo: Option<String>,
}

/// What a demo entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A new balanced entry with these lines.
    Post {
        /// The lines; debits and credits of equal total.
        lines: Vec<DemoLine>,
    },
    /// The reversal of the entry with `external_id` `of`. The mirror lines
    /// come from that entry; posted entries are never edited.
    Reverse {
        /// The `external_id` of the entry being reversed.
        of: String,
    },
    /// An invoice or bill posted through the subledger (`subledger.post-item`):
    /// the control line (receivables or payables) and the counter lines, all
    /// in `lines`. The control account comes from `ledger`.
    Item {
        /// `ar` or `ap`.
        ledger: &'static str,
        /// `invoice`, `credit-note` or `bill`.
        kind: &'static str,
        /// The customer or vendor the item belongs to.
        party: &'static str,
        /// The document number (unique per company and kind).
        source_id: String,
        /// The posted lines.
        lines: Vec<DemoLine>,
    },
    /// Payment of one party's items from a bank account, through
    /// `subledger.allocate`. `items` are `(external_id of the item entry,
    /// amount)`; `lines` are the bank and control lines it posts.
    Allocate {
        /// `ar` or `ap`.
        ledger: &'static str,
        /// The customer or vendor paying or paid.
        party: &'static str,
        /// The bank account, `1020` in the demo.
        bank: &'static str,
        /// The items allocated: `(external_id of the item entry, amount)`.
        items: Vec<(String, i64)>,
        /// The posted lines.
        lines: Vec<DemoLine>,
    },
    /// Clears a credit item against a debit item of the same party, through
    /// `subledger.offset`. `credit` and `debit` are the `external_id`s of the
    /// item entries. A link only: with no exchange difference it posts no
    /// journal entry, so it has no lines.
    Offset {
        /// `ar` or `ap`.
        ledger: &'static str,
        /// The customer or vendor.
        party: &'static str,
        /// The `external_id` of the credit item entry.
        credit: String,
        /// The `external_id` of the debit item entry.
        debit: String,
        /// The amount cleared, in minor units.
        amount: i64,
    },
}

impl EntryKind {
    /// The journal lines the entry posts, for every kind that posts its own
    /// lines. `None` for a reversal (its lines come from its target) and for a
    /// link-only offset (it posts none).
    #[must_use]
    pub fn lines(&self) -> Option<&[DemoLine]> {
        match self {
            Self::Post { lines } | Self::Item { lines, .. } | Self::Allocate { lines, .. } => {
                Some(lines)
            }
            Self::Offset { .. } | Self::Reverse { .. } => None,
        }
    }
}

/// One demo entry in posting order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoEntry {
    /// The idempotency key the entry is posted with. Stable across years.
    pub external_id: String,
    /// Who posts it.
    pub poster: Poster,
    /// The posting date, `YYYY-MM-DD`.
    pub posting_date: String,
    /// The description (invented text).
    pub description: String,
    /// What the entry does.
    pub kind: EntryKind,
}

/// A dimension value the company must have before its entries post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoDimension {
    /// `cost_centre` or `project`.
    pub kind: &'static str,
    /// The code (2 to 16 ASCII alphanumerics).
    pub code: &'static str,
    /// The display name.
    pub name: &'static str,
}

/// The whole fixture: the company, its dimension values and its entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoBook {
    /// The company's display name.
    pub company: &'static str,
    /// The base currency code.
    pub currency: &'static str,
    /// The fiscal year the dates fall in.
    pub year: i32,
    /// The dimension values to create before posting.
    pub dimensions: Vec<DemoDimension>,
    /// The business units to create before posting (ledgers-ledger units).
    pub units: Vec<DemoUnit>,
    /// The entries, in the order they post.
    pub entries: Vec<DemoEntry>,
}

/// A business unit the company keeps its own books for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoUnit {
    /// The short code (2 to 16 ASCII alphanumerics).
    pub code: &'static str,
    /// The display name.
    pub name: &'static str,
    /// The colour, `#RRGGBB`.
    pub colour: &'static str,
}

/// The chart accounts an intercompany movement between units uses: the
/// inter-unit receivable (`inter-unit-due-from`) and payable
/// (`inter-unit-due-to`) of the generic SME chart. An entry that touches one
/// of them is intercompany: consolidated views leave it out.
pub const INTERCOMPANY_ACCOUNTS: [&str; 2] = ["1070", "2050"];

/// Whether an entry is intercompany (touches an inter-unit account).
#[must_use]
pub fn is_intercompany(entry: &DemoEntry) -> bool {
    entry.kind.lines().is_some_and(|lines| {
        lines
            .iter()
            .any(|line| INTERCOMPANY_ACCOUNTS.contains(&line.account))
    })
}

/// The signed total of an entry's lines in minor units: debits minus
/// credits. Zero for a balanced entry.
#[must_use]
pub fn balance_of(lines: &[DemoLine]) -> i64 {
    lines.iter().map(|line| line.side.sign() * line.minor).sum()
}

/// The lines a reversal posts: the original's lines with each side flipped.
#[must_use]
pub fn mirror_of(lines: &[DemoLine]) -> Vec<DemoLine> {
    lines
        .iter()
        .map(|line| DemoLine {
            side: line.side.flip(),
            ..line.clone()
        })
        .collect()
}

impl DemoBook {
    /// The lines an entry posts, with reversals resolved to their mirror.
    /// `None` when the entry is a reversal whose target is not in the book.
    #[must_use]
    pub fn posted_lines(&self, entry: &DemoEntry) -> Option<Vec<DemoLine>> {
        match &entry.kind {
            EntryKind::Reverse { of } => {
                let original = self.entry(of)?;
                original.kind.lines().map(mirror_of)
            }
            kind => kind.lines().map(<[DemoLine]>::to_vec),
        }
    }

    /// The entry with this external id.
    #[must_use]
    pub fn entry(&self, external_id: &str) -> Option<&DemoEntry> {
        self.entries
            .iter()
            .find(|entry| entry.external_id == external_id)
    }

    /// Debit-positive balance per account after every entry dated on or
    /// before `through` (`YYYY-MM-DD`). Accounts with a zero balance are
    /// left out.
    #[must_use]
    pub fn balances_through(&self, through: &str) -> BTreeMap<&'static str, i64> {
        self.sum_where(|date| date <= through)
    }

    /// Debit-positive activity per account for the month `YYYY-MM`.
    #[must_use]
    pub fn activity_in_month(&self, month: &str) -> BTreeMap<&'static str, i64> {
        self.sum_where(|date| date.starts_with(month))
    }

    fn sum_where(&self, keep: impl Fn(&str) -> bool) -> BTreeMap<&'static str, i64> {
        let mut totals: BTreeMap<&'static str, i64> = BTreeMap::new();
        for entry in &self.entries {
            // Consolidated: intercompany movements between units net away.
            if !keep(&entry.posting_date) || is_intercompany(entry) {
                continue;
            }
            if let Some(lines) = self.posted_lines(entry) {
                for line in lines {
                    *totals.entry(line.account).or_insert(0) += line.side.sign() * line.minor;
                }
            }
        }
        totals.retain(|_, balance| *balance != 0);
        totals
    }
}

/// VAT at 19 percent on a net amount in minor units, rounded half to even
/// (the rounding `ledgers-tax` names for German standard-rate VAT). Integer
/// arithmetic only.
#[must_use]
pub fn vat_19(net_minor: i64) -> i64 {
    let scaled = i128::from(net_minor) * 19;
    let mut quotient = scaled / 100;
    let remainder = scaled % 100;
    if remainder > 50 || (remainder == 50 && quotient % 2 == 1) {
        quotient += 1;
    }
    i64::try_from(quotient).unwrap_or(i64::MAX)
}
