use super::xml::Doc;
use super::{EInvoiceError, MAX_LINES, Out, in_currency};
pub fn map(doc: &Doc, out: &mut Out) -> Result<(), EInvoiceError> {
    let root = doc.root();
    let header = doc.child(root, "ExchangedDocument");
    let tx = doc.child(root, "SupplyChainTradeTransaction");
    if let Some(h) = header {
        out.invoice.invoice_number = out.text("invoice_number", doc.child(h, "ID"));
        if let Some(dt) = doc.child(h, "IssueDateTime") {
            out.invoice.invoice_date = out.date("invoice_date", doc.child(dt, "DateTimeString"));
        }
    }
    if let Some(t) = tx {
        if let Some(sett) = doc.child(t, "ApplicableHeaderTradeSettlement") {
            out.currency(doc.child(sett, "InvoiceCurrencyCode"));
            if let Some(terms) = doc.children(sett, "SpecifiedTradePaymentTerms").next()
                && let Some(dt) = doc.child(terms, "DueDateDateTime")
            {
                out.invoice.due_date = out.date("due_date", doc.child(dt, "DateTimeString"));
            }
            if let Some(seller) = doc
                .child(t, "ApplicableHeaderTradeAgreement")
                .and_then(|a| doc.child(a, "SellerTradeParty"))
            {
                out.invoice.vendor_name = out.text("vendor_name", doc.child(seller, "Name"));
                let vat = doc
                    .children(seller, "SpecifiedTaxRegistration")
                    .find(|r| {
                        doc.child(r, "ID")
                            .is_some_and(|id| id.attr("schemeID") == Some("VA"))
                    })
                    .and_then(|r| doc.child(r, "ID"));
                out.invoice.vendor_vat_id = out.text("vendor_vat_id", vat);
                let iban = doc
                    .children(sett, "SpecifiedTradeSettlementPaymentMeans")
                    .find_map(|m| {
                        doc.child(m, "PayeePartyCreditorFinancialAccount")
                            .and_then(|a| doc.child(a, "IBANID"))
                    });
                out.invoice.vendor_iban = out.text("vendor_iban", iban).map(|v| v.replace(' ', ""));
            }
            if let Some(sums) = doc.child(sett, "SpecifiedTradeSettlementHeaderMonetarySummation") {
                out.invoice.net_minor =
                    out.amount("net_minor", doc.child(sums, "TaxBasisTotalAmount"));
                let currency = out.invoice.currency.clone();
                let vat_total =
                    in_currency(doc.children(sums, "TaxTotalAmount"), currency.as_deref());
                out.invoice.vat_minor = out.amount("vat_minor", vat_total);
                out.invoice.gross_minor =
                    out.amount("gross_minor", doc.child(sums, "GrandTotalAmount"));
            }
        }
        let lines: Vec<_> = doc
            .children(t, "IncludedSupplyChainTradeLineItem")
            .collect();
        if lines.len() > MAX_LINES {
            return Err(EInvoiceError::TooManyLines);
        }
        for (i, line) in lines.into_iter().enumerate() {
            let product = doc.child(line, "SpecifiedTradeProduct");
            let ls = doc.child(line, "SpecifiedLineTradeSettlement");
            let net = ls.and_then(|s| {
                doc.child(s, "SpecifiedTradeSettlementLineMonetarySummation")
                    .and_then(|m| doc.child(m, "LineTotalAmount"))
            });
            let rate = ls.and_then(|s| {
                doc.children(s, "ApplicableTradeTax")
                    .find_map(|t| doc.child(t, "RateApplicablePercent"))
            });
            out.line(i, product.and_then(|p| doc.child(p, "Name")), net, rate);
        }
    }
    Ok(())
}
