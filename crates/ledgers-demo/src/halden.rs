//! Halden Works `GmbH`, the invented demo company, and one fiscal year of its
//! books. Every customer, vendor, person and reference number here is
//! invented; the names only sound plausible.
//!
//! Agent entries touch only unprotected accounts (inventory, prepayments,
//! accrued expenses, depreciation, expense accounts). Everything that moves
//! bank, receivables, payables, VAT or equity is person-posted.

use crate::{DemoBook, DemoDimension, DemoEntry, DemoLine, EntryKind, Poster, Side, vat_19};

/// The company's display name.
pub const COMPANY: &str = "Halden Works GmbH";
/// The base currency.
pub const CURRENCY: &str = "EUR";
/// The tax code the standard-rate lines carry.
const TAX: &str = "19-U";

/// Cost centre: sales.
const CC_SALES: (&str, &str) = ("cost_centre", "CC100");
/// Cost centre: production.
const CC_PRODUCTION: (&str, &str) = ("cost_centre", "CC200");
/// Cost centre: administration.
const CC_ADMIN: (&str, &str) = ("cost_centre", "CC300");
/// Project: the Kessler integration work billed as services.
const PROJECT_KESSLER: (&str, &str) = ("project", "PRJHALDEN01");

/// Net sales of frames, by month (cents), to Nordstrand Präzision `GmbH`.
const SALES_NET: [i64; 12] = [
    2_140_000, 1_985_000, 2_310_000, 2_275_000, 2_430_000, 2_090_000, 1_860_000, 2_540_000,
    2_610_000, 2_480_000, 2_730_000, 2_905_000,
];
/// Net integration services, by month (cents), to Kessler Logistik `GmbH`.
const SERVICES_NET: [i64; 12] = [
    680_000, 720_000, 695_000, 760_000, 810_000, 745_000, 670_000, 835_000, 880_000, 820_000,
    905_000, 940_000,
];
/// Net steel stock bills from Nordwald Stahl `GmbH`, by month (cents).
const MATERIALS_NET: [i64; 12] = [
    480_000, 515_000, 462_000, 533_000, 498_000, 524_000, 441_000, 557_000, 512_000, 489_000,
    546_000, 475_000,
];

/// Share capital paid in on 1 January.
const CAPITAL: i64 = 2_500_000;
/// Prior-year profit carried forward into the opening bank balance.
const RETAINED_OPENING: i64 = 1_200_000;
/// The bank loan drawn on 2 January.
const LOAN: i64 = 15_000_000;
/// The annual software licence paid in advance on 3 January (net).
const PREPAID_LICENCE: i64 = 120_000;
/// Monthly amortisation of the licence (net).
const LICENCE_AMORTISATION: i64 = 10_000;
/// The production equipment bought on 1 February (net), on AP.
const EQUIPMENT: i64 = 12_000_000;
/// Straight-line depreciation of the equipment, per month from February.
const DEPRECIATION: i64 = 200_000;
/// Cloud hosting, monthly (net).
const HOSTING: i64 = 18_900;
/// Monthly rent, paid by bank.
const RENT: i64 = 240_000;
/// Gross monthly salaries.
const PAYROLL_GROSS: i64 = 1_400_000;
/// Employee deductions withheld from gross pay (tax and social security).
const PAYROLL_WITHHELD: i64 = 470_000;
/// The employer's social security share, monthly.
const PAYROLL_EMPLOYER: i64 = 280_000;
/// Monthly bank charges.
const BANK_CHARGES: i64 = 990;
/// Monthly loan interest (150,000 at 4.2 percent a year).
const LOAN_INTEREST: i64 = 52_500;
/// The goods received note GR-0931 (net), accrued in March, billed in April.
const GOODS_RECEIVED: i64 = 350_000;

/// Month names in German, for the descriptions.
const MONTH_NAMES: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];

