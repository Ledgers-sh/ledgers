//! The checks every extraction passes before a draft is made. They do not
//! trust the model: totals are recomputed from the lines, VAT is recomputed
//! from net and rate, dates and currency are validated, and the amounts must
//! appear among the numbers of the document's own text when it has one.
//!
//! A failed check never blocks a draft; it makes the draft need review.

use crate::money::{exponent_of, format_plain};
use crate::schema::{ExtractedInvoice, parse_iso_date};
use time::Date;

/// The VAT rounding tolerance, in minor units, per line.
pub const VAT_TOLERANCE_MINOR: i64 = 1;

/// One check and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// A stable identifier (`lines-net-sum`, `vat-rate`, ...).
    pub id: &'static str,
    /// Whether the check passed.
    pub ok: bool,
    /// What it found, for the review screen and the audit trail.
    pub detail: String,
}

/// The outcome of every check on one extraction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Every check, in a fixed order.
    pub checks: Vec<Check>,
}

impl Report {
    /// True when every check passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }
}

fn check(id: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        id,
        ok,
        detail: detail.into(),
    }
}

/// Rounds `net * rate_bp / 10000` half away from zero, in minor units.
#[must_use]
pub fn vat_for(net_minor: i64, rate_bp: i64) -> i64 {
    let product = i128::from(net_minor) * i128::from(rate_bp);
    let rounded = if product >= 0 {
        (product + 5_000) / 10_000
    } else {
        (product - 5_000) / 10_000
    };
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

/// Runs every check on `invoice`. `today` bounds the date sanity check;
/// `text` is the document's text layer, when there is one.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn run(invoice: &ExtractedInvoice, today: Date, text: Option<&str>) -> Report {
    let mut checks = Vec::new();
    let exponent = invoice.currency.as_deref().and_then(exponent_of);
    checks.push(match (&invoice.currency, exponent) {
        (Some(code), Some(_)) => check("currency", true, format!("{code} is a known currency")),
        (Some(code), None) => check(
            "currency",
            false,
            format!("{code} is not a known currency code"),
        ),
        (None, _) => check("currency", false, "the currency is missing"),
    });
    let exponent = exponent.unwrap_or(2);

    checks.push(match invoice.invoice_date.as_deref() {
        None => check("invoice-date", false, "the invoice date is missing"),
        Some(text) => match parse_iso_date(text) {
            None => check("invoice-date", false, format!("{text} is not a valid date")),
            Some(date) => {
                let years = i64::from(today.year()) - i64::from(date.year());
                let future = date > today.saturating_add(time::Duration::days(1));
                if future || years > 10 {
                    check(
                        "invoice-date",
                        false,
                        format!("{text} is outside the plausible range"),
                    )
                } else {
                    check(
                        "invoice-date",
                        true,
                        format!("{text} is a valid invoice date"),
                    )
                }
            }
        },
    });

    if let (Some(invoice_date), Some(due)) = (
        invoice.invoice_date.as_deref().and_then(parse_iso_date),
        invoice.due_date.as_deref(),
    ) {
        match parse_iso_date(due) {
            Some(due_date) => checks.push(check(
                "due-date",
                due_date >= invoice_date,
                if due_date >= invoice_date {
                    format!("due {due} is on or after the invoice date")
                } else {
                    format!("due {due} is before the invoice date")
                },
            )),
            None => checks.push(check(
                "due-date",
                false,
                format!("{due} is not a valid date"),
            )),
        }
    }

    let totals = (invoice.net_minor, invoice.vat_minor, invoice.gross_minor);
    checks.push(match totals {
        (Some(_), Some(_), Some(_)) => {
            check("totals-present", true, "net, VAT and gross are present")
        }
        _ => check(
            "totals-present",
            false,
            "net, VAT or gross total is missing",
        ),
    });
    if let (Some(net), Some(vat), Some(gross)) = totals {
        let ok = net.checked_add(vat) == Some(gross);
        checks.push(check(
            "gross-equals-net-plus-vat",
            ok,
            format!(
                "{} + {} = {}",
                format_plain(net, exponent),
                format_plain(vat, exponent),
                format_plain(gross, exponent)
            ),
        ));
    }

    if invoice.lines.is_empty() {
        checks.push(check(
            "lines-present",
            false,
            "no invoice lines were extracted",
        ));
    } else {
        checks.push(check(
            "lines-present",
            true,
            format!("{} lines", invoice.lines.len()),
        ));
        let line_nets: Option<Vec<i64>> = invoice.lines.iter().map(|l| l.net_minor).collect();
        if let (Some(nets), Some(net)) = (line_nets, invoice.net_minor) {
            let sum: i64 = nets.iter().sum();
            checks.push(check(
                "lines-net-sum",
                sum == net,
                format!(
                    "lines sum to {}, net total is {}",
                    format_plain(sum, exponent),
                    format_plain(net, exponent)
                ),
            ));
        } else {
            checks.push(check(
                "lines-net-sum",
                false,
                "a line or the net total has no amount",
            ));
        }
        let line_vats: Option<Vec<i64>> = invoice.lines.iter().map(|l| l.vat_minor).collect();
        if let (Some(vats), Some(vat)) = (line_vats, invoice.vat_minor) {
            let sum: i64 = vats.iter().sum();
            checks.push(check(
                "lines-vat-sum",
                sum == vat,
                format!(
                    "line VAT sums to {}, VAT total is {}",
                    format_plain(sum, exponent),
                    format_plain(vat, exponent)
                ),
            ));
        } else {
            checks.push(check(
                "lines-vat-sum",
                false,
                "a line or the VAT total has no amount",
            ));
        }
        let mut bad_rate = Vec::new();
        let mut bad_gross = Vec::new();
        for (index, line) in invoice.lines.iter().enumerate() {
            match (line.net_minor, line.vat_rate_bp, line.vat_minor) {
                (Some(net), Some(rate), Some(vat)) => {
                    if (vat - vat_for(net, rate)).abs() > VAT_TOLERANCE_MINOR {
                        bad_rate.push(index + 1);
                    }
                }
                _ => bad_rate.push(index + 1),
            }
            if let (Some(net), Some(vat), Some(gross)) =
                (line.net_minor, line.vat_minor, line.gross_minor)
                && net.checked_add(vat) != Some(gross)
            {
                bad_gross.push(index + 1);
            }
        }
        checks.push(check(
            "vat-rate",
            bad_rate.is_empty(),
            if bad_rate.is_empty() {
                "every line's VAT is its net times the rate, within rounding".to_owned()
            } else {
                format!("lines {bad_rate:?} do not give net times rate as VAT")
            },
        ));
        checks.push(check(
            "line-gross",
            bad_gross.is_empty(),
            if bad_gross.is_empty() {
                "every line's gross is its net plus VAT".to_owned()
            } else {
                format!("lines {bad_gross:?} have gross different from net plus VAT")
            },
        ));
    }

    checks.push(match text {
        None => check(
            "amounts-in-document",
            true,
            "not compared: the document has no text layer",
        ),
        Some(body) => {
            let numbers = grounded_numbers(body, exponent);
            let missing: Vec<String> = [invoice.net_minor, invoice.vat_minor, invoice.gross_minor]
                .into_iter()
                .flatten()
                .filter(|minor| !numbers.contains(minor))
                .map(|minor| format_plain(minor, exponent))
                .collect();
            check(
                "amounts-in-document",
                missing.is_empty(),
                if missing.is_empty() {
                    "every total appears in the document text".to_owned()
                } else {
                    format!(
                        "totals not found in the document text: {}",
                        missing.join(", ")
                    )
                },
            )
        }
    });

    Report { checks }
}

