//! ISO 4217 currencies: a static, sorted table of active and curated
//! historic codes.
//!
//! The table below is generated from live sources and mirrored by the
//! fixture in `tests/iso4217_table.rs`, which fails if the two drift. See
//! the crate docs ("Refreshing the currency table") for the refresh
//! procedure — in particular, codes are never deleted: a retired code
//! moves from active to historic so old entries keep parsing.

use crate::error::MoneyError;
use std::fmt;
use std::str::FromStr;

/// One row of the static currency table.
struct CurrencyDef {
    code: &'static str,
    numeric: u16,
    exponent: u8,
    active: bool,
}

// Source of truth (retrieved 2026-10-18):
//
//   Primary (active codes): Wikipedia, "ISO 4217"
//   https://en.wikipedia.org/wiki/ISO_4217
//   (fetched via https://en.wikipedia.org/api/rest_v1/page/html/ISO_4217)
//   — the article's active-codes table, every row except the four
//   precious-metal codes (XAU, XAG, XPD, XPT), which are not List One
//   currencies and are deliberately absent from this crate.
//
//   Secondary (cross-check of numeric codes and exponents, and the
//   historic withdrawal record): the ISO 4217 maintenance-agency data
//   mirrored by datasets/currency-codes
//   https://raw.githubusercontent.com/datasets/currency-codes/master/data/codes-all.csv
//
// Every active exponent and numeric in the table was cross-checked between
// the two sources with zero disagreements. Historic numeric codes were
// additionally validated against the last official SIX "list three"
// snapshot (published 2018-08-20,
// https://web.archive.org/web/2020/https://www.currency-iso.org/dam/downloads/lists/list_three.xml).
//
// The `active` rows are ISO 4217 List One circulating currencies plus the
// funds/other codes the list carries (XDR, XSU, XUA, XTS, XXX, CLF, MXV,
// COU, UYI, UYW, USN, CHE, CHW, BOV, the bond-market units XBA-XBD, ...).
// Codes whose exponent the list marks "N.A." (D = ".") are stored with
// exponent 0. SUI is absent from both live sources and therefore not in
// the table.
//
// The `historic` section is a curated subset of ISO 4217 List Three, not
// the complete list: every currency the euro replaced, plus
// commonly-encountered retired codes, kept so old entries keep parsing.
// Its exponents come from the historic table of the primary source (the
// ISO-registered minor unit, not CLDR's display-digit defaults, which
// differ deliberately, e.g. SLL). GRD is stored with its final exponent 0
// (the source lists "0, 2" across its lifetime). GNE was dropped: no
// source confirms its exponent.
static ISO_4217: [CurrencyDef; crate::currency::TABLE_LEN] = [
    CurrencyDef {
        code: "AED",
        numeric: 784,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "AFA",
        numeric: 4,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "AFN",
        numeric: 971,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ALL",
        numeric: 8,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "AMD",
        numeric: 51,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "AOA",
        numeric: 973,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ARS",
        numeric: 32,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ATS",
        numeric: 40,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "AUD",
        numeric: 36,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "AWG",
        numeric: 533,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "AZN",
        numeric: 944,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BAM",
        numeric: 977,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BBD",
        numeric: 52,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BDT",
        numeric: 50,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BEF",
        numeric: 56,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "BHD",
        numeric: 48,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "BIF",
        numeric: 108,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "BMD",
        numeric: 60,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BND",
        numeric: 96,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BOB",
        numeric: 68,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BOV",
        numeric: 984,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BRL",
        numeric: 986,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BSD",
        numeric: 44,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BTN",
        numeric: 64,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BWP",
        numeric: 72,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BYN",
        numeric: 933,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "BYR",
        numeric: 974,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "BZD",
        numeric: 84,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CAD",
        numeric: 124,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CDF",
        numeric: 976,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CHE",
        numeric: 947,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CHF",
        numeric: 756,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CHW",
        numeric: 948,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CLF",
        numeric: 990,
        exponent: 4,
        active: true,
    },
    CurrencyDef {
        code: "CLP",
        numeric: 152,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "CNY",
        numeric: 156,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "COP",
        numeric: 170,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "COU",
        numeric: 970,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CRC",
        numeric: 188,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CSK",
        numeric: 200,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "CUC",
        numeric: 931,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "CUP",
        numeric: 192,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CVE",
        numeric: 132,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "CYP",
        numeric: 196,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "CZK",
        numeric: 203,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "DEM",
        numeric: 276,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "DJF",
        numeric: 262,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "DKK",
        numeric: 208,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "DOP",
        numeric: 214,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "DZD",
        numeric: 12,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "EEK",
        numeric: 233,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "EGP",
        numeric: 818,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ERN",
        numeric: 232,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ESP",
        numeric: 724,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "ETB",
        numeric: 230,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "EUR",
        numeric: 978,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "FIM",
        numeric: 246,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "FJD",
        numeric: 242,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "FKP",
        numeric: 238,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "FRF",
        numeric: 250,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "GBP",
        numeric: 826,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GEL",
        numeric: 981,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GHS",
        numeric: 936,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GIP",
        numeric: 292,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GMD",
        numeric: 270,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GNF",
        numeric: 324,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "GRD",
        numeric: 300,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "GTQ",
        numeric: 320,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "GYD",
        numeric: 328,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "HKD",
        numeric: 344,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "HNL",
        numeric: 340,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "HTG",
        numeric: 332,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "HUF",
        numeric: 348,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "IDR",
        numeric: 360,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "IEP",
        numeric: 372,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "ILS",
        numeric: 376,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "INR",
        numeric: 356,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "IQD",
        numeric: 368,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "IRR",
        numeric: 364,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ISK",
        numeric: 352,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "ITL",
        numeric: 380,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "JMD",
        numeric: 388,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "JOD",
        numeric: 400,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "JPY",
        numeric: 392,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "KES",
        numeric: 404,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "KGS",
        numeric: 417,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "KHR",
        numeric: 116,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "KMF",
        numeric: 174,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "KPW",
        numeric: 408,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "KRW",
        numeric: 410,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "KWD",
        numeric: 414,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "KYD",
        numeric: 136,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "KZT",
        numeric: 398,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LAK",
        numeric: 418,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LBP",
        numeric: 422,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LKR",
        numeric: 144,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LRD",
        numeric: 430,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LSL",
        numeric: 426,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "LUF",
        numeric: 442,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "LVL",
        numeric: 428,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "LYD",
        numeric: 434,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "MAD",
        numeric: 504,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MDL",
        numeric: 498,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MGA",
        numeric: 969,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MGF",
        numeric: 450,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "MKD",
        numeric: 807,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MMK",
        numeric: 104,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MNT",
        numeric: 496,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MOP",
        numeric: 446,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MRO",
        numeric: 478,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "MRU",
        numeric: 929,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MTL",
        numeric: 470,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "MUR",
        numeric: 480,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MVR",
        numeric: 462,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MWK",
        numeric: 454,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MXN",
        numeric: 484,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MXV",
        numeric: 979,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MYR",
        numeric: 458,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "MZM",
        numeric: 508,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "MZN",
        numeric: 943,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NAD",
        numeric: 516,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NGN",
        numeric: 566,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NIO",
        numeric: 558,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NLG",
        numeric: 528,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "NOK",
        numeric: 578,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NPR",
        numeric: 524,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "NZD",
        numeric: 554,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "OMR",
        numeric: 512,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "PAB",
        numeric: 590,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PEN",
        numeric: 604,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PGK",
        numeric: 598,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PHP",
        numeric: 608,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PKR",
        numeric: 586,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PLN",
        numeric: 985,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "PTE",
        numeric: 620,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "PYG",
        numeric: 600,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "QAR",
        numeric: 634,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ROL",
        numeric: 642,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "RON",
        numeric: 946,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "RSD",
        numeric: 941,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "RUB",
        numeric: 643,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "RUR",
        numeric: 810,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "RWF",
        numeric: 646,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "SAR",
        numeric: 682,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SBD",
        numeric: 90,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SCR",
        numeric: 690,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SDD",
        numeric: 736,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "SDG",
        numeric: 938,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SEK",
        numeric: 752,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SGD",
        numeric: 702,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SHP",
        numeric: 654,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SIT",
        numeric: 705,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "SKK",
        numeric: 703,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "SLE",
        numeric: 925,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SLL",
        numeric: 694,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "SOS",
        numeric: 706,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SRD",
        numeric: 968,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SRG",
        numeric: 740,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "SSP",
        numeric: 728,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "STD",
        numeric: 678,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "STN",
        numeric: 930,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SVC",
        numeric: 222,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SYP",
        numeric: 760,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "SZL",
        numeric: 748,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "THB",
        numeric: 764,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TJS",
        numeric: 972,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TMM",
        numeric: 795,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "TMT",
        numeric: 934,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TND",
        numeric: 788,
        exponent: 3,
        active: true,
    },
    CurrencyDef {
        code: "TOP",
        numeric: 776,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TRL",
        numeric: 792,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "TRY",
        numeric: 949,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TTD",
        numeric: 780,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TWD",
        numeric: 901,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "TZS",
        numeric: 834,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "UAH",
        numeric: 980,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "UGX",
        numeric: 800,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "USD",
        numeric: 840,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "USN",
        numeric: 997,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "UYI",
        numeric: 940,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "UYU",
        numeric: 858,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "UYW",
        numeric: 927,
        exponent: 4,
        active: true,
    },
    CurrencyDef {
        code: "UZS",
        numeric: 860,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "VEB",
        numeric: 862,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "VED",
        numeric: 926,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "VEF",
        numeric: 937,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "VES",
        numeric: 928,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "VND",
        numeric: 704,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "VUV",
        numeric: 548,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "WST",
        numeric: 882,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "XAD",
        numeric: 396,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "XAF",
        numeric: 950,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XBA",
        numeric: 955,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XBB",
        numeric: 956,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XBC",
        numeric: 957,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XBD",
        numeric: 958,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XCD",
        numeric: 951,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "XCG",
        numeric: 532,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "XDR",
        numeric: 960,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XEU",
        numeric: 954,
        exponent: 0,
        active: false,
    },
    CurrencyDef {
        code: "XOF",
        numeric: 952,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XPF",
        numeric: 953,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XSU",
        numeric: 994,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XTS",
        numeric: 963,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XUA",
        numeric: 965,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "XXX",
        numeric: 999,
        exponent: 0,
        active: true,
    },
    CurrencyDef {
        code: "YER",
        numeric: 886,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "YUM",
        numeric: 891,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "ZAL",
        numeric: 991,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "ZAR",
        numeric: 710,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ZMK",
        numeric: 894,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "ZMW",
        numeric: 967,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ZWD",
        numeric: 716,
        exponent: 2,
        active: false,
    },
    CurrencyDef {
        code: "ZWG",
        numeric: 924,
        exponent: 2,
        active: true,
    },
    CurrencyDef {
        code: "ZWL",
        numeric: 932,
        exponent: 2,
        active: false,
    },
];

