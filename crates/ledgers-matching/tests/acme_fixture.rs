use ledgers_matching::csv::*;
use ledgers_matching::*;
use ledgers_money::{Amount, Currency};

/// Helper: build PO CSV for bracket (200 @ 12.50) + plate (40 @ 25.00).
fn build_po_csv(po_num: &str) -> String {
    format!(
        "po_number,vendor,currency,line,item,description,quantity,unit_price\n{po_num},Acme GmbH,EUR,1,SB-12,Steel bracket,200,12.50\n{po_num},Acme GmbH,EUR,2,MP-40,Mounting plate,40,25.00"
    )
}

/// Helper: build receipt CSV matching both PO lines.
fn build_receipt_csv(gr_num: &str, po_num: &str) -> String {
    format!(
        "receipt_number,vendor,po_number,po_line,quantity\n{gr_num},Acme GmbH,{po_num},1,200\n{gr_num},Acme GmbH,{po_num},2,40"
    )
}

/// Helper: build bill with two lines matching PO-1182 items.
fn build_acme_bill(
    number: &str,
    unit_price_1: Amount,
    unit_price_2: Amount,
    po_ref: Option<&str>,
) -> MatchBill {
    let eur = Currency::from_code("EUR").unwrap();
    MatchBill {
        number: number.to_string(),
        vendor: "Acme GmbH".to_string(),
        currency: eur,
        po_reference: po_ref.map(str::to_string),
        lines: vec![
            MatchBillLine {
                item: Some("SB-12".to_string()),
                description: "Steel bracket".to_string(),
                quantity: Quantity::parse("200").unwrap(),
                unit_price: unit_price_1,
            },
            MatchBillLine {
                item: Some("MP-40".to_string()),
                description: "Mounting plate".to_string(),
                quantity: Quantity::parse("40").unwrap(),
                unit_price: unit_price_2,
            },
        ],
    }
}

/// Helper: build single-item bill.
#[allow(clippy::too_many_arguments)]
fn build_bill(
    number: &str,
    vendor: &str,
    item: &str,
    desc: &str,
    qty: &str,
    unit_price: Amount,
    po_ref: Option<&str>,
    eur: Currency,
) -> MatchBill {
    MatchBill {
        number: number.to_string(),
        vendor: vendor.to_string(),
        currency: eur,
        po_reference: po_ref.map(str::to_string),
        lines: vec![MatchBillLine {
            item: Some(item.to_string()),
            description: desc.to_string(),
            quantity: Quantity::parse(qty).unwrap(),
            unit_price,
        }],
    }
}

#[test]
fn test_1_fixture_acme_3way_perfect() {
    let eur = Currency::from_code("EUR").unwrap();
    let po_csv = build_po_csv("PO-1182");
    let gr_csv = build_receipt_csv("GR-0931", "PO-1182");
    let pos = import_purchase_orders(&po_csv).unwrap();
    let receipts = import_goods_receipts(&gr_csv).unwrap();
    let bill = build_acme_bill(
        "RE-2026-0418",
        Amount::new(1250, eur),
        Amount::from_major(25, eur).unwrap(),
        Some("PO 1182"),
    );
    let proposals = propose(&bill, &pos, &receipts, &Tolerances::new(Tolerance::ZERO)).unwrap();

    assert_eq!(proposals.len(), 1);
    let p = &proposals[0];
    assert_eq!(p.kind, matching::MatchKind::ThreeWay);
    assert_eq!(p.bill, "RE-2026-0418");
    assert_eq!(p.po, "PO-1182");
    assert_eq!(p.receipts, vec!["GR-0931"]);
    assert_eq!(p.confidence, 100);
    assert!(p.auto_accept);
    assert!(p.explanation.blockers.is_empty());
    assert!(p.explanation.differs.is_empty());

    assert_eq!(
        p.explanation.matched,
        vec![
            "vendor Acme GmbH".to_string(),
            "currency EUR".to_string(),
            "line 1 Steel bracket: quantity 200 within 200 received".to_string(),
            "line 1 Steel bracket: price 12.50 EUR/unit as ordered".to_string(),
            "line 2 Mounting plate: quantity 40 within 40 received".to_string(),
            "line 2 Mounting plate: price 25.00 EUR/unit as ordered".to_string(),
            "PO reference PO-1182".to_string(),
        ]
    );
}