/// The amounts printed in the document text, as minor units of `exponent`
/// digits, found by the protected-number scanner of `cratefield-text-guard`.
#[must_use]
pub fn grounded_numbers(text: &str, exponent: u32) -> Vec<i64> {
    cratefield_text_guard::extract(text)
        .into_iter()
        .filter(|span| span.kind == cratefield_text_guard::SpanKind::Number)
        .filter_map(|span| crate::money::parse_minor(&span.text, exponent))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ExtractedLine;
    use time::Month;

    fn today() -> Date {
        Date::from_calendar_date(2026, Month::October, 22).expect("date")
    }

    /// A correct two-line invoice: 1000,00 net at 19% and 250,00 net at 7%.
    fn correct() -> ExtractedInvoice {
        let line = |net: i64, rate: i64| ExtractedLine {
            description: Some("Item".into()),
            net_minor: Some(net),
            vat_rate_bp: Some(rate),
            vat_minor: Some(vat_for(net, rate)),
            gross_minor: Some(net + vat_for(net, rate)),
        };
        let lines = vec![line(100_000, 1900), line(25_000, 700)];
        let net: i64 = lines.iter().filter_map(|l| l.net_minor).sum();
        let vat: i64 = lines.iter().filter_map(|l| l.vat_minor).sum();
        ExtractedInvoice {
            currency: Some("EUR".into()),
            invoice_date: Some("2026-10-15".into()),
            due_date: Some("2026-11-14".into()),
            lines,
            net_minor: Some(net),
            vat_minor: Some(vat),
            gross_minor: Some(net + vat),
            ..ExtractedInvoice::default()
        }
    }

    fn failed(report: &Report) -> Vec<&'static str> {
        report
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.id)
            .collect()
    }

    #[test]
    fn a_correct_invoice_passes_every_check() {
        let report = run(&correct(), today(), None);
        assert!(report.passed(), "failed: {:?}", failed(&report));
    }

    #[test]
    fn every_deliberately_wrong_total_is_caught() {
        // Each mutation changes one total or one line; each must fail a check.
        type Mutation = (&'static str, Box<dyn Fn(&mut ExtractedInvoice)>);
        let mutations: Vec<Mutation> = vec![
            (
                "gross off by one cent",
                Box::new(|i| i.gross_minor = i.gross_minor.map(|g| g + 1)),
            ),
            (
                "net off by 100",
                Box::new(|i| i.net_minor = i.net_minor.map(|n| n + 100)),
            ),
            (
                "vat off by 5 euro",
                Box::new(|i| i.vat_minor = i.vat_minor.map(|v| v + 500)),
            ),
            (
                "line net changed",
                Box::new(|i| i.lines[1].net_minor = Some(20_000)),
            ),
            (
                "line rate wrong",
                Box::new(|i| i.lines[0].vat_rate_bp = Some(700)),
            ),
            (
                "line gross wrong",
                Box::new(|i| i.lines[0].gross_minor = Some(1)),
            ),
            (
                "currency unknown",
                Box::new(|i| i.currency = Some("ZZZ".into())),
            ),
            (
                "due before invoice",
                Box::new(|i| i.due_date = Some("2026-10-01".into())),
            ),
            (
                "date from the future",
                Box::new(|i| i.invoice_date = Some("2031-01-01".into())),
            ),
            ("no lines", Box::new(|i| i.lines.clear())),
        ];
        for (name, mutate) in mutations {
            let mut invoice = correct();
            mutate(&mut invoice);
            assert!(
                !run(&invoice, today(), None).passed(),
                "{name} was not caught"
            );
        }
    }

    #[test]
    fn a_total_absent_from_the_document_text_is_flagged() {
        // The fixture invoice: net 1.250,00, VAT 207,50, gross 1.457,50.
        let text = "Invoice 42\nGesamt 1.457,50 EUR\nNetto 1.250,00 EUR\nMwSt 207,50 EUR";
        let report = run(&correct(), today(), Some(text));
        assert!(
            report.passed(),
            "matching text should pass: {:?}",
            failed(&report)
        );
        let mut wrong_text = correct();
        wrong_text.gross_minor = Some(999_999);
        let report = run(&wrong_text, today(), Some(text));
        assert!(failed(&report).contains(&"amounts-in-document"));
    }

    #[test]
    fn vat_rounding_within_one_cent_passes() {
        assert_eq!(vat_for(1_005, 1900), 191);
        assert_eq!(vat_for(-1_005, 1900), -191);
        let mut invoice = correct();
        invoice.lines[0].vat_minor = invoice.lines[0].vat_minor.map(|v| v + 1);
        invoice.vat_minor = invoice.vat_minor.map(|v| v + 1);
        invoice.gross_minor = invoice.gross_minor.map(|g| g + 1);
        let report = run(&invoice, today(), None);
        assert!(report.checks.iter().any(|c| c.id == "vat-rate" && c.ok));
    }
}
