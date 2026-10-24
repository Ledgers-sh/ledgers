//! E-invoice generation tests with `KoSIT` validation.

use ledgers_invoice::model::{DocumentLine, InvoiceDocument, InvoiceTotals, TaxBreakdown};
use ledgers_invoice::{
    ContactInfo, EInvoiceDocument, PartyInfo, PaymentMeans, Profile, render_einvoice,
};
use std::fs;
use std::path::Path;

/// Validate that XML bytes are well-formed by parsing with quick-xml.
fn validate_xml_wellformed(xml_bytes: &[u8]) {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_reader(xml_bytes);
    let mut buf = Vec::new();

    // Iterate through all XML events to check for well-formedness
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                panic!("XML well-formedness error: {e}");
            }
        }
    }
}

fn create_seller() -> PartyInfo {
    PartyInfo {
        name: "Halden Works GmbH".to_string(),
        street: "Haldenweg 1".to_string(),
        city: "Berlin".to_string(),
        postal_code: "10115".to_string(),
        country_code: "DE".to_string(),
        vat_id: Some("DE123456789".to_string()),
        tax_number: None,
        email: Some("info@haldenworks.de".to_string()),
    }
}

fn create_acme_buyer() -> PartyInfo {
    PartyInfo {
        name: "Acme GmbH".to_string(),
        street: "Main Street 1".to_string(),
        city: "Berlin".to_string(),
        postal_code: "10115".to_string(),
        country_code: "DE".to_string(),
        vat_id: Some("DE987654321".to_string()),
        tax_number: None,
        email: Some("ap@acme.de".to_string()),
    }
}