#[test]
fn test_2_price_over_tolerance() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1182,Acme GmbH,EUR,1,SB-12,Steel bracket,200,12.50").unwrap();
    let bill = build_bill(
        "RE-2026-0418",
        "Acme GmbH",
        "SB-12",
        "Steel bracket",
        "200",
        Amount::from_major(13, eur).unwrap(),
        Some("PO 1182"),
        eur,
    );
    let p = &propose(
        &bill,
        &pos,
        &[],
        &Tolerances::new(Tolerance {
            price_abs_minor: 1000,
            ..Tolerance::ZERO
        }),
    )
    .unwrap()[0];
    assert!(!p.auto_accept);
    assert_eq!(
        p.explanation.blockers,
        vec!["line 1 Steel bracket: billed 13.00 EUR/unit vs PO 12.50 EUR/unit, +0.50 per unit, +100.00 EUR over 200 units, tolerance 10.00 EUR".to_string()]
    );
}

#[test]
fn test_3_price_within_vendor_tolerance() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1182,Acme GmbH,EUR,1,SB-12,Steel bracket,200,12.50").unwrap();
    let bill = build_bill(
        "RE-2026-0418",
        "Acme GmbH",
        "SB-12",
        "Steel bracket",
        "200",
        Amount::new(1255, eur),
        Some("PO 1182"),
        eur,
    );
    let p = &propose(
        &bill,
        &pos,
        &[],
        &Tolerances::new(Tolerance {
            price_abs_minor: 1000,
            price_pct_bp: 100,
            ..Tolerance::ZERO
        }),
    )
    .unwrap()[0];
    assert!(p.auto_accept);
    assert_eq!(p.confidence, 100);
    assert!(p.explanation.blockers.is_empty());
    assert_eq!(
        p.explanation.differs,
        vec!["line 1 Steel bracket: billed 12.55 EUR/unit vs PO 12.50 EUR/unit, +0.05 per unit, +10.00 EUR over 200 units, within tolerance 10.00 EUR".to_string()]
    );
}

#[test]
fn test_3a_price_percent_stricter() {
    // Percent limit (10 bp = 0.1%) is stricter than absolute limit (10.00 EUR)
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1182,Acme GmbH,EUR,1,SB-12,Steel bracket,200,12.50").unwrap();
    let bill = build_bill(
        "RE-2026-0418",
        "Acme GmbH",
        "SB-12",
        "Steel bracket",
        "200",
        Amount::new(1252, eur),
        Some("PO 1182"),
        eur,
    );
    // ordered = 12.50 * 200 = 2500.00 EUR = 250000 minor
    // allowed = min(1000 minor, 250000 * 10 / 10000) = min(10.00 EUR, 0.25 EUR) = 0.25 EUR = 2.50 EUR... wait
    // Actually, price_pct_bp is 10 basis points, so 250000 * 10 / 10000 = 250 minor = 2.50 EUR
    // So allowed = min(1000, 250) = 250 minor = 2.50 EUR
    // bill line: 12.52 * 200 = 2504.00 EUR, so total_diff = 4.00 EUR = 400 minor
    // 400 > 250, so it's a blocker
    let p = &propose(
        &bill,
        &pos,
        &[],
        &Tolerances::new(Tolerance {
            price_abs_minor: 1000,
            price_pct_bp: 10,
            ..Tolerance::ZERO
        }),
    )
    .unwrap()[0];
    assert!(!p.auto_accept);
    assert_eq!(
        p.explanation.blockers,
        vec!["line 1 Steel bracket: billed 12.52 EUR/unit vs PO 12.50 EUR/unit, +0.02 per unit, +4.00 EUR over 200 units, tolerance 2.50 EUR".to_string()]
    );
}

#[test]
fn test_3b_price_cheaper() {
    // Cheaper price is always accepted (price-OK).
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1182,Acme GmbH,EUR,1,SB-12,Steel bracket,200,12.50").unwrap();
    let bill = build_bill(
        "RE-2026-0418",
        "Acme GmbH",
        "SB-12",
        "Steel bracket",
        "200",
        Amount::new(1240, eur),
        Some("PO 1182"),
        eur,
    );
    let p = &propose(&bill, &pos, &[], &Tolerances::new(Tolerance::ZERO)).unwrap()[0];
    assert!(p.auto_accept);
    assert_eq!(p.confidence, 100);
    assert!(p.explanation.blockers.is_empty());
    assert_eq!(
        p.explanation.differs,
        vec!["line 1 Steel bracket: billed 12.40 EUR/unit vs PO 12.50 EUR/unit, -0.10 per unit, -20.00 EUR over 200 units".to_string()]
    );
}

