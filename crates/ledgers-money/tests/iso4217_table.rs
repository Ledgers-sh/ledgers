//! The ISO 4217 acceptance test: the production table in
//! `src/currency.rs` and the fixture below must agree entry for entry, in
//! the same order, so neither can drift silently.
//!
//! Sources (retrieved 2026-10-18):
//!
//! - Primary (active codes): Wikipedia, "ISO 4217",
//!   <https://en.wikipedia.org/wiki/ISO_4217> — the article's
//!   active-codes table, which lists **178** rows; the 4 precious-metal
//!   codes (XAU, XAG, XPD, XPT) are deliberately excluded here, leaving
//!   the 174 active codes below.
//! - Secondary (cross-check of numeric codes and exponents, and the
//!   historic withdrawal record): ISO 4217 maintenance-agency data
//!   mirrored by datasets/currency-codes,
//!   <https://raw.githubusercontent.com/datasets/currency-codes/master/data/codes-all.csv>.
//!
//! Every active exponent and numeric matched between the two sources with
//! zero disagreements. Historic numerics were additionally validated
//! against the last official SIX "list three" snapshot (published
//! 2018-08-20, archived at
//! <https://web.archive.org/web/2020/https://www.currency-iso.org/dam/downloads/lists/list_three.xml>).

use ledgers_money::{Currency, MoneyError};

/// The source snapshot's own count of active codes (see the header: 178
/// table rows minus the 4 excluded metal codes).
const EXPECTED_ACTIVE: usize = 174;

/// Curated historic codes (a subset of ISO 4217 List Three; see the
/// currency module docs).
const EXPECTED_HISTORIC: usize = 45;