fn create_fjordlys_buyer() -> PartyInfo {
    PartyInfo {
        name: "Fjordlys AB".to_string(),
        street: "Stockholm Vägen 1".to_string(),
        city: "Stockholm".to_string(),
        postal_code: "17000".to_string(),
        country_code: "SE".to_string(),
        vat_id: Some("SE556614220001".to_string()),
        tax_number: None,
        email: Some("accounting@fjordlys.se".to_string()),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_domestic_invoice_with_mixed_rates() {
    // Scenario (a): domestic invoice with 19% and 7% VAT
    let base_doc = InvoiceDocument {
        seller_name: "Halden Works GmbH".to_string(),
        seller_address: "Haldenweg 1, Berlin".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Acme GmbH".to_string(),
        buyer_address: "Main Street 1, Berlin".to_string(),
        buyer_vat_id: Some("DE987654321".to_string()),
        invoice_number: "RE-2026-0001".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![
            DocumentLine {
                sequence: 1,
                description: "Software development (100 hours)".to_string(),
                quantity: "100".to_string(),
                unit: Some("h".to_string()),
                net_minor: 100_000,
                tax_rate_percent: Some("19.00".to_string()),
                tax_kind: Some("standard".to_string()),
            },
            DocumentLine {
                sequence: 2,
                description: "Hardware (server)".to_string(),
                quantity: "1".to_string(),
                unit: Some("pcs".to_string()),
                net_minor: 50000,
                tax_rate_percent: Some("19.00".to_string()),
                tax_kind: Some("standard".to_string()),
            },
            DocumentLine {
                sequence: 3,
                description: "Support services (20 hours)".to_string(),
                quantity: "20".to_string(),
                unit: Some("h".to_string()),
                net_minor: 40000,
                tax_rate_percent: Some("19.00".to_string()),
                tax_kind: Some("standard".to_string()),
            },
            DocumentLine {
                sequence: 4,
                description: "Training materials".to_string(),
                quantity: "5".to_string(),
                unit: Some("pcs".to_string()),
                net_minor: 10000,
                tax_rate_percent: Some("7.00".to_string()),
                tax_kind: Some("standard".to_string()),
            },
        ],
        tax_breakdown: vec![
            TaxBreakdown {
                tax_rate_percent: "19.00".to_string(),
                net_minor: 190_000,
                tax_minor: 36_100,
                kind: "standard".to_string(),
            },
            TaxBreakdown {
                tax_rate_percent: "7.00".to_string(),
                net_minor: 10_000,
                tax_minor: 700,
                kind: "standard".to_string(),
            },
        ],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 200_000,
            tax_minor: 36_800,
            gross_minor: 236_800,
            currency: "EUR".to_string(),
        },
        due_date: Some("2026-11-07".to_string()),
        note: Some("Payment terms: net 14 days. Thank you for your business!".to_string()),
        intra_eu_note: None,
        reverse_charge_note: None,
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "PO-2026-12345".to_string(),
        seller_contact: Some(ContactInfo {
            name: Some("John Smith".to_string()),
            phone: Some("+49 30 123456".to_string()),
            email: Some("john@haldenworks.de".to_string()),
        }),
        payment_means: Some(PaymentMeans {
            code: "58".to_string(),
            iban: "DE02120300000000202051".to_string(),
            bic: Some("BYLADEM".to_string()),
            account_name: Some("Halden Works GmbH".to_string()),
        }),
        payment_terms_text: Some("Net 14 days".to_string()),
    };

    // Test all three profiles
    let ubl_result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(ubl_result.is_ok());
    let ubl = ubl_result.unwrap();
    assert_eq!(ubl.format, "xrechnung-ubl");
    assert_eq!(ubl.file_name, "RE-2026-0001.xml");
    assert!(!ubl.bytes.is_empty());

    // Validate XML well-formedness
    validate_xml_wellformed(&ubl.bytes);

    // Check that XML declaration is present
    let xml_str = String::from_utf8_lossy(&ubl.bytes);
    assert!(xml_str.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml_str.contains("RE-2026-0001"));
    assert!(xml_str.contains("Halden Works GmbH"));
    assert!(xml_str.contains("Acme GmbH"));

    let cii_result = render_einvoice(&doc, Profile::XRechnungCii);
    assert!(cii_result.is_ok());
    let cii = cii_result.unwrap();
    assert_eq!(cii.format, "xrechnung-cii");
    validate_xml_wellformed(&cii.bytes);

    let factur_x_result = render_einvoice(&doc, Profile::FacturX);
    assert!(factur_x_result.is_ok());
    let factur_x = factur_x_result.unwrap();
    assert_eq!(factur_x.format, "factur-x");
    assert_eq!(factur_x.file_name, "factur-x.xml");
    validate_xml_wellformed(&factur_x.bytes);

    // Write samples if LEDGERS_EINVOICE_OUT is set
    if let Ok(out_dir) = std::env::var("LEDGERS_EINVOICE_OUT") {
        let _ = fs::create_dir_all(&out_dir);
        let _ = fs::write(
            Path::new(&out_dir).join("domestic-invoice-ubl.xml"),
            &ubl.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("domestic-invoice-cii.xml"),
            &cii.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("domestic-invoice-factur-x.xml"),
            &factur_x.bytes,
        );
    }
}

#[test]
fn test_credit_note() {
    // Scenario (b): credit note referencing an invoice
    let base_doc = InvoiceDocument {
        seller_name: "Halden Works GmbH".to_string(),
        seller_address: "Haldenweg 1, Berlin".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Acme GmbH".to_string(),
        buyer_address: "Main Street 1, Berlin".to_string(),
        buyer_vat_id: Some("DE987654321".to_string()),
        invoice_number: "CN-2026-0001".to_string(),
        issue_date: "2026-10-25".to_string(),
        supply_date: "2026-10-25".to_string(),
        lines: vec![DocumentLine {
            sequence: 1,
            description: "Credit for returned materials".to_string(),
            quantity: "1".to_string(),
            unit: None,
            net_minor: 10000,
            tax_rate_percent: Some("19.00".to_string()),
            tax_kind: Some("standard".to_string()),
        }],
        tax_breakdown: vec![TaxBreakdown {
            tax_rate_percent: "19.00".to_string(),
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

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "credit_note".to_string(),
        preceding_invoice_number: Some("RE-2026-0001".to_string()),
        preceding_invoice_date: Some("2026-10-24".to_string()),
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "PO-2026-12345".to_string(),
        seller_contact: Some(ContactInfo {
            name: Some("John Smith".to_string()),
            phone: Some("+49 30 123456".to_string()),
            email: Some("john@haldenworks.de".to_string()),
        }),
        payment_means: Some(PaymentMeans {
            code: "58".to_string(),
            iban: "DE02120300000000202051".to_string(),
            bic: Some("BYLADEM".to_string()),
            account_name: Some("Halden Works GmbH".to_string()),
        }),
        payment_terms_text: None,
    };

    let ubl_result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(ubl_result.is_ok());
    let ubl = ubl_result.unwrap();
    assert_eq!(ubl.file_name, "CN-2026-0001.xml");
    validate_xml_wellformed(&ubl.bytes);

    let cii_result = render_einvoice(&doc, Profile::XRechnungCii);
    assert!(cii_result.is_ok());
    let cii = cii_result.unwrap();
    assert_eq!(cii.format, "xrechnung-cii");
    validate_xml_wellformed(&cii.bytes);

    let factur_x_result = render_einvoice(&doc, Profile::FacturX);
    assert!(factur_x_result.is_ok());
    let factur_x = factur_x_result.unwrap();
    assert_eq!(factur_x.format, "factur-x");
    validate_xml_wellformed(&factur_x.bytes);

    // Write samples if LEDGERS_EINVOICE_OUT is set
    if let Ok(out_dir) = std::env::var("LEDGERS_EINVOICE_OUT") {
        let _ = fs::create_dir_all(&out_dir);
        let _ = fs::write(Path::new(&out_dir).join("credit-note-ubl.xml"), &ubl.bytes);
        let _ = fs::write(Path::new(&out_dir).join("credit-note-cii.xml"), &cii.bytes);
        let _ = fs::write(
            Path::new(&out_dir).join("credit-note-factur-x.xml"),
            &factur_x.bytes,
        );
    }
}

#[test]
fn test_intra_eu_supply() {
    // Scenario (c): intra-EU supply to Fjordlys AB (K category, zero rate)
    let base_doc = InvoiceDocument {
        seller_name: "Halden Works GmbH".to_string(),
        seller_address: "Haldenweg 1, Berlin".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Fjordlys AB".to_string(),
        buyer_address: "Stockholm Vägen 1, Stockholm".to_string(),
        buyer_vat_id: Some("SE556614220001".to_string()),
        invoice_number: "RE-2026-0002".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![DocumentLine {
            sequence: 1,
            description: "Consulting services (intra-EU zero-rated)".to_string(),
            quantity: "50".to_string(),
            unit: Some("h".to_string()),
            net_minor: 75000,
            tax_rate_percent: Some("0.00".to_string()),
            tax_kind: Some("intra-eu".to_string()),
        }],
        tax_breakdown: vec![TaxBreakdown {
            tax_rate_percent: "0.00".to_string(),
            net_minor: 75000,
            tax_minor: 0,
            kind: "intra-eu".to_string(),
        }],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 75000,
            tax_minor: 0,
            gross_minor: 75000,
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

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_fjordlys_buyer(),
        buyer_reference: "ORD-2026-99999".to_string(),
        seller_contact: Some(ContactInfo {
            name: Some("John Smith".to_string()),
            phone: Some("+49 30 123456".to_string()),
            email: Some("john@haldenworks.de".to_string()),
        }),
        payment_means: Some(PaymentMeans {
            code: "58".to_string(),
            iban: "DE02120300000000202051".to_string(),
            bic: Some("BYLADEM".to_string()),
            account_name: Some("Halden Works GmbH".to_string()),
        }),
        payment_terms_text: None,
    };

    let ubl_result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(ubl_result.is_ok());
    let ubl = ubl_result.unwrap();
    validate_xml_wellformed(&ubl.bytes);
    let xml_str = String::from_utf8_lossy(&ubl.bytes);
    assert!(xml_str.contains("Fjordlys AB"));
    assert!(xml_str.contains("SE556614220001"));

    let cii_result = render_einvoice(&doc, Profile::XRechnungCii);
    assert!(cii_result.is_ok());
    let cii = cii_result.unwrap();
    validate_xml_wellformed(&cii.bytes);

    let factur_x_result = render_einvoice(&doc, Profile::FacturX);
    assert!(factur_x_result.is_ok());
    let factur_x = factur_x_result.unwrap();
    validate_xml_wellformed(&factur_x.bytes);

    // Write samples if LEDGERS_EINVOICE_OUT is set
    if let Ok(out_dir) = std::env::var("LEDGERS_EINVOICE_OUT") {
        let _ = fs::create_dir_all(&out_dir);
        let _ = fs::write(
            Path::new(&out_dir).join("intra-eu-supply-ubl.xml"),
            &ubl.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("intra-eu-supply-cii.xml"),
            &cii.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("intra-eu-supply-factur-x.xml"),
            &factur_x.bytes,
        );
    }
}

#[test]
fn test_reverse_charge() {
    // Scenario (d): reverse charge (AE category, zero rate)
    let base_doc = InvoiceDocument {
        seller_name: "Halden Works GmbH".to_string(),
        seller_address: "Haldenweg 1, Berlin".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Acme GmbH".to_string(),
        buyer_address: "Main Street 1, Berlin".to_string(),
        buyer_vat_id: Some("DE987654321".to_string()),
        invoice_number: "RE-2026-0003".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![DocumentLine {
            sequence: 1,
            description: "Import services (reverse charge)".to_string(),
            quantity: "1".to_string(),
            unit: None,
            net_minor: 50000,
            tax_rate_percent: Some("0.00".to_string()),
            tax_kind: Some("reverse-charge".to_string()),
        }],
        tax_breakdown: vec![TaxBreakdown {
            tax_rate_percent: "0.00".to_string(),
            net_minor: 50000,
            tax_minor: 0,
            kind: "reverse-charge".to_string(),
        }],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 50000,
            tax_minor: 0,
            gross_minor: 50000,
            currency: "EUR".to_string(),
        },
        due_date: None,
        note: None,
        intra_eu_note: None,
        reverse_charge_note: Some("Steuerschuldnerschaft des Leistungsempfängers".to_string()),
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "PO-2026-12345".to_string(),
        seller_contact: Some(ContactInfo {
            name: Some("John Smith".to_string()),
            phone: Some("+49 30 123456".to_string()),
            email: Some("john@haldenworks.de".to_string()),
        }),
        payment_means: Some(PaymentMeans {
            code: "58".to_string(),
            iban: "DE02120300000000202051".to_string(),
            bic: Some("BYLADEM".to_string()),
            account_name: Some("Halden Works GmbH".to_string()),
        }),
        payment_terms_text: None,
    };

    let ubl_result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(ubl_result.is_ok());
    let ubl = ubl_result.unwrap();
    validate_xml_wellformed(&ubl.bytes);

    let cii_result = render_einvoice(&doc, Profile::XRechnungCii);
    assert!(cii_result.is_ok());
    let cii = cii_result.unwrap();
    validate_xml_wellformed(&cii.bytes);

    let factur_x_result = render_einvoice(&doc, Profile::FacturX);
    assert!(factur_x_result.is_ok());
    let factur_x = factur_x_result.unwrap();
    validate_xml_wellformed(&factur_x.bytes);

    // Write samples if LEDGERS_EINVOICE_OUT is set
    if let Ok(out_dir) = std::env::var("LEDGERS_EINVOICE_OUT") {
        let _ = fs::create_dir_all(&out_dir);
        let _ = fs::write(
            Path::new(&out_dir).join("reverse-charge-ubl.xml"),
            &ubl.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("reverse-charge-cii.xml"),
            &cii.bytes,
        );
        let _ = fs::write(
            Path::new(&out_dir).join("reverse-charge-factur-x.xml"),
            &factur_x.bytes,
        );
    }
}

#[test]
fn test_missing_buyer_reference() {
    // Error case: missing buyer reference
    let base_doc = InvoiceDocument {
        seller_name: "Seller".to_string(),
        seller_address: "Address".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Buyer".to_string(),
        buyer_address: "Address".to_string(),
        buyer_vat_id: None,
        invoice_number: "RE-2026-0001".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![],
        tax_breakdown: vec![],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 0,
            tax_minor: 0,
            gross_minor: 0,
            currency: "EUR".to_string(),
        },
        due_date: None,
        note: None,
        intra_eu_note: None,
        reverse_charge_note: None,
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: String::new(), // Missing!
        seller_contact: None,
        payment_means: None,
        payment_terms_text: None,
    };

    let result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(result.is_err());
    match result {
        Err(err) => {
            let err_str = format!("{err}");
            assert!(err_str.contains("buyer_reference"));
        }
        _ => panic!("Expected error"),
    }
}

#[test]
fn test_unknown_unit() {
    // Error case: unknown unit code
    let base_doc = InvoiceDocument {
        seller_name: "Seller".to_string(),
        seller_address: "Address".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Buyer".to_string(),
        buyer_address: "Address".to_string(),
        buyer_vat_id: None,
        invoice_number: "RE-2026-0001".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![DocumentLine {
            sequence: 1,
            description: "Test".to_string(),
            quantity: "1".to_string(),
            unit: Some("INVALID123".to_string()), // Invalid unit
            net_minor: 1000,
            tax_rate_percent: Some("19.00".to_string()),
            tax_kind: Some("standard".to_string()),
        }],
        tax_breakdown: vec![],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 1000,
            tax_minor: 190,
            gross_minor: 1190,
            currency: "EUR".to_string(),
        },
        due_date: None,
        note: None,
        intra_eu_note: None,
        reverse_charge_note: None,
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "REF123".to_string(),
        seller_contact: None,
        payment_means: None,
        payment_terms_text: None,
    };

    let result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(result.is_err());
    match result {
        Err(err) => {
            let err_str = format!("{err}");
            assert!(err_str.contains("unknown unit") || err_str.contains("INVALID"));
        }
        _ => panic!("Expected error"),
    }
}

