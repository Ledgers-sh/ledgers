//! UBL 2.1 Invoice → bill schema (EN 16931 business terms in comments).
use super::xml::Doc;
use super::{EInvoiceError, MAX_LINES, Out, in_currency};

pub fn map(doc: &Doc, out: &mut Out) -> Result<(), EInvoiceError> {
    let root = doc.root();
    out.currency(doc.child(root, "DocumentCurrencyCode")); // BT-5
    out.invoice.invoice_number = out.text("invoice_number", doc.child(root, "ID")); // BT-1
    out.invoice.invoice_date = out.date("invoice_date", doc.child(root, "IssueDate")); // BT-2
    out.invoice.due_date = out.date("due_date", doc.child(root, "DueDate")); // BT-9
    let party = doc.path(root, &["AccountingSupplierParty", "Party"]);
    let name = party.and_then(|p| {
        doc.path(p, &["PartyLegalEntity", "RegistrationName"])
            .or_else(|| doc.path(p, &["PartyName", "Name"]))
    });
    out.invoice.vendor_name = out.text("vendor_name", name); // BT-27
    let vat = party
        .into_iter()
        .flat_map(|p| doc.children(p, "PartyTaxScheme"))
        .find(|s| {
            doc.path(s, &["TaxScheme", "ID"])
                .is_some_and(|id| id.text.trim() == "VAT")
        })
        .and_then(|s| doc.child(s, "CompanyID"));
    out.invoice.vendor_vat_id = out.text("vendor_vat_id", vat); // BT-31
    let iban_node = doc.path(root, &["PaymentMeans", "PayeeFinancialAccount", "ID"]);
    out.invoice.vendor_iban = out
        .text("vendor_iban", iban_node)
        .map(|iban| iban.replace(' ', ""));
    let totals = doc.child(root, "LegalMonetaryTotal");
    out.invoice.net_minor = out.amount(
        "net_minor",
        totals.and_then(|t| doc.child(t, "TaxExclusiveAmount")),
    ); // BT-109
    let currency = out.invoice.currency.clone();
    let vat_total = in_currency(
        doc.children(root, "TaxTotal")
            .filter_map(|t| doc.child(t, "TaxAmount")),
        currency.as_deref(),
    );
    out.invoice.vat_minor = out.amount("vat_minor", vat_total); // BT-110
    out.invoice.gross_minor = out.amount(
        "gross_minor",
        totals.and_then(|t| doc.child(t, "TaxInclusiveAmount")),
    ); // BT-112
    let lines: Vec<_> = doc.children(root, "InvoiceLine").collect();
    if lines.len() > MAX_LINES {
        return Err(EInvoiceError::TooManyLines);
    }
    for (i, line) in lines.into_iter().enumerate() {
        let item = doc.child(line, "Item");
        out.line(
            i,
            item.and_then(|it| doc.child(it, "Name")),
            doc.child(line, "LineExtensionAmount"),
            item.and_then(|it| doc.path(it, &["ClassifiedTaxCategory", "Percent"])),
        ); // BT-153, BT-131, BT-152
    }
    Ok(())
}
