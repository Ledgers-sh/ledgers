//! Invoice template rendering (HTML).
//! Document text is data: all user input is escaped.

use crate::model::{DocumentLine, InvoiceDocument, TaxBreakdown};

/// Escape HTML special characters to prevent XSS.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Render an invoice to HTML.
/// All text is escaped; document text is data, never instructions.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_invoice(doc: &InvoiceDocument) -> String {
    let seller_tax = doc
        .seller_tax_number
        .as_deref()
        .or(doc.seller_vat_id.as_deref())
        .unwrap_or("(no tax number)");

    let note_html = doc.note.as_ref().map(|n| {
        format!(
            "<div style=\"margin-top: 1em; padding: 0.5em; background: #f5f5f5;\">{}</div>",
            escape_html(n).replace('\n', "<br/>")
        )
    });

    format!(
        "<!DOCTYPE html>
<html>
<head>
  <meta charset=\"UTF-8\" />
  <title>Invoice {}</title>
  <style>
    body {{ font-family: sans-serif; margin: 2em; }}
    .header {{ display: flex; justify-content: space-between; margin-bottom: 2em; }}
    .seller, .buyer {{ flex: 1; }}
    table {{ width: 100%; border-collapse: collapse; margin-top: 1em; }}
    th, td {{ border: 1px solid #ccc; padding: 0.5em; text-align: left; }}
    th {{ background: #f5f5f5; }}
    .number-right {{ text-align: right; }}
    .totals {{ margin-top: 1em; font-weight: bold; }}
  </style>
</head>
<body>
  <h1>Invoice {}</h1>

  <div class=\"header\">
    <div class=\"seller\">
      <h3>Seller</h3>
      <p><strong>{}</strong></p>
      <p>{}</p>
      <p>Tax Number: {}</p>
    </div>
    <div class=\"buyer\">
      <h3>Buyer</h3>
      <p><strong>{}</strong></p>
      <p>{}</p>
      {}
    </div>
  </div>

  <table>
    <tr>
      <th>Issue Date</th>
      <td>{}</td>
    </tr>
    <tr>
      <th>Supply Date</th>
      <td>{}</td>
    </tr>
  </table>

  <h3>Lines</h3>
  <table>
    <tr>
      <th>#</th>
      <th>Description</th>
      <th>Quantity</th>
      <th>Unit</th>
      <th class=\"number-right\">Net ({})</th>
    </tr>
    {}
  </table>

  <h3>Tax Breakdown</h3>
  <table>
    <tr>
      <th>Rate/Kind</th>
      <th class=\"number-right\">Net ({})</th>
      <th class=\"number-right\">Tax ({})</th>
    </tr>
    {}
  </table>

  <div class=\"totals\">
    <p>Net: {} {}</p>
    <p>Tax: {} {}</p>
    <p>Total: {} {}</p>
  </div>

  {}
  {}
  {}
  {}

</body>
</html>",
        escape_html(&doc.invoice_number),
        escape_html(&doc.invoice_number),
        escape_html(&doc.seller_name),
        escape_html(&doc.seller_address),
        seller_tax,
        escape_html(&doc.buyer_name),
        escape_html(&doc.buyer_address),
        doc.buyer_vat_id
            .as_ref()
            .map(|id| format!("<p>VAT ID: {}</p>", escape_html(id)))
            .unwrap_or_default(),
        escape_html(&doc.issue_date),
        escape_html(&doc.supply_date),
        doc.currency,
        render_lines(&doc.lines, &doc.currency),
        doc.currency,
        doc.currency,
        render_tax_breakdown(&doc.tax_breakdown, &doc.currency),
        format_money(doc.totals.net_minor),
        doc.currency,
        format_money(doc.totals.tax_minor),
        doc.currency,
        format_money(doc.totals.gross_minor),
        doc.currency,
        doc.due_date
            .as_ref()
            .map(|due| format!("<p>Due Date: {}</p>", escape_html(due)))
            .unwrap_or_default(),
        doc.intra_eu_note
            .as_ref()
            .map(|n| format!("<div style=\"margin-top: 1em; padding: 0.5em; background: #fff3cd; border: 1px solid #ffc107;\"><strong>Intra-EU Supply:</strong> {}</div>", escape_html(n)))
            .unwrap_or_default(),
        doc.reverse_charge_note
            .as_ref()
            .map(|n| format!("<div style=\"margin-top: 1em; padding: 0.5em; background: #fff3cd; border: 1px solid #ffc107;\"><strong>Reverse Charge:</strong> {}</div>", escape_html(n)))
            .unwrap_or_default(),
        note_html.unwrap_or_default(),
    )
}

fn render_lines(lines: &[DocumentLine], currency: &str) -> String {
    lines
        .iter()
        .map(|line| {
            let unit = line
                .unit
                .as_ref()
                .map(|u| escape_html(u))
                .unwrap_or_default();
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"number-right\">{} {}</td></tr>",
                line.sequence,
                escape_html(&line.description),
                line.quantity,
                unit,
                format_money(line.net_minor),
                currency
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

fn render_tax_breakdown(breakdown: &[TaxBreakdown], currency: &str) -> String {
    breakdown
        .iter()
        .map(|tb| {
            format!(
                "<tr><td>{} ({})</td><td class=\"number-right\">{} {}</td><td class=\"number-right\">{} {}</td></tr>",
                escape_html(&tb.tax_rate_percent),
                escape_html(&tb.kind),
                format_money(tb.net_minor),
                currency,
                format_money(tb.tax_minor),
                currency,
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

/// Format minor units as a decimal string (e.g. 1234 EUR cents → "12.34").
fn format_money(minor: i64) -> String {
    let major = minor / 100;
    let cents = (minor % 100).abs();
    format!("{major}.{cents:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InvoiceTotals;

    #[test]
    fn test_html_escaping() {
        let doc = InvoiceDocument {
            seller_name: "Halden Works GmbH".to_string(),
            seller_address: "1234 Nowhere St".to_string(),
            seller_tax_number: Some("DE123456789".to_string()),
            seller_vat_id: None,
            buyer_name: "Acme GmbH".to_string(),
            buyer_address: "5678 Somewhere Ave".to_string(),
            buyer_vat_id: None,
            invoice_number: "RE-2026-0001".to_string(),
            issue_date: "2026-10-24".to_string(),
            supply_date: "2026-10-24".to_string(),
            lines: vec![DocumentLine {
                sequence: 1,
                description: "Test with <script>alert('xss')</script>".to_string(),
                quantity: "1".to_string(),
                unit: None,
                net_minor: 10000,
                tax_rate_percent: Some("19.00".to_string()),
                tax_kind: Some("standard".to_string()),
            }],
            tax_breakdown: vec![TaxBreakdown {
                tax_rate_percent: "19%".to_string(),
                net_minor: 10000,
                tax_minor: 1900,
                kind: "standard".to_string(),
            }],
            currency: "EUR".to_string(),
            totals: InvoiceTotals {
                net_minor: 10000,
                tax_minor: 1900,
                gross_minor: 11900,
                currency: "EUR".to_string(),
            },
            due_date: None,
            note: None,
            intra_eu_note: None,
            reverse_charge_note: None,
        };

        let html = render_invoice(&doc);
        // Script tag should be escaped, not executable
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_german_ustg_section_14_fields() {
        let doc = InvoiceDocument {
            seller_name: "Halden Works GmbH".to_string(),
            seller_address: "Haldenweg 1, 70174 Stuttgart, Germany".to_string(),
            seller_tax_number: Some("DE 123 456 789".to_string()),
            seller_vat_id: Some("DE123456789".to_string()),
            buyer_name: "Acme GmbH".to_string(),
            buyer_address: "Main Street 1, 10115 Berlin, Germany".to_string(),
            buyer_vat_id: Some("DE987654321".to_string()),
            invoice_number: "RE-2026-0001".to_string(),
            issue_date: "2026-10-24".to_string(),
            supply_date: "2026-10-23".to_string(),
            lines: vec![DocumentLine {
                sequence: 1,
                description: "Consulting services".to_string(),
                quantity: "10".to_string(),
                unit: Some("h".to_string()),
                net_minor: 50000,
                tax_rate_percent: Some("19.00".to_string()),
                tax_kind: Some("standard".to_string()),
            }],
            tax_breakdown: vec![TaxBreakdown {
                tax_rate_percent: "19%".to_string(),
                net_minor: 50000,
                tax_minor: 9500,
                kind: "standard".to_string(),
            }],
            currency: "EUR".to_string(),
            totals: InvoiceTotals {
                net_minor: 50000,
                tax_minor: 9500,
                gross_minor: 59500,
                currency: "EUR".to_string(),
            },
            due_date: Some("2026-11-07".to_string()),
            note: None,
            intra_eu_note: None,
            reverse_charge_note: None,
        };

        let html = render_invoice(&doc);

        // §14 Abs. 4 UStG required fields
        assert!(html.contains("Halden Works GmbH"), "seller full name");
        assert!(
            html.contains("Haldenweg 1, 70174 Stuttgart, Germany"),
            "seller address"
        );
        assert!(html.contains("DE 123 456 789"), "seller tax number");
        assert!(html.contains("Acme GmbH"), "buyer full name");
        assert!(
            html.contains("Main Street 1, 10115 Berlin, Germany"),
            "buyer address"
        );
        assert!(html.contains("2026-10-24"), "issue date");
        assert!(html.contains("RE-2026-0001"), "invoice number");
        assert!(html.contains("10"), "quantity");
        assert!(
            html.contains("Consulting services"),
            "description of services"
        );
        assert!(html.contains("2026-10-23"), "time of supply");
        assert!(html.contains("500.00"), "net amount (5000 cents)");
        assert!(html.contains("19%"), "tax rate");
        assert!(html.contains("95.00"), "tax amount (9500 cents)");

        // Test intra-EU zero-rated case with exemption note
        let intra_eu_doc = InvoiceDocument {
            seller_name: "Halden Works GmbH".to_string(),
            seller_address: "Haldenweg 1, 70174 Stuttgart, Germany".to_string(),
            seller_tax_number: Some("DE 123 456 789".to_string()),
            seller_vat_id: Some("DE123456789".to_string()),
            buyer_name: "Fjordlys AB".to_string(),
            buyer_address: "Stockholm, Sweden".to_string(),
            buyer_vat_id: Some("SE556614220001".to_string()),
            invoice_number: "RE-2026-0002".to_string(),
            issue_date: "2026-10-24".to_string(),
            supply_date: "2026-10-23".to_string(),
            lines: vec![DocumentLine {
                sequence: 1,
                description: "Consulting services".to_string(),
                quantity: "10".to_string(),
                unit: Some("h".to_string()),
                net_minor: 50000,
                tax_rate_percent: Some("0.00".to_string()),
                tax_kind: Some("intra-eu".to_string()),
            }],
            tax_breakdown: vec![TaxBreakdown {
                tax_rate_percent: "0%".to_string(),
                net_minor: 50000,
                tax_minor: 0,
                kind: "intra-eu".to_string(),
            }],
            currency: "EUR".to_string(),
            totals: InvoiceTotals {
                net_minor: 50000,
                tax_minor: 0,
                gross_minor: 50000,
                currency: "EUR".to_string(),
            },
            due_date: Some("2026-11-07".to_string()),
            note: None,
            intra_eu_note: Some(
                "Steuerfreie innergemeinschaftliche Lieferung (§ 4 Nr. 1b i.V.m. § 6a UStG)"
                    .to_string(),
            ),
            reverse_charge_note: None,
        };

        let intra_eu_html = render_invoice(&intra_eu_doc);

        // Check seller fields
        assert!(
            intra_eu_html.contains("Halden Works GmbH"),
            "seller name in intra-EU"
        );
        assert!(
            intra_eu_html.contains("DE 123 456 789"),
            "seller tax number in intra-EU"
        );

        // Check buyer fields
        assert!(
            intra_eu_html.contains("Fjordlys AB"),
            "buyer name in intra-EU"
        );
        assert!(
            intra_eu_html.contains("SE556614220001"),
            "buyer VAT ID in intra-EU (§14a)"
        );

        // Check document data
        assert!(
            intra_eu_html.contains("RE-2026-0002"),
            "invoice number in intra-EU"
        );
        assert!(
            intra_eu_html.contains("2026-10-24"),
            "issue date in intra-EU"
        );
        assert!(intra_eu_html.contains("10"), "quantity in intra-EU");
        assert!(
            intra_eu_html.contains("Consulting services"),
            "description in intra-EU"
        );
        assert!(
            intra_eu_html.contains("2026-10-23"),
            "supply date in intra-EU"
        );
        assert!(intra_eu_html.contains("500.00"), "net amount in intra-EU");

        // Check exemption note
        assert!(
            intra_eu_html.contains("Steuerfreie innergemeinschaftliche Lieferung"),
            "exemption note in intra-EU"
        );
        assert!(
            intra_eu_html.contains("§ 4 Nr. 1b"),
            "UStG reference in intra-EU"
        );
    }
}