#[test]
fn test_unsupported_tax_kind() {
    // Error case: unsupported tax kind
    let base_doc = InvoiceDocument {
        seller_name: "Seller".to_string(),
        seller_address: "Address".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Buyer".to_string(),
        buyer_address: "Address".to_string(),
        buyer_vat_id: None,
        invoice_number: "RE-2026-0001".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![DocumentLine {
            sequence: 1,
            description: "Test".to_string(),
            quantity: "1".to_string(),
            unit: None,
            net_minor: 1000,
            tax_rate_percent: Some("19.00".to_string()),
            tax_kind: Some("unknown-kind".to_string()), // Invalid kind
        }],
        tax_breakdown: vec![],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 1000,
            tax_minor: 190,
            gross_minor: 1190,
            currency: "EUR".to_string(),
        },
        due_date: None,
        note: None,
        intra_eu_note: None,
        reverse_charge_note: None,
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "invoice".to_string(),
        preceding_invoice_number: None,
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "REF123".to_string(),
        seller_contact: None,
        payment_means: None,
        payment_terms_text: None,
    };

    let result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(result.is_err());
    match result {
        Err(err) => {
            let err_str = format!("{err}");
            assert!(err_str.contains("unsupported tax kind") || err_str.contains("unknown-kind"));
        }
        _ => panic!("Expected error"),
    }
}

