//! The Factory Zero demo book (demo company "Factory Zero Pte. Ltd. (demo)",
//! Singapore, SGD): one year of a holding company that runs seven ventures
//! as business units inside one legal company.
//!
//! - `FZ` is the holding and factory unit. It carries the shared costs
//!   (people, infrastructure, tools) and recovers them each month from the
//!   ventures by a fixed allocation key (see [`ALLOCATION`]).
//! - Each venture is a unit with its own subscription revenue (some growing
//!   fast, some slowly, two still pre-revenue) and its own direct costs
//!   (hosting, model and API spend, domains, ads).
//! - Venture direct costs are posted by the agent; revenue receipts, the
//!   shared costs and the capital are posted by the person.
//! - The recharges are intercompany: consolidated views leave them out, and
//!   every unit sees its own part.
//!
//! **Tax.** Singapore GST (9%) is not applied: the amounts are net. TODO:
//! when the country pack lands, post the GST codes on the revenue and cost
//! lines.
//!
//! Every name, amount and reference is invented. Amounts are integer SGD
//! cents; the book uses no floats (docs/PLAN.md section 3).

use crate::{DemoBook, DemoEntry, DemoLine, DemoUnit, EntryKind, Poster, Side};

/// The company's display name.
pub const COMPANY: &str = "Factory Zero Pte. Ltd. (demo)";
/// The base currency.
pub const CURRENCY: &str = "SGD";

/// The holding and factory unit.
pub const UNIT_FZ: &str = "FZ";

/// One venture: its unit code, its name, its colour, its share of the shared
/// costs in percent, and its monthly subscription revenue in SGD (January to
/// December; zero is a pre-revenue month).
struct Venture {
    code: &'static str,
    name: &'static str,
    colour: &'static str,
    share_pct: i64,
    revenue_sgd: [i64; 12],
    /// Flat monthly Cloudflare (hosting, DNS, workers) spend, SGD.
    hosting_sgd: i64,
    /// Model and API spend: a share of revenue when it has revenue, else this flat amount.
    api_flat_sgd: i64,
    /// Domains, SGD per month.
    domains_sgd: i64,
    /// Ads, SGD per month (zero for none).
    ads_sgd: i64,
}

/// The ventures, in unit-code order for the book. The allocation keys sum to 100.
const VENTURES: [Venture; 7] = [
    Venture {
        code: "LDG",
        name: "Ledgers",
        colour: "#7FA6C9",
        share_pct: 25,
        revenue_sgd: [
            1200, 1272, 1348, 1429, 1515, 1606, 1702, 1804, 1912, 2027, 2148, 2277,
        ],
        hosting_sgd: 90,
        api_flat_sgd: 300,
        domains_sgd: 15,
        ads_sgd: 400,
    },
    Venture {
        code: "SQZ",
        name: "Sqeezin",
        colour: "#E9B44C",
        share_pct: 15,
        revenue_sgd: [400, 440, 484, 532, 585, 644, 708, 779, 857, 943, 1037, 1141],
        hosting_sgd: 45,
        api_flat_sgd: 120,
        domains_sgd: 5,
        ads_sgd: 250,
    },
    Venture {
        code: "LB",
        name: "Living Brain",
        colour: "#9AA0A6",
        share_pct: 10,
        revenue_sgd: [0, 0, 0, 0, 0, 0, 150, 165, 185, 200, 225, 250],
        hosting_sgd: 30,
        api_flat_sgd: 400,
        domains_sgd: 5,
        ads_sgd: 0,
    },
    Venture {
        code: "CF",
        name: "Cratefield",
        colour: "#7FB38A",
        share_pct: 20,
        revenue_sgd: [
            2500, 2575, 2652, 2732, 2814, 2898, 2985, 3074, 3167, 3262, 3360, 3461,
        ],
        hosting_sgd: 150,
        api_flat_sgd: 250,
        domains_sgd: 10,
        ads_sgd: 0,
    },
    Venture {
        code: "OWL",
        name: "Owlpost",
        colour: "#B88BD6",
        share_pct: 15,
        revenue_sgd: [
            800, 832, 865, 900, 936, 973, 1012, 1052, 1094, 1138, 1183, 1231,
        ],
        hosting_sgd: 60,
        api_flat_sgd: 80,
        domains_sgd: 10,
        ads_sgd: 200,
    },
    Venture {
        code: "COL",
        name: "Colonizer",
        colour: "#E07A5F",
        share_pct: 10,
        revenue_sgd: [0; 12],
        hosting_sgd: 40,
        api_flat_sgd: 300,
        domains_sgd: 5,
        ads_sgd: 150,
    },
    Venture {
        code: "SEAL",
        name: "Sealbin",
        colour: "#5FA8A8",
        share_pct: 5,
        revenue_sgd: [0, 0, 0, 250, 265, 280, 300, 320, 340, 360, 380, 400],
        hosting_sgd: 25,
        api_flat_sgd: 60,
        domains_sgd: 5,
        ads_sgd: 100,
    },
];