/// The number of entries in [`ISO_4217`].
pub(crate) const TABLE_LEN: usize = 215;

/// An ISO 4217 currency, active or historic, interned in the static table.
///
/// Constructed via [`Currency::from_code`], [`Currency::from_numeric`] or
/// [`FromStr`]; the only `Currency` values that exist are the table's, so
/// two equal codes are the same value. Copy semantics: pass by value.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Currency {
    code: &'static str,
}

impl Currency {
    fn from_def(def: &'static CurrencyDef) -> Self {
        Self { code: def.code }
    }

    /// Look up a currency by its three-letter alphabetic code.
    ///
    /// Nothing is trimmed. ASCII lowercase input is normalised to
    /// uppercase (`"eur"` finds EUR); the input must be exactly three
    /// ASCII letters, else [`MoneyError::InvalidCurrencyCode`]. A
    /// well-formed code that is not in the table — active or historic —
    /// is [`MoneyError::UnknownCurrency`].
    ///
    /// # Errors
    /// [`MoneyError::InvalidCurrencyCode`] for malformed input;
    /// [`MoneyError::UnknownCurrency`] for a well-formed unknown code.
    pub fn from_code(input: &str) -> Result<Self, MoneyError> {
        if input.len() != 3 || !input.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(MoneyError::InvalidCurrencyCode {
                input: input.to_owned(),
            });
        }
        let normalised = input.to_ascii_uppercase();
        let idx = ISO_4217.partition_point(|def| def.code < normalised.as_str());
        match ISO_4217.get(idx) {
            Some(def) if def.code == normalised => Ok(Self::from_def(def)),
            _ => Err(MoneyError::UnknownCurrency { code: normalised }),
        }
    }

    /// Look up a currency by its ISO 4217 numeric code.
    ///
    /// Numeric codes are unique among active currencies, but ISO reissues
    /// numbers across eras, so a numeric may match both an active and a
    /// historic currency: the **active** match wins. The current table
    /// happens to contain no such collision.
    ///
    /// # Errors
    /// [`MoneyError::UnknownNumericCode`] if no table entry has this
    /// numeric.
    pub fn from_numeric(numeric: u16) -> Result<Self, MoneyError> {
        ISO_4217
            .iter()
            .find(|def| def.numeric == numeric && def.active)
            .or_else(|| ISO_4217.iter().find(|def| def.numeric == numeric))
            .map(Self::from_def)
            .ok_or(MoneyError::UnknownNumericCode { numeric })
    }

    /// Every currency in the table, active and historic, in code order
    /// (the table is sorted, so iteration is, too).
    pub fn all() -> impl Iterator<Item = Currency> {
        ISO_4217.iter().map(Self::from_def)
    }

    /// The three-letter alphabetic code, e.g. `"EUR"`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// The ISO 4217 numeric code, e.g. `978` for EUR.
    #[must_use]
    pub const fn numeric(self) -> u16 {
        Self::def_for(self.code).numeric
    }

    /// The minor-unit exponent: JPY 0, EUR 2, KWD 3, CLF 4.
    #[must_use]
    pub const fn exponent(self) -> u32 {
        Self::def_for(self.code).exponent as u32
    }

    /// Whether the code is on the active list (true) or the curated
    /// historic section (false).
    #[must_use]
    pub const fn is_active(self) -> bool {
        Self::def_for(self.code).active
    }

    /// 10 to the power of [`Currency::exponent`]: the number of minor
    /// units in one major unit, as an [`i128`].
    #[must_use]
    pub const fn minor_per_major(self) -> i128 {
        let mut pow: i128 = 1;
        let mut n = self.exponent();
        while n > 0 {
            pow *= 10;
            n -= 1;
        }
        pow
    }

    /// Binary-search the table for this code's definition. Total because
    /// `self.code` is always one of the table's `&'static str` entries:
    /// the field is private and every constructor stores a table entry,
    /// so the search always lands on a match.
    const fn def_for(code: &str) -> &'static CurrencyDef {
        let bytes = code.as_bytes();
        let mut lo = 0usize;
        let mut hi = ISO_4217.len();
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if const_str_lt(ISO_4217[mid].code.as_bytes(), bytes) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        &ISO_4217[lo]
    }
}