/// One line per production-table entry, in the same (code-sorted) order:
/// `CODE NUM EXP ACTIVE`.
const FIXTURE: &str = r"
AED 784 2 active
AFA 4 2 historic
AFN 971 2 active
ALL 8 2 active
AMD 51 2 active
ANG 532 2 historic
AOA 973 2 active
ARS 32 2 active
ATS 40 2 historic
AUD 36 2 active
AWG 533 2 active
AZN 944 2 active
BAM 977 2 active
BBD 52 2 active
BDT 50 2 active
BEF 56 2 historic
BGN 975 2 historic
BHD 48 3 active
BIF 108 0 active
BMD 60 2 active
BND 96 2 active
BOB 68 2 active
BOV 984 2 active
BRL 986 2 active
BSD 44 2 active
BTN 64 2 active
BWP 72 2 active
BYN 933 2 active
BYR 974 0 historic
BZD 84 2 active
CAD 124 2 active
CDF 976 2 active
CHE 947 2 active
CHF 756 2 active
CHW 948 2 active
CLF 990 4 active
CLP 152 0 active
CNY 156 2 active
COP 170 2 active
COU 970 2 active
CRC 188 2 active
CSK 200 2 historic
CUC 931 2 historic
CUP 192 2 active
CVE 132 2 active
CYP 196 2 historic
CZK 203 2 active
DEM 276 2 historic
DJF 262 0 active
DKK 208 2 active
DOP 214 2 active
DZD 12 2 active
EEK 233 2 historic
EGP 818 2 active
ERN 232 2 active
ESP 724 0 historic
ETB 230 2 active
EUR 978 2 active
FIM 246 2 historic
FJD 242 2 active
FKP 238 2 active
FRF 250 2 historic
GBP 826 2 active
GEL 981 2 active
GHS 936 2 active
GIP 292 2 active
GMD 270 2 active
GNF 324 0 active
GRD 300 0 historic
GTQ 320 2 active
GYD 328 2 active
HKD 344 2 active
HNL 340 2 active
HRK 191 2 historic
HTG 332 2 active
HUF 348 2 active
IDR 360 2 active
IEP 372 2 historic
ILS 376 2 active
INR 356 2 active
IQD 368 3 active
IRR 364 2 active
ISK 352 0 active
ITL 380 0 historic
JMD 388 2 active
JOD 400 3 active
JPY 392 0 active
KES 404 2 active
KGS 417 2 active
KHR 116 2 active
KMF 174 0 active
KPW 408 2 active
KRW 410 0 active
KWD 414 3 active
KYD 136 2 active
KZT 398 2 active
LAK 418 2 active
LBP 422 2 active
LKR 144 2 active
LRD 430 2 active
LSL 426 2 active
LTL 440 2 historic
LUF 442 2 historic
LVL 428 2 historic
LYD 434 3 active
MAD 504 2 active
MDL 498 2 active
MGA 969 2 active
MGF 450 0 historic
MKD 807 2 active
MMK 104 2 active
MNT 496 2 active
MOP 446 2 active
MRO 478 2 historic
MRU 929 2 active
MTL 470 2 historic
MUR 480 2 active
MVR 462 2 active
MWK 454 2 active
MXN 484 2 active
MXV 979 2 active
MYR 458 2 active
MZM 508 2 historic
MZN 943 2 active
NAD 516 2 active
NGN 566 2 active
NIO 558 2 active
NLG 528 2 historic
NOK 578 2 active
NPR 524 2 active
NZD 554 2 active
OMR 512 3 active
PAB 590 2 active
PEN 604 2 active
PGK 598 2 active
PHP 608 2 active
PKR 586 2 active
PLN 985 2 active
PTE 620 0 historic
PYG 600 0 active
QAR 634 2 active
ROL 642 0 historic
RON 946 2 active
RSD 941 2 active
RUB 643 2 active
RUR 810 2 historic
RWF 646 0 active
SAR 682 2 active
SBD 90 2 active
SCR 690 2 active
SDD 736 2 historic
SDG 938 2 active
SEK 752 2 active
SGD 702 2 active
SHP 654 2 active
SIT 705 2 historic
SKK 703 2 historic
SLE 925 2 active
SLL 694 2 historic
SOS 706 2 active
SRD 968 2 active
SRG 740 2 historic
SSP 728 2 active
STD 678 2 historic
STN 930 2 active
SVC 222 2 active
SYP 760 2 active
SZL 748 2 active
THB 764 2 active
TJS 972 2 active
TMM 795 2 historic
TMT 934 2 active
TND 788 3 active
TOP 776 2 active
TRL 792 0 historic
TRY 949 2 active
TTD 780 2 active
TWD 901 2 active
TZS 834 2 active
UAH 980 2 active
UGX 800 0 active
USD 840 2 active
USN 997 2 active
UYI 940 0 active
UYU 858 2 active
UYW 927 4 active
UZS 860 2 active
VEB 862 2 historic
VED 926 2 active
VEF 937 2 historic
VES 928 2 active
VND 704 0 active
VUV 548 0 active
WST 882 2 active
XAD 396 2 active
XAF 950 0 active
XBA 955 0 active
XBB 956 0 active
XBC 957 0 active
XBD 958 0 active
XCD 951 2 active
XCG 532 2 active
XDR 960 0 active
XEU 954 0 historic
XOF 952 0 active
XPF 953 0 active
XSU 994 0 active
XTS 963 0 active
XUA 965 0 active
XXX 999 0 active
YER 886 2 active
YUM 891 2 historic
ZAL 991 2 historic
ZAR 710 2 active
ZMK 894 2 historic
ZMW 967 2 active
ZWD 716 2 historic
ZWG 924 2 active
ZWL 932 2 historic
";

#[derive(Debug)]
struct Row {
    code: String,
    numeric: u16,
    exponent: u32,
    active: bool,
}

fn parse_fixture() -> Vec<Row> {
    let mut rows = Vec::new();
    for line in FIXTURE.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let code = fields.next().expect("fixture code column");
        let numeric: u16 = fields
            .next()
            .expect("fixture num column")
            .parse()
            .expect("fixture numeric");
        let exponent: u32 = fields
            .next()
            .expect("fixture exp column")
            .parse()
            .expect("fixture exponent");
        let active = match fields.next().expect("fixture flag column") {
            "active" => true,
            "historic" => false,
            other => panic!("fixture flag must be active|historic, got {other}"),
        };
        assert!(
            fields.next().is_none(),
            "extra fields in fixture line {line:?}"
        );
        rows.push(Row {
            code: code.to_owned(),
            numeric,
            exponent,
            active,
        });
    }
    rows
}

#[test]
fn every_fixture_line_loads_with_exact_data() {
    for row in parse_fixture() {
        let currency = Currency::from_code(&row.code)
            .unwrap_or_else(|e| panic!("fixture code {} should parse: {e}", row.code));
        assert_eq!(currency.numeric(), row.numeric, "{} numeric", row.code);
        assert_eq!(currency.exponent(), row.exponent, "{} exponent", row.code);
        assert_eq!(currency.is_active(), row.active, "{} active flag", row.code);
        // The fixture is the source of truth for order, too.
        assert_eq!(currency.to_string(), row.code);
    }
}