/// The allocation key of the shared costs, by venture code, in percent.
pub const ALLOCATION: [(&str, i64); 7] = [
    ("LDG", 25),
    ("SQZ", 15),
    ("LB", 10),
    ("CF", 20),
    ("OWL", 15),
    ("COL", 10),
    ("SEAL", 5),
];

/// The monthly shared costs the holding carries, in SGD: people, shared
/// infrastructure and tools. Their total is divisible by every share in
/// percent in cents, so each recharge is an exact integer.
const SHARED_PEOPLE_SGD: i64 = 18_000;
const SHARED_INFRA_SGD: i64 = 4_200;
const SHARED_TOOLS_SGD: i64 = 1_100;

/// The capital the company starts the year with, SGD.
const CAPITAL_SGD: i64 = 150_000;

/// The chart accounts the book uses (the bundled generic SME chart).
const BANK: &str = "1020";
const SUBSCRIPTIONS: &str = "4010";
const RECOVERY: &str = "4030";
const HOSTING: &str = "5010";
const PEOPLE: &str = "5020";
const ADS: &str = "5050";
const SHARED_SERVICES: &str = "5060";
const ACCRUED: &str = "2030";
const SHARE_CAPITAL: &str = "3010";
const DUE_FROM: &str = "1070";
const DUE_TO: &str = "2050";