/// Lexicographic byte-wise `a < b`, callable in const contexts (`str`
/// ordering is not).
const fn const_str_lt(a: &[u8], b: &[u8]) -> bool {
    let n = if a.len() < b.len() { a.len() } else { b.len() };
    let mut i = 0;
    while i < n {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl FromStr for Currency {
    type Err = MoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_code(s)
    }
}

#[cfg(test)]
mod tests {
    use super::Currency;
    use crate::error::MoneyError;

    fn code(input: &str) -> Result<Currency, MoneyError> {
        input.parse::<Currency>()
    }

    #[test]
    fn from_code_normalises_ascii_lowercase() {
        assert_eq!(code("eur").unwrap(), code("EUR").unwrap());
        assert_eq!(code("GbP").unwrap().code(), "GBP");
        assert_eq!(code("jpy").unwrap().exponent(), 0);
    }

    #[test]
    fn from_code_rejects_malformed() {
        for bad in [
            "EU", "EURO", "e1", "   ", " US", "USA ", "Ü£", "us", "123", "",
        ] {
            assert!(
                matches!(code(bad), Err(MoneyError::InvalidCurrencyCode { .. })),
                "{bad:?} should be an invalid code"
            );
        }
    }

    #[test]
    fn from_code_rejects_wellformed_unknown() {
        assert_eq!(
            code("XYZ").unwrap_err(),
            MoneyError::UnknownCurrency {
                code: String::from("XYZ")
            }
        );
        assert_eq!(
            code("Aaa").unwrap_err(),
            MoneyError::UnknownCurrency {
                code: String::from("AAA")
            },
            "unknown code reported normalised"
        );
    }

    #[test]
    fn from_numeric_spot_checks() {
        assert_eq!(Currency::from_numeric(978).unwrap(), code("EUR").unwrap());
        assert_eq!(Currency::from_numeric(392).unwrap(), code("JPY").unwrap());
        assert!(matches!(
            Currency::from_numeric(9999),
            Err(MoneyError::UnknownNumericCode { numeric: 9999 })
        ));
        // Active ZWG (924) and historic ZWL (932) both resolve; today's
        // table has no numeric shared between an active and a historic
        // entry, so the active-wins rule has no live example — it exists
        // for the next era's reissued numerics.
        assert!(Currency::from_numeric(924).unwrap().is_active());
        assert!(!Currency::from_numeric(932).unwrap().is_active());
    }

    #[test]
    fn accessors() {
        let eur = code("EUR").unwrap();
        assert_eq!(eur.code(), "EUR");
        assert_eq!(eur.numeric(), 978);
        assert_eq!(eur.exponent(), 2);
        assert!(eur.is_active());
        assert_eq!(eur.minor_per_major(), 100);
        assert_eq!(code("JPY").unwrap().minor_per_major(), 1);
        assert_eq!(code("KWD").unwrap().minor_per_major(), 1000);
        assert_eq!(code("CLF").unwrap().minor_per_major(), 10_000);
        assert_eq!(eur.to_string(), "EUR");
    }

    #[test]
    fn table_is_sorted_and_wellformed() {
        let all: Vec<Currency> = Currency::all().collect();
        for pair in all.windows(2) {
            assert!(
                pair[0].code() < pair[1].code(),
                "table not sorted at {}",
                pair[1].code()
            );
        }
        assert_eq!(all.len(), super::TABLE_LEN);
        assert_eq!(all.len(), super::ISO_4217.len());
        for c in &all {
            let code = c.code();
            assert_eq!(code.len(), 3, "{code}");
            assert!(
                code.bytes().all(|b| b.is_ascii_uppercase()),
                "{code} is not three uppercase ASCII letters"
            );
            if c.is_active() {
                assert!(c.exponent() <= 4, "{code} exponent {}", c.exponent());
            }
        }
        // Numeric codes are unique within the active set.
        let mut numerics: Vec<u16> = all
            .iter()
            .filter(|c| c.is_active())
            .map(|c| c.numeric())
            .collect();
        numerics.sort_unstable();
        let len = numerics.len();
        numerics.dedup();
        assert_eq!(numerics.len(), len, "duplicate numeric among active codes");
    }

    #[test]
    fn currency_is_copy_and_eq() {
        let a = code("EUR").unwrap();
        let b = a;
        assert_eq!(a, b);
        assert_ne!(a, code("USD").unwrap());
        let mut set = std::collections::HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }
}
