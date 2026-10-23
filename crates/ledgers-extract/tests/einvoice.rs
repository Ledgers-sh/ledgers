//! E-invoice parsing integration tests: UBL, CII, `ZUGFeRD`, security, and validation.

use ledgers_documents::Location;
use ledgers_extract::einvoice::{self, EInvoiceError, Format};
use ledgers_extract::schema::{ExtractedInvoice, ExtractedLine};
use std::collections::BTreeMap;
use std::fs;
use time::{Date, Month};

fn xrechnung_ubl() -> Vec<u8> {
    include_bytes!("fixtures/einvoice/xrechnung-ubl.xml").to_vec()
}

fn xrechnung_cii() -> Vec<u8> {
    include_bytes!("fixtures/einvoice/xrechnung-cii.xml").to_vec()
}

fn zugferd_pdf() -> Vec<u8> {
    include_bytes!("fixtures/einvoice/zugferd-extended.pdf").to_vec()
}

fn kosit_expected() -> ExtractedInvoice {
    ExtractedInvoice {
        vendor_name: Some("[Seller name]".to_owned()),
        vendor_vat_id: Some("DE123456789".to_owned()),
        vendor_iban: Some("DE79000000001234567890".to_owned()),
        invoice_number: Some("Rechnungsnummer".to_owned()),
        invoice_date: Some("2016-03-02".to_owned()),
        due_date: Some("2016-03-16".to_owned()),
        currency: Some("EUR".to_owned()),
        net_minor: Some(25661),
        vat_minor: Some(4876),
        gross_minor: Some(30537),
        lines: vec![
            ExtractedLine {
                description: Some("Keyboard/Maus - weiß".to_owned()),
                net_minor: Some(1252),
                vat_rate_bp: Some(1900),
                vat_minor: Some(238),
                gross_minor: Some(1490),
            },
            ExtractedLine {
                description: Some("MX Master".to_owned()),
                net_minor: Some(12690),
                vat_rate_bp: Some(1900),
                vat_minor: Some(2411),
                gross_minor: Some(15101),
            },
            ExtractedLine {
                description: Some("Tastatur Deutsch".to_owned()),
                net_minor: Some(9564),
                vat_rate_bp: Some(1900),
                vat_minor: Some(1817),
                gross_minor: Some(11381),
            },
            ExtractedLine {
                description: Some("Beschaffungspauschale".to_owned()),
                net_minor: Some(1175),
                vat_rate_bp: Some(1900),
                vat_minor: Some(223),
                gross_minor: Some(1398),
            },
            ExtractedLine {
                description: Some("Lieferkosten".to_owned()),
                net_minor: Some(980),
                vat_rate_bp: Some(1900),
                vat_minor: Some(186),
                gross_minor: Some(1166),
            },
        ],
        confidence: BTreeMap::default(),
    }
}

fn zugferd_expected() -> ExtractedInvoice {
    ExtractedInvoice {
        vendor_name: Some("Rohstoff AG Salzgitter".to_owned()),
        vendor_vat_id: Some("DE123456788".to_owned()),
        vendor_iban: Some("DE77370700600321987000".to_owned()),
        invoice_number: Some("47110815".to_owned()),
        invoice_date: Some("2025-12-01".to_owned()),
        due_date: Some("2025-12-31".to_owned()),
        currency: Some("GBP".to_owned()),
        net_minor: Some(85_875),
        vat_minor: Some(16_316),
        gross_minor: Some(102_191),
        lines: vec![ExtractedLine {
            description: Some("Stahlcoil".to_owned()),
            net_minor: Some(85_000),
            vat_rate_bp: Some(1_900),
            vat_minor: Some(16_150),
            gross_minor: Some(101_150),
        }],
        confidence: BTreeMap::default(),
    }
}

#[test]
fn official_samples_parse_with_correct_values() {
    let expected = kosit_expected();
    for (bytes, format) in [
        (xrechnung_ubl(), Format::Ubl),
        (xrechnung_cii(), Format::Cii),
    ] {
        let parsed = einvoice::parse(&bytes).expect("should parse");
        assert_eq!(parsed.format, format);
        assert!(!parsed.embedded_in_pdf);
        assert_eq!(
            (
                parsed.invoice.vendor_name.as_ref(),
                parsed.invoice.vendor_vat_id.as_ref()
            ),
            (
                expected.vendor_name.as_ref(),
                expected.vendor_vat_id.as_ref()
            )
        );
        assert_eq!(parsed.invoice.net_minor, expected.net_minor);
        assert_eq!(parsed.invoice.vat_minor, expected.vat_minor);
        assert_eq!(parsed.invoice.gross_minor, expected.gross_minor);
        assert_eq!(parsed.invoice.lines.len(), expected.lines.len());
    }

    let pdf = einvoice::parse(&zugferd_pdf()).expect("ZUGFeRD should parse");
    assert_eq!(pdf.format, Format::Cii);
    assert!(pdf.embedded_in_pdf);
}