#[test]
fn test_4_partial_many_receipts() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1,Acme GmbH,EUR,1,ITEM,Widget,100,10.00").unwrap();
    let receipts = import_goods_receipts("receipt_number,vendor,po_number,po_line,quantity\nGR-B,Acme GmbH,PO-1,1,40\nGR-A,Acme GmbH,PO-1,1,60").unwrap();
    let bill_100 = build_bill(
        "INV-100",
        "Acme GmbH",
        "ITEM",
        "Widget",
        "100",
        Amount::from_major(10, eur).unwrap(),
        None,
        eur,
    );
    let p = &propose(
        &bill_100,
        &pos,
        &receipts,
        &Tolerances::new(Tolerance::ZERO),
    )
    .unwrap()[0];
    assert_eq!(p.kind, matching::MatchKind::ThreeWay);
    assert_eq!(p.receipts, vec!["GR-A", "GR-B"]);
    assert!(p.explanation.blockers.is_empty());

    // Bill 100 for only 40 received (GR-B only) should have blockers
    let p_partial = &propose(
        &bill_100,
        &pos,
        &receipts[0..1],
        &Tolerances::new(Tolerance::ZERO),
    )
    .unwrap()[0];
    assert_eq!(
        p_partial.explanation.blockers,
        vec!["line 1 Widget: billed 100 exceeds 60 received by 40, tolerance 0".to_string()]
    );

    // Bill 60 for 100 received (partial delivery) should have no blockers
    let bill_60 = build_bill(
        "INV-60",
        "Acme GmbH",
        "ITEM",
        "Widget",
        "60",
        Amount::from_major(10, eur).unwrap(),
        None,
        eur,
    );
    let p_60 = &propose(&bill_60, &pos, &receipts, &Tolerances::new(Tolerance::ZERO)).unwrap()[0];
    assert!(p_60.explanation.blockers.is_empty());
    assert_eq!(
        p_60.explanation.differs,
        vec!["line 1 Widget: quantity 60 within 100 received".to_string()]
    );
}

#[test]
fn test_5_2way_when_no_receipts() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1,Acme GmbH,EUR,1,ITEM,Widget,100,10.00").unwrap();
    let bill = build_bill(
        "INV-1",
        "Acme GmbH",
        "ITEM",
        "Widget",
        "50",
        Amount::from_major(10, eur).unwrap(),
        None,
        eur,
    );
    let p = &propose(&bill, &pos, &[], &Tolerances::new(Tolerance::ZERO)).unwrap()[0];
    assert_eq!(p.kind, matching::MatchKind::TwoWay);
    assert!(p.receipts.is_empty());
}

#[test]
fn test_6_unmatched_line_blocks() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1,Acme GmbH,EUR,1,ITEM1,Widget,100,10.00").unwrap();
    let mut bill = build_bill(
        "INV-1",
        "Acme GmbH",
        "ITEM1",
        "Widget",
        "100",
        Amount::from_major(10, eur).unwrap(),
        None,
        eur,
    );
    bill.lines.push(MatchBillLine {
        item: Some("UNKNOWN".to_string()),
        description: "Unknown item".to_string(),
        quantity: Quantity::parse("50").unwrap(),
        unit_price: Amount::from_major(5, eur).unwrap(),
    });
    let p = &propose(&bill, &pos, &[], &Tolerances::new(Tolerance::ZERO)).unwrap()[0];
    assert!(!p.auto_accept);
    assert_eq!(
        p.explanation.blockers,
        vec!["bill line 2 Unknown item: no PO line with this item or description".to_string()]
    );
}

#[test]
fn test_7_no_reference_max_80() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1,Acme GmbH,EUR,1,ITEM,Widget,100,10.00").unwrap();
    let bill = build_bill(
        "INV-1",
        "Acme GmbH",
        "ITEM",
        "Widget",
        "100",
        Amount::from_major(10, eur).unwrap(),
        None,
        eur,
    );
    let p = &propose(&bill, &pos, &[], &Tolerances::new(Tolerance::ZERO)).unwrap()[0];
    assert_eq!(p.confidence, 80);
    assert!(!p.auto_accept);
}