#[test]
fn active_code_count_matches_the_source_snapshot() {
    let fixture = parse_fixture();
    let active = fixture.iter().filter(|r| r.active).count();
    let historic = fixture.iter().filter(|r| !r.active).count();
    assert_eq!(active, EXPECTED_ACTIVE, "active codes in table and fixture");
    assert_eq!(
        historic, EXPECTED_HISTORIC,
        "historic codes in table and fixture"
    );
    let total: Vec<Currency> = Currency::all().collect();
    assert_eq!(
        total.len(),
        fixture.len(),
        "table and fixture sizes must agree"
    );
}

#[test]
fn every_table_currency_is_present_in_the_fixture() {
    let fixture = parse_fixture();
    for (i, currency) in Currency::all().enumerate() {
        let row = fixture
            .get(i)
            .unwrap_or_else(|| panic!("table has more entries than the fixture at {i}"));
        assert_eq!(currency.code(), row.code, "entry {i} must match in order");
        assert_eq!(currency.numeric(), row.numeric, "{} numeric", row.code);
        assert_eq!(currency.exponent(), row.exponent, "{} exponent", row.code);
        assert_eq!(currency.is_active(), row.active, "{} active flag", row.code);
    }
}

#[test]
fn the_issues_named_examples() {
    let eur = Currency::from_code("EUR").unwrap();
    let jpy = Currency::from_code("JPY").unwrap();
    let bhd = Currency::from_code("BHD").unwrap();
    let kwd = Currency::from_code("KWD").unwrap();
    let clf = Currency::from_code("CLF").unwrap();
    let zwg = Currency::from_code("ZWG").unwrap();
    let zwl = Currency::from_code("ZWL").unwrap();

    assert_eq!(jpy.exponent(), 0);
    assert_eq!(jpy.numeric(), 392);
    assert_eq!(eur.exponent(), 2);
    assert_eq!(eur.numeric(), 978);
    assert_eq!(bhd.exponent(), 3);
    assert_eq!(kwd.exponent(), 3);
    assert_eq!(clf.exponent(), 4);
    assert!(zwg.is_active());
    assert_eq!(zwg.exponent(), 2);
    assert!(!zwl.is_active(), "ZWL was replaced by ZWG in 2024");
    assert_eq!(zwl.exponent(), 2);
    assert_eq!(zwl.numeric(), 932);

    // Retired codes are kept, never deleted: every currency the euro
    // replaced (Croatia 2023, Bulgaria 2026 included) and ANG, which XCG
    // replaced in 2025, still parse so old entries in them keep loading.
    for (code, numeric) in [("BGN", 975), ("HRK", 191), ("LTL", 440), ("ANG", 532)] {
        let c = Currency::from_code(code).unwrap_or_else(|e| panic!("{code}: {e}"));
        assert!(!c.is_active(), "{code} is retired");
        assert_eq!(c.exponent(), 2, "{code} exponent");
        assert_eq!(c.numeric(), numeric, "{code} numeric");
    }

    // More spot checks from the issue.
    for (code, exp) in [
        ("ISK", 0),
        ("UYI", 0),
        ("UYW", 4),
        ("TND", 3),
        ("VES", 2),
        ("VED", 2),
        ("SLE", 2),
        ("MGA", 2),
        ("MRU", 2),
        ("VUV", 0),
        ("CLP", 0),
        ("KRW", 0),
        ("VND", 0),
        ("MXV", 2),
        ("COU", 2),
        ("USN", 2),
        ("CHE", 2),
        ("CHW", 2),
        ("BOV", 2),
    ] {
        assert_eq!(
            Currency::from_code(code).unwrap().exponent(),
            exp,
            "{code} exponent"
        );
    }

    // Crypto comes later, behind the reserved `crypto` feature: USDC is
    // not a code today (it is not even a well-formed ISO code).
    assert!(Currency::from_code("USDC").is_err());

    // Numeric lookups, both directions.
    assert_eq!(Currency::from_numeric(978).unwrap(), eur);
    assert_eq!(Currency::from_numeric(392).unwrap(), jpy);
    assert_eq!(Currency::from_numeric(932).unwrap(), zwl);
    assert!(matches!(
        Currency::from_numeric(42),
        Err(MoneyError::UnknownNumericCode { numeric: 42 })
    ));
}