#[test]
fn test_credit_note_without_preceding_invoice() {
    // Error case: credit note without preceding invoice number
    let base_doc = InvoiceDocument {
        seller_name: "Seller".to_string(),
        seller_address: "Address".to_string(),
        seller_tax_number: None,
        seller_vat_id: Some("DE123456789".to_string()),
        buyer_name: "Buyer".to_string(),
        buyer_address: "Address".to_string(),
        buyer_vat_id: None,
        invoice_number: "CN-2026-0001".to_string(),
        issue_date: "2026-10-24".to_string(),
        supply_date: "2026-10-24".to_string(),
        lines: vec![],
        tax_breakdown: vec![],
        currency: "EUR".to_string(),
        totals: InvoiceTotals {
            net_minor: 0,
            tax_minor: 0,
            gross_minor: 0,
            currency: "EUR".to_string(),
        },
        due_date: None,
        note: None,
        intra_eu_note: None,
        reverse_charge_note: None,
    };

    let doc = EInvoiceDocument {
        base: base_doc,
        document_type: "credit_note".to_string(),
        preceding_invoice_number: None, // Missing!
        preceding_invoice_date: None,
        seller: create_seller(),
        buyer: create_acme_buyer(),
        buyer_reference: "REF123".to_string(),
        seller_contact: None,
        payment_means: None,
        payment_terms_text: None,
    };

    let result = render_einvoice(&doc, Profile::XRechnungUbl);
    assert!(result.is_err());
    match result {
        Err(err) => {
            let err_str = format!("{err}");
            assert!(err_str.contains("preceding_invoice_number"));
        }
        _ => panic!("Expected error"),
    }
}