#[test]
fn lines_have_correct_amounts() {
    for bytes in [xrechnung_ubl(), xrechnung_cii()] {
        let invoice = &einvoice::parse(&bytes).expect("parse").invoice;
        assert_eq!(invoice.lines.len(), 5);
        let expected = kosit_expected();
        for (i, expected_line) in expected.lines.iter().enumerate() {
            assert_eq!(invoice.lines[i], *expected_line);
        }
    }
}

#[test]
fn anchors_are_valid_and_correct() {
    let fixtures = [
        ("UBL", xrechnung_ubl(), Format::Ubl),
        ("CII", xrechnung_cii(), Format::Cii),
        ("ZUGFeRD", zugferd_pdf(), Format::Cii),
    ];

    for (name, bytes, expected_format) in &fixtures {
        let einv = einvoice::parse(bytes).unwrap_or_else(|_| panic!("{name} should parse"));
        assert_eq!(einv.format, *expected_format);

        for anchor in &einv.anchors {
            anchor
                .validate()
                .unwrap_or_else(|_| panic!("{name}: anchor {} validation failed", anchor.field));
            assert!(matches!(anchor.location, Location::Xml { .. }));
        }

        let has_invoice_id = einv.anchors.iter().any(|a| {
            a.field == "invoice_number"
                && matches!(&a.location, Location::Xml { path } if path.contains("/ID"))
        });
        assert!(has_invoice_id);

        for anchor in &einv.anchors {
            if anchor.field.starts_with("lines.") {
                assert!(
                    !anchor.field.contains("vat_minor") && !anchor.field.contains("gross_minor")
                );
            }
        }
    }
}

#[test]
fn checks_flags() {
    for bytes in [xrechnung_ubl(), xrechnung_cii()] {
        let einv = einvoice::parse(&bytes).expect("parse");
        let today = Date::from_calendar_date(2026, Month::October, 23).unwrap();
        let report = ledgers_extract::checks::run(&einv.invoice, today, None);
        let failed: Vec<&str> = report
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.id)
            .collect();
        assert_eq!(failed, vec!["lines-vat-sum"]);
    }
}

#[test]
fn detection() {
    assert_eq!(einvoice::detect(&xrechnung_ubl()), Some(Format::Ubl));
    assert_eq!(einvoice::detect(&xrechnung_cii()), Some(Format::Cii));
    assert_eq!(einvoice::detect(&zugferd_pdf()), Some(Format::Cii));

    assert_eq!(
        einvoice::detect(b"<?xml version=\"1.0\"?><Document xmlns=\"urn:iso:std:iso:20022:tech:xsd:camt.053.001.02\"></Document>"),
        None
    );
    assert_eq!(
        einvoice::detect(
            b"<?xml version=\"1.0\"?><Invoice xmlns=\"urn:wrong:namespace\"></Invoice>"
        ),
        None
    );
}

#[test]
fn hostile_corpus_all_rejected() {
    let mut hostile_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    hostile_dir.push("tests/fixtures/einvoice/hostile");
    let entries = fs::read_dir(&hostile_dir).expect("read hostile dir");

    let mut count = 0;
    let mut file_results = vec![];
    for entry in entries {
        let path = entry.expect("read entry").path();
        if path.is_file() && path.extension().is_some_and(|e| e == "xml") {
            count += 1;
            let payload = fs::read(&path).unwrap_or_default();
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned();
            let _ = einvoice::detect(&payload);
            let parse_result = einvoice::parse(&payload);
            assert!(parse_result.is_err());
            file_results.push((file_name, parse_result.err()));
        }
    }
    assert!(count > 0);

    let expected = [
        ("billion_laughs.xml", "DOCTYPE not allowed"),
        ("deep_nesting.xml", "nesting too deep"),
        ("mismatched_tags.xml", "malformed"),
        ("quadratic_blowup.xml", "DOCTYPE not allowed"),
        ("truncated.xml", "unclosed"),
        ("two_roots.xml", "multiple root"),
        ("unknown_entity.xml", "unknown entity"),
        ("xxe_file.xml", "DOCTYPE not allowed"),
        ("xxe_http.xml", "DOCTYPE not allowed"),
        ("xxe_parameter_entity.xml", "DOCTYPE not allowed"),
    ];

    for (file_name, expected_error) in &expected {
        if let Some((_, Some(err))) = file_results.iter().find(|(n, _)| n.as_str() == *file_name) {
            assert!(err.to_string().contains(expected_error));
        }
    }
}