/// The units of the book: the holding, then the seven ventures.
#[must_use]
pub fn units() -> Vec<DemoUnit> {
    let mut out = vec![DemoUnit {
        code: UNIT_FZ,
        name: "Factory Zero",
        colour: "#3D5A80",
    }];
    out.extend(VENTURES.iter().map(|venture| DemoUnit {
        code: venture.code,
        name: venture.name,
        colour: venture.colour,
    }));
    out
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn date(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

fn line(account: &'static str, side: Side, minor: i64, unit: Option<&'static str>) -> DemoLine {
    DemoLine {
        account,
        side,
        minor,
        tax_code: None,
        dimensions: unit.map(|code| vec![("unit", code)]).unwrap_or_default(),
        memo: None,
    }
}

fn entry(
    external_id: String,
    poster: Poster,
    posting_date: String,
    description: String,
    lines: Vec<DemoLine>,
) -> DemoEntry {
    DemoEntry {
        external_id,
        poster,
        posting_date,
        description,
        kind: EntryKind::Post { lines },
    }
}

/// One venture's entries for one month: its subscription receipts (when it
/// has revenue), its direct costs posted by the agent, and its monthly
/// recharge of the shared costs.
fn venture_month(
    year: i32,
    month: u32,
    name: &str,
    venture: &Venture,
    entries: &mut Vec<DemoEntry>,
) {
    let index = usize::try_from(month - 1).unwrap_or(0);
    let cents = |sgd: i64| sgd * 100;
    let shared_sgd = SHARED_PEOPLE_SGD + SHARED_INFRA_SGD + SHARED_TOOLS_SGD;
    let code = venture.code;
    let revenue_sgd = venture.revenue_sgd[index];
    if revenue_sgd > 0 {
        // Subscription receipts, paid by the customers into the bank.
        entries.push(entry(
            format!("{year}-{month:02}-{code}-subscriptions"),
            Poster::Person,
            date(year, month, 5),
            format!("{} subscription receipts, {name}", venture.name),
            vec![
                line(BANK, Side::Debit, cents(revenue_sgd), None),
                line(SUBSCRIPTIONS, Side::Credit, cents(revenue_sgd), Some(code)),
            ],
        ));
    }

    // Direct costs, posted by the agent against accrued expenses.
    let api_sgd = if revenue_sgd > 0 {
        revenue_sgd * 12 / 100
    } else {
        venture.api_flat_sgd
    };
    let mut costs = vec![
        line(HOSTING, Side::Debit, cents(venture.hosting_sgd), Some(code)),
        line(HOSTING, Side::Debit, cents(api_sgd), Some(code)),
        line(HOSTING, Side::Debit, cents(venture.domains_sgd), Some(code)),
    ];
    if venture.ads_sgd > 0 {
        costs.push(line(ADS, Side::Debit, cents(venture.ads_sgd), Some(code)));
    }
    let total: i64 = costs.iter().map(|l| l.minor).sum();
    costs.push(line(ACCRUED, Side::Credit, total, None));
    entries.push(entry(
        format!("{year}-{month:02}-{code}-direct-costs"),
        Poster::Agent,
        date(year, month, 3),
        format!(
            "Hosting, model API, domains and ads, {}, {name}",
            venture.name
        ),
        costs,
    ));

    // The monthly recharge of the shared costs: the venture books the
    // cost, the holding books the recovery. Intercompany.
    let recharge_sgd = shared_sgd * venture.share_pct / 100;
    entries.push(entry(
        format!("{year}-{month:02}-{code}-shared-recharge"),
        Poster::Person,
        date(year, month, 28),
        format!("Shared costs recharged to {}, {name}", venture.name),
        vec![
            line(
                SHARED_SERVICES,
                Side::Debit,
                cents(recharge_sgd),
                Some(code),
            ),
            line(DUE_TO, Side::Credit, cents(recharge_sgd), Some(code)),
            line(DUE_FROM, Side::Debit, cents(recharge_sgd), Some(UNIT_FZ)),
            line(RECOVERY, Side::Credit, cents(recharge_sgd), Some(UNIT_FZ)),
        ],
    ));
}

/// The whole Factory Zero book for fiscal year `year`. The amounts do not
/// depend on the year; only the dates move.
#[must_use]
pub fn book(year: i32) -> DemoBook {
    let mut entries: Vec<DemoEntry> = Vec::new();
    let cents = |sgd: i64| sgd * 100;

    // Capital: the person opens the year with share capital in the bank.
    entries.push(entry(
        format!("{year}-01-capital"),
        Poster::Person,
        date(year, 1, 1),
        "Share capital paid in".to_owned(),
        vec![
            line(BANK, Side::Debit, cents(CAPITAL_SGD), None),
            line(SHARE_CAPITAL, Side::Credit, cents(CAPITAL_SGD), None),
        ],
    ));

    for (index, name) in MONTHS.iter().enumerate() {
        let month = u32::try_from(index + 1).unwrap_or(0);
        let shared_sgd = SHARED_PEOPLE_SGD + SHARED_INFRA_SGD + SHARED_TOOLS_SGD;

        // The holding's shared costs, paid by the person.
        entries.push(entry(
            format!("{year}-{month:02}-shared-costs"),
            Poster::Person,
            date(year, month, 25),
            format!("Shared people, infrastructure and tools, {name}"),
            vec![
                line(PEOPLE, Side::Debit, cents(SHARED_PEOPLE_SGD), Some(UNIT_FZ)),
                line(HOSTING, Side::Debit, cents(SHARED_INFRA_SGD), Some(UNIT_FZ)),
                line(
                    SHARED_SERVICES,
                    Side::Debit,
                    cents(SHARED_TOOLS_SGD),
                    Some(UNIT_FZ),
                ),
                line(BANK, Side::Credit, cents(shared_sgd), None),
            ],
        ));

        for venture in &VENTURES {
            venture_month(year, month, name, venture, &mut entries);
        }
    }

    // One reversal, so the book shows a correction: the agent's direct cost
    // for Colonizer in March was keyed twice and is reversed by the person.
    entries.push(DemoEntry {
        external_id: format!("{year}-03-COL-direct-costs-reversal"),
        poster: Poster::Person,
        posting_date: date(year, 3, 6),
        description: "Reversal of a duplicated Colonizer cost line".to_owned(),
        kind: EntryKind::Reverse {
            of: format!("{year}-03-COL-direct-costs"),
        },
    });

    DemoBook {
        company: COMPANY,
        currency: CURRENCY,
        year,
        dimensions: Vec::new(),
        units: units(),
        entries,
    }
}

/// The golden totals of the book for its fiscal year, consolidated (the
/// intercompany recharges are eliminated). SGD cents. Computed from the
/// arrays above and checked by the tests.
pub struct Golden {
    /// Total subscription revenue for the year, credit-positive.
    pub fy_revenue: i64,
    /// Total expenses for the year: venture direct costs and shared costs.
    pub fy_expenses: i64,
}

/// The consolidated golden totals; see [`Golden`].
pub const GOLDEN: Golden = Golden {
    fy_revenue: 8_035_600,
    fy_expenses: 31_404_100,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_keys_sum_to_one_hundred_and_match_the_ventures() {
        let total: i64 = ALLOCATION.iter().map(|(_, pct)| pct).sum();
        assert_eq!(total, 100);
        for venture in &VENTURES {
            let share = ALLOCATION
                .iter()
                .find(|(code, _)| *code == venture.code)
                .expect("a key per venture");
            assert_eq!(share.1, venture.share_pct);
        }
    }

    #[test]
    fn shared_costs_recharge_in_exact_cents() {
        let shared_cents = (SHARED_PEOPLE_SGD + SHARED_INFRA_SGD + SHARED_TOOLS_SGD) * 100;
        for (_, pct) in ALLOCATION {
            assert_eq!(shared_cents * pct % 100, 0, "a {pct}% share is whole cents");
        }
    }
}