/// The dimension values the fixture posts against.
#[must_use]
fn dimensions() -> Vec<DemoDimension> {
    vec![
        DemoDimension {
            kind: "cost_centre",
            code: "CC100",
            name: "Sales",
        },
        DemoDimension {
            kind: "cost_centre",
            code: "CC200",
            name: "Production",
        },
        DemoDimension {
            kind: "cost_centre",
            code: "CC300",
            name: "Administration",
        },
        DemoDimension {
            kind: "project",
            code: "PRJHALDEN01",
            name: "Kessler integration",
        },
    ]
}

/// The whole demo book for fiscal year `year`. The amounts do not depend on
/// the year, so the golden totals hold for any year; only the dates move.
#[must_use]
pub fn book(year: i32) -> DemoBook {
    let mut entries: Vec<DemoEntry> = Vec::new();
    for month in 1..=12u32 {
        entries.extend(month_entries(year, month));
    }
    DemoBook {
        company: COMPANY,
        currency: CURRENCY,
        year,
        dimensions: dimensions(),
        entries,
    }
}

fn date(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

fn id(month: u32, slug: &str) -> String {
    format!("halden-{month:02}-{slug}")
}

fn gross(net: i64) -> i64 {
    net + vat_19(net)
}

fn debit(account: &'static str, minor: i64) -> DemoLine {
    DemoLine {
        account,
        side: Side::Debit,
        minor,
        tax_code: None,
        dimensions: Vec::new(),
        memo: None,
    }
}

fn credit(account: &'static str, minor: i64) -> DemoLine {
    DemoLine {
        side: Side::Credit,
        ..debit(account, minor)
    }
}

fn taxed(mut line: DemoLine) -> DemoLine {
    line.tax_code = Some(TAX);
    line
}

fn tagged(mut line: DemoLine, dimension: (&'static str, &'static str)) -> DemoLine {
    line.dimensions.push(dimension);
    line
}

fn post(
    external_id: String,
    poster: Poster,
    posting_date: String,
    description: impl Into<String>,
    lines: Vec<DemoLine>,
) -> DemoEntry {
    DemoEntry {
        external_id,
        poster,
        posting_date,
        description: description.into(),
        kind: EntryKind::Post { lines },
    }
}

fn reverse(
    external_id: String,
    poster: Poster,
    posting_date: String,
    description: impl Into<String>,
    of: &str,
) -> DemoEntry {
    DemoEntry {
        external_id,
        poster,
        posting_date,
        description: description.into(),
        kind: EntryKind::Reverse { of: of.to_owned() },
    }
}

/// Output VAT on the month's sales (frames and services), minor units.
fn output_vat(month_index: usize) -> i64 {
    vat_19(SALES_NET[month_index]) + vat_19(SERVICES_NET[month_index])
}

/// Input VAT deductible in the month: the bills of that month and the
/// January licence and February equipment purchases.
fn input_vat(month: u32) -> i64 {
    let index = (month - 1) as usize;
    let mut total = vat_19(MATERIALS_NET[index]) + vat_19(HOSTING);
    if month == 1 {
        total += vat_19(PREPAID_LICENCE);
    }
    if month == 2 {
        total += vat_19(EQUIPMENT);
    }
    if month == 4 {
        total += vat_19(GOODS_RECEIVED);
    }
    total
}

/// The entries of one month. Earlier months are not needed: every amount is
/// a function of the month's index, so the year is a pure function too.
#[allow(clippy::too_many_lines)]
fn month_entries(year: i32, month: u32) -> Vec<DemoEntry> {
    let index = (month - 1) as usize;
    let name = MONTH_NAMES[index];
    let mut out: Vec<DemoEntry> = Vec::new();
    let prev = index.checked_sub(1);

    // Opening balances and the one-off purchases, all person-posted.
    if month == 1 {
        out.push(post(
            id(1, "opening-capital"),
            Poster::Person,
            date(year, 1, 1),
            "Opening balance: share capital paid in",
            vec![debit("1020", CAPITAL), credit("3010", CAPITAL)],
        ));
        out.push(post(
            id(1, "opening-retained"),
            Poster::Person,
            date(year, 1, 1),
            "Opening balance: prior-year profit carried forward",
            vec![
                debit("1020", RETAINED_OPENING),
                credit("3020", RETAINED_OPENING),
            ],
        ));
        out.push(post(
            id(1, "loan-drawdown"),
            Poster::Person,
            date(year, 1, 2),
            "Bank loan drawn down (4.2 percent, five years)",
            vec![debit("1020", LOAN), credit("2510", LOAN)],
        ));
        out.push(post(
            id(1, "prepaid-licence"),
            Poster::Person,
            date(year, 1, 3),
            "Annual software licence paid in advance",
            vec![
                debit("1060", PREPAID_LICENCE),
                taxed(debit("1050", vat_19(PREPAID_LICENCE))),
                credit("1020", PREPAID_LICENCE + vat_19(PREPAID_LICENCE)),
            ],
        ));
    }
    if month == 2 {
        out.push(post(
            id(2, "equipment-purchase"),
            Poster::Person,
            date(year, 2, 1),
            "Production line equipment, bill from Fördertechnik Oberhausen GmbH",
            vec![
                debit("1510", EQUIPMENT),
                taxed(debit("1050", vat_19(EQUIPMENT))),
                credit("2010", EQUIPMENT + vat_19(EQUIPMENT)),
            ],
        ));
    }
    if month == 3 {
        out.push(post(
            id(3, "equipment-payment"),
            Poster::Person,
            date(year, 3, 15),
            "Payment to Fördertechnik Oberhausen GmbH",
            vec![
                debit("2010", EQUIPMENT + vat_19(EQUIPMENT)),
                credit("1020", EQUIPMENT + vat_19(EQUIPMENT)),
            ],
        ));
        out.push(post(
            id(3, "goods-received-gr-0931"),
            Poster::Agent,
            date(year, 3, 12),
            "Goods received GR-0931 against PO-1182 from Acme GmbH, not yet invoiced",
            vec![
                debit("1040", GOODS_RECEIVED),
                credit("2030", GOODS_RECEIVED),
            ],
        ));
    }

    // Agent: the licence is amortised monthly, equipment depreciated from
    // February.
    out.push(post(
        id(month, "prepaid-amortisation"),
        Poster::Agent,
        date(year, month, 28),
        format!("Software licence amortisation, {name}"),
        vec![
            tagged(debit("5060", LICENCE_AMORTISATION), CC_ADMIN),
            credit("1060", LICENCE_AMORTISATION),
        ],
    ));
    if month >= 2 {
        out.push(post(
            id(month, "depreciation"),
            Poster::Agent,
            date(year, month, 28),
            format!("Depreciation of production equipment, {name}"),
            vec![
                tagged(debit("5080", DEPRECIATION), CC_PRODUCTION),
                credit("1590", DEPRECIATION),
            ],
        ));
    }
    if month == 6 {
        // Agent posted the June depreciation twice; the agent reverses the
        // second posting on the last day of the month.
        out.push(post(
            id(6, "depreciation-duplicate"),
            Poster::Agent,
            date(year, 6, 28),
            "Depreciation of production equipment, Juni (posted twice in error)",
            vec![
                tagged(debit("5080", DEPRECIATION), CC_PRODUCTION),
                credit("1590", DEPRECIATION),
            ],
        ));
        out.push(reverse(
            id(6, "depreciation-duplicate-reversal"),
            Poster::Agent,
            date(year, 6, 30),
            "Reversal of the duplicate June depreciation",
            &id(6, "depreciation-duplicate"),
        ));
    }

    // Person: rent, hosting and materials bills, and the bank's own charges.
    out.push(post(
        id(month, "rent"),
        Poster::Person,
        date(year, month, 1),
        format!("Rent of the Halden site, {name}"),
        vec![tagged(debit("5030", RENT), CC_ADMIN), credit("1020", RENT)],
    ));
    out.push(post(
        id(month, "hosting-bill"),
        Poster::Person,
        date(year, month, 3),
        format!("Cloudsteward GmbH hosting, {name}"),
        vec![
            tagged(taxed(debit("5060", HOSTING)), CC_ADMIN),
            taxed(debit("1050", vat_19(HOSTING))),
            credit("2010", gross(HOSTING)),
        ],
    ));
    out.push(post(
        id(month, "materials-bill"),
        Poster::Person,
        date(year, month, 8),
        format!("Nordwald Stahl GmbH steel stock, {name}"),
        vec![
            tagged(taxed(debit("5010", MATERIALS_NET[index])), CC_PRODUCTION),
            taxed(debit("1050", vat_19(MATERIALS_NET[index]))),
            credit("2010", gross(MATERIALS_NET[index])),
        ],
    ));
    out.push(post(
        id(month, "payroll"),
        Poster::Person,
        date(year, month, 25),
        format!("Payroll summary, {name}: gross pay and employer contributions"),
        vec![
            tagged(debit("5020", PAYROLL_GROSS + PAYROLL_EMPLOYER), CC_ADMIN),
            credit("1020", PAYROLL_GROSS - PAYROLL_WITHHELD),
            credit("2030", PAYROLL_WITHHELD + PAYROLL_EMPLOYER),
        ],
    ));
    out.push(post(
        id(month, "bank-charges-and-interest"),
        Poster::Person,
        date(year, month, 28),
        format!("Bank charges and loan interest, {name}"),
        vec![
            debit("5100", BANK_CHARGES),
            debit("5090", LOAN_INTEREST),
            credit("1020", BANK_CHARGES + LOAN_INTEREST),
        ],
    ));

    // Sales: two invoices a month, and last month's invoices paid in.
    let elbe_number = format!("RE-{year}-{:04}", (index * 2) + 1);
    let kessler_number = format!("RE-{year}-{:04}", (index * 2) + 2);
    out.push(post(
        id(month, "invoice-nordstrand"),
        Poster::Person,
        date(year, month, 5),
        format!("Invoice {elbe_number} to Nordstrand Präzision GmbH, frames, {name}"),
        vec![
            debit("1030", gross(SALES_NET[index])),
            tagged(taxed(credit("4010", SALES_NET[index])), CC_SALES),
            credit("2020", vat_19(SALES_NET[index])),
        ],
    ));
    out.push(post(
        id(month, "invoice-kessler"),
        Poster::Person,
        date(year, month, 12),
        format!("Invoice {kessler_number} to Kessler Logistik GmbH, integration services, {name}"),
        vec![
            debit("1030", gross(SERVICES_NET[index])),
            tagged(taxed(credit("4020", SERVICES_NET[index])), PROJECT_KESSLER),
            credit("2020", vat_19(SERVICES_NET[index])),
        ],
    ));
    if month == 8 {
        // A duplicate of the August frames invoice, posted by the owner and
        // reversed the same day.
        let duplicate = id(8, "invoice-nordstrand-duplicate");
        out.push(post(
            duplicate.clone(),
            Poster::Person,
            date(year, 8, 12),
            "Invoice Nordstrand Präzision GmbH, posted twice (duplicate)",
            vec![
                debit("1030", gross(SALES_NET[index])),
                tagged(taxed(credit("4010", SALES_NET[index])), CC_SALES),
                credit("2020", vat_19(SALES_NET[index])),
            ],
        ));
        out.push(reverse(
            id(8, "invoice-nordstrand-duplicate-reversal"),
            Poster::Person,
            date(year, 8, 12),
            "Reversal of the duplicate invoice",
            &duplicate,
        ));
    }
    if let Some(prev) = prev {
        out.push(post(
            id(month, "receipts"),
            Poster::Person,
            date(year, month, 20),
            format!("Customer payments for {}, received", MONTH_NAMES[prev]),
            vec![
                debit("1020", gross(SALES_NET[prev]) + gross(SERVICES_NET[prev])),
                credit("1030", gross(SALES_NET[prev]) + gross(SERVICES_NET[prev])),
            ],
        ));
        // Last month's supplier bills are paid on the 25th.
        let mut supplier_bills = gross(HOSTING) + gross(MATERIALS_NET[prev]);
        if month == 5 {
            supplier_bills += gross(GOODS_RECEIVED);
        }
        out.push(post(
            id(month, "supplier-payments"),
            Poster::Person,
            date(year, month, 26),
            format!("Payments to suppliers for {}", MONTH_NAMES[prev]),
            vec![
                debit("2010", supplier_bills),
                credit("1020", supplier_bills),
            ],
        ));
        // Last month's payroll taxes and social security are remitted on the 10th.
        out.push(post(
            id(month, "payroll-remittance"),
            Poster::Person,
            date(year, month, 10),
            format!(
                "Payroll taxes and social security remitted for {}",
                MONTH_NAMES[prev]
            ),
            vec![
                debit("2030", PAYROLL_WITHHELD + PAYROLL_EMPLOYER),
                credit("1020", PAYROLL_WITHHELD + PAYROLL_EMPLOYER),
            ],
        ));
    }
    if month == 4 {
        out.push(post(
            id(4, "invoice-acme-rg-0418"),
            Poster::Person,
            date(year, 4, 3),
            "Acme GmbH invoice RE-2026-0418 against PO-1182 and GR-0931, clearing the goods received accrual",
            vec![
                debit("2030", GOODS_RECEIVED),
                debit("1050", vat_19(GOODS_RECEIVED)),
                credit("2010", gross(GOODS_RECEIVED)),
            ],
        ));
    }

    // VAT settlement with the tax office after each quarter (April, July,
    // October). The December quarter settles in the following year.
    if matches!(month, 4 | 7 | 10) {
        let quarter: Vec<u32> = (month - 3..month).collect();
        let out_vat: i64 = quarter.iter().map(|m| output_vat((m - 1) as usize)).sum();
        let in_vat: i64 = quarter.iter().map(|m| input_vat(*m)).sum();
        let quarter_name = match month {
            4 => "Q1",
            7 => "Q2",
            _ => "Q3",
        };
        let mut lines = Vec::new();
        if out_vat > 0 {
            lines.push(debit("2020", out_vat));
        }
        if in_vat > 0 {
            lines.push(credit("1050", in_vat));
        }
        let payable = out_vat - in_vat;
        if payable > 0 {
            lines.push(credit("1020", payable));
        } else if payable < 0 {
            lines.push(debit("1020", -payable));
        }
        out.push(post(
            id(month, "vat-settlement"),
            Poster::Person,
            date(year, month, 10),
            format!("VAT return {quarter_name} paid to the Finanzamt"),
            lines,
        ));
    }

    out
}

/// The golden totals of the fixture for its fiscal year. The amounts do not
/// depend on the year, so these hold for any `book(year)`.
pub struct Golden {
    /// Debit-positive balance per account at 31 December (accounts with a
    /// balance only), sorted by account number.
    pub year_end_balances: &'static [(&'static str, i64)],
    /// Total income for the year, credit-positive.
    pub fy_revenue: i64,
    /// Total expenses for the year.
    pub fy_expenses: i64,
    /// Income minus expenses for the year.
    pub fy_net_income: i64,
}

/// The golden totals; see [`Golden`].
pub const GOLDEN: Golden = Golden {
    year_end_balances: &[
        ("1020", 12_617_038),
        ("1030", 4_575_550),
        ("1040", 350_000),
        ("1050", 297_673),
        ("1510", 12_000_000),
        ("1590", -2_200_000),
        ("2010", -587_741),
        ("2020", -2_048_200),
        ("2030", -750_000),
        ("2510", -15_000_000),
        ("3010", -2_500_000),
        ("3020", -1_200_000),
        ("4010", -28_355_000),
        ("4020", -9_460_000),
        ("5010", 6_032_000),
        ("5020", 20_160_000),
        ("5030", 2_880_000),
        ("5060", 346_800),
        ("5080", 2_200_000),
        ("5090", 630_000),
        ("5100", 11_880),
    ],
    fy_revenue: 37_815_000,
    fy_expenses: 32_260_680,
    fy_net_income: 5_554_320,
};