#[test]
fn test_8_determinism_and_sorting() {
    let eur = Currency::from_code("EUR").unwrap();
    let pos = import_purchase_orders("po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-2,Acme GmbH,EUR,1,ITEM,Widget,100,10.00\nPO-1,Acme GmbH,EUR,1,ITEM,Widget,100,10.00").unwrap();
    let bill = build_bill(
        "INV-1",
        "Acme GmbH",
        "ITEM",
        "Widget",
        "100",
        Amount::from_major(10, eur).unwrap(),
        Some("PO-1"),
        eur,
    );
    let tolerances = Tolerances::new(Tolerance::ZERO);
    let proposals1 = propose(&bill, &pos, &[], &tolerances).unwrap();
    let mut pos_rev = pos.clone();
    pos_rev.reverse();
    let proposals2 = propose(&bill, &pos_rev, &[], &tolerances).unwrap();
    assert_eq!(proposals1.len(), proposals2.len());
    for (p1, p2) in proposals1.iter().zip(proposals2.iter()) {
        assert_eq!(p1.po, p2.po);
        assert_eq!(p1.confidence, p2.confidence);
    }
}

#[test]
fn test_quantity_parse_display() {
    assert_eq!(Quantity::parse("100").unwrap().to_string(), "100");
    assert_eq!(Quantity::parse("2.5").unwrap().to_string(), "2.5");
    assert_eq!(Quantity::parse("0.125").unwrap().to_string(), "0.125");
}

#[test]
fn test_csv_quoted_field() {
    let csv = r#"po_number,vendor,currency,line,item,description,quantity,unit_price
PO-001,"Vendor, Inc.",EUR,1,ITEM001,"Item with, comma",100,10.00"#;

    let pos = import_purchase_orders(csv).unwrap();
    assert_eq!(pos[0].vendor, "Vendor, Inc.");
    assert_eq!(pos[0].lines[0].description, "Item with, comma");
}

#[test]
fn test_csv_row_error() {
    let csv =
        "po_number,vendor,currency,line,item,description,quantity,unit_price\nPO-1,bad_vendor";

    let result = import_purchase_orders(csv);
    assert!(result.is_err());
    if let Err(MatchError::Csv { row, message: _ }) = result {
        assert_eq!(row, 2); // 1-based row numbering
    } else {
        panic!("Expected Csv error");
    }
}

#[test]
fn test_normalize_reference() {
    use ledgers_matching::matching::{normalize_reference, normalize_vendor};

    assert_eq!(normalize_reference("PO-1182"), "PO1182");
    assert_eq!(normalize_reference("PO 1182"), "PO1182");
    assert_eq!(normalize_reference("po 1182"), "PO1182");

    assert_eq!(normalize_vendor("Acme GmbH"), "acme gmbh");
    assert_eq!(normalize_vendor("  ACME   GmbH  "), "acme gmbh");
}

#[test]
fn test_9_vendor_mismatch_receipt_ignored() {
    // Receipt from "Other GmbH" with PO-1182 should not match PO from "Acme GmbH".
    // The bill should remain 2-way (no receipts), not become 3-way.
    let eur = Currency::from_code("EUR").unwrap();
    let po_csv = build_po_csv("PO-1182");
    let receipts = import_goods_receipts(
        "receipt_number,vendor,po_number,po_line,quantity\nGR-0931,Other GmbH,PO-1182,1,200\nGR-0931,Other GmbH,PO-1182,2,40",
    )
    .unwrap();
    let pos = import_purchase_orders(&po_csv).unwrap();
    let bill = build_acme_bill(
        "RE-2026-0418",
        Amount::new(1250, eur),
        Amount::from_major(25, eur).unwrap(),
        Some("PO 1182"),
    );
    let proposals = propose(&bill, &pos, &receipts, &Tolerances::new(Tolerance::ZERO)).unwrap();

    assert_eq!(proposals.len(), 1);
    let p = &proposals[0];
    // Receipt from different vendor should be ignored, so match is 2-way, not 3-way.
    assert_eq!(p.kind, matching::MatchKind::TwoWay);
    // Receipts list should be empty because vendor mismatch.
    assert!(p.receipts.is_empty());
    // Quantities should be checked against ordered (2-way), not received.
    assert!(
        p.explanation
            .matched
            .iter()
            .any(|e| e.contains("open on order"))
    );
}