#[test]
fn zugferd_pdf_comprehensive() {
    let pdf = einvoice::parse(&zugferd_pdf()).expect("ZUGFeRD should parse");
    let expected = zugferd_expected();
    assert_eq!(pdf.format, Format::Cii);
    assert!(pdf.embedded_in_pdf);
    assert_eq!(pdf.invoice.vendor_name, expected.vendor_name);
    assert_eq!(pdf.invoice.currency, expected.currency);
    assert_eq!(pdf.invoice.net_minor, expected.net_minor);
    assert_eq!(pdf.invoice.vat_minor, expected.vat_minor);
    assert_eq!(pdf.invoice.gross_minor, expected.gross_minor);
    assert_eq!(pdf.invoice.lines.len(), 1);
    let line = &pdf.invoice.lines[0];
    assert_eq!(
        line.description.as_deref(),
        expected.lines[0].description.as_deref()
    );
    assert_eq!(line.net_minor, expected.lines[0].net_minor);
    assert_eq!(line.vat_rate_bp, expected.lines[0].vat_rate_bp);
}

#[test]
fn oversize_xml_rejected() {
    let mut large_xml = Vec::new();
    large_xml.extend_from_slice(
        b"<?xml version=\"1.0\"?><Invoice xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Invoice-2\">",
    );
    large_xml.resize(8 * 1024 * 1024 + 1, b' ');
    large_xml.extend_from_slice(b"</Invoice>");

    assert!(
        matches!(einvoice::parse(&large_xml), Err(EInvoiceError::TooLarge)),
        "oversized input should be rejected with TooLarge"
    );
}

#[test]
fn too_many_lines_rejected() {
    let mut xml = Vec::new();
    xml.extend_from_slice(
        b"<?xml version=\"1.0\"?><Invoice xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Invoice-2\" \
          xmlns:cac=\"urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2\" \
          xmlns:cbc=\"urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2\">\
          <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>",
    );
    for i in 0..10_001 {
        xml.extend_from_slice(
            format!(
                "<cac:InvoiceLine><cbc:ID>{i}</cbc:ID><cbc:LineExtensionAmount>1</cbc:LineExtensionAmount></cac:InvoiceLine>"
            )
            .as_bytes(),
        );
    }
    xml.extend_from_slice(b"</Invoice>");

    assert!(
        matches!(einvoice::parse(&xml), Err(EInvoiceError::TooManyLines)),
        "too many lines should be rejected with TooManyLines"
    );
}

#[test]
fn flate_bomb_in_pdf_rejected() {
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
    let zeros = vec![0u8; 64 * 1024 * 1024];
    encoder.write_all(&zeros).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut pdf = vec![0u8; 0];
    pdf.extend_from_slice(b"%PDF-1.4\n");
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    );
    pdf.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /EmbeddedFile /Length {} /Filter /FlateDecode >>\nstream\n",
            compressed.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f\n");
    pdf.extend_from_slice(b"xref\ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    pdf.extend_from_slice(b"100\n%%EOF");
    assert!(
        matches!(einvoice::parse(&pdf), Err(EInvoiceError::TooLarge)),
        "flate bomb should be rejected with TooLarge"
    );
}

#[test]
fn truncated_and_flipped_bytes_never_panic() {
    let fixtures = [xrechnung_ubl(), xrechnung_cii(), zugferd_pdf()];
    for payload in fixtures {
        for i in 0..payload.len().min(100) {
            let mut truncated = payload.clone();
            truncated.truncate(i);
            let _ = einvoice::parse(&truncated);
            let mut flipped = payload.clone();
            if !flipped.is_empty() {
                let idx = i % flipped.len();
                flipped[idx] ^= 0xFF;
            }
            let _ = einvoice::parse(&flipped);
        }
    }
}
