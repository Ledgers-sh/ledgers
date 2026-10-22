//! Integration tests for document deletion, anchors, and linked-document guards (issue #20).

mod common;

use common::http::{APP_ORIGIN, Auth, Reply, call};
use common::{Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::Blob;
use cratefield_core::axum::http::{Method, StatusCode};
use ledgers_documents::anchor::{Anchor, BoundingBox, Location};
use serde_json::json;

fn fx() -> Fixture {
    common::fixture()
}

fn person_fx() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

/// A valid invented one-page PDF with a text layer.
fn invoice_pdf() -> Vec<u8> {
    ledgers_extract::fixture::pdf_with_text(&[
        "Halden Supplies GmbH",
        "VAT ID: DE123456789",
        "Invoice No: RE-2026-0042",
        "Invoice date: 15.10.2026",
        "Due date: 2026-11-14",
        "IBAN DE89 3704 0044 0532 0130 00",
        "Subtotal 1.000,00 EUR",
        "VAT 19%: 190,00 EUR",
        "Total 1.190,00 EUR",
    ])
}

/// Uploads `body` with `key` and the declared `content_type`, returning the document id.
fn upload_document(fx: &Fixture, key: &str, body: &[u8], content_type: &str) -> String {
    let request = cratefield_core::axum::http::Request::builder()
        .method(Method::POST)
        .uri("/v1/documents/upload")
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", content_type)
        .header("x-file-name", "Rechnung-0042.pdf")
        .body(cratefield_core::axum::body::Body::from(body.to_vec()))
        .expect("request builds");
    let response = pollster::block_on(tower::ServiceExt::oneshot(fx.router().clone(), request))
        .expect("router answers");
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body");
    let body = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    body["document"]["id"].as_str().unwrap().to_owned()
}

/// Deletes a document as the owner and returns the response.
fn delete_as_human(fx: &Fixture, doc_id: &str) -> Reply {
    call(
        fx,
        Method::POST,
        &format!("/v1/documents/{doc_id}/delete"),
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    )
}

/// Asserts that a document exists in the database.
fn assert_document_exists(fx: &Fixture, doc_id: &str) {
    let doc_rows = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT COUNT(*) AS n FROM document WHERE company_id = ? AND id = ?".to_owned(),
        vec![
            sea_query::Value::from(common::COMPANY),
            sea_query::Value::from(doc_id),
        ],
    )))
    .expect("query document");
    let doc_count = doc_rows
        .first()
        .and_then(|r| r.get::<i64>("n"))
        .unwrap_or(0);
    assert_eq!(doc_count, 1, "document should exist");
}

/// Asserts that a blob does not exist in storage.
fn assert_blob_deleted(fx: &Fixture, sha256: &str) {
    let blob_key = format!("documents/{sha256}");
    let blob_result = pollster::block_on(fx.blob.get(&blob_key));
    assert!(
        blob_result.ok().flatten().is_none(),
        "blob should be deleted, key: {blob_key}"
    );
}

/// Asserts that a delete request for a document fails with document-linked conflict.
fn assert_delete_fails_linked(fx: &Fixture, doc_id: &str) {
    let response = delete_as_human(fx, doc_id);
    assert_eq!(response.status, StatusCode::CONFLICT);
    let body = response.json();
    let problem_type = body["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    assert_eq!(problem_type, "document-linked");
}

/// Extracts and posts a bill from a document, returning the entry id.
fn extract_and_post_bill(fx: &Fixture, doc_id: &str) -> String {
    let extract_response = fx.send(
        Method::POST,
        "/v1/bills/extract",
        &fx.keys.full,
        Some(&json!({"document_id": doc_id}).to_string()),
    );
    assert_eq!(extract_response.status, StatusCode::OK, "extract failed");
    let bill_id = extract_response.json()["bill"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let complete_fields = json!({
        "vendor_name": "Halden Works GmbH",
        "vendor_vat_id": "DE123456789",
        "vendor_iban": "DE89370400440532013000",
        "invoice_number": "RE-2026-0042",
        "invoice_date": "2026-10-15",
        "due_date": "2026-11-14",
        "currency": "EUR",
        "net_minor": 100_000,
        "vat_minor": 19_000,
        "gross_minor": 119_000,
        "lines": [{
            "description": "Office paper",
            "net_minor": 100_000,
            "vat_rate_bp": 1900,
            "vat_minor": 19_000,
            "gross_minor": 119_000,
            "expense_account": "5060"
        }],
        "source": "manual"
    });

    let update_response = fx.send(
        Method::POST,
        &format!("/v1/bills/{bill_id}/update"),
        &fx.keys.draft,
        Some(&complete_fields.to_string()),
    );
    assert_eq!(update_response.status, StatusCode::OK, "bill update failed");

    let confirm_response = call(
        fx,
        Method::POST,
        &format!("/v1/bills/{bill_id}/confirm"),
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(
        confirm_response.status,
        StatusCode::OK,
        "bill confirm failed"
    );

    let post_response = call(
        fx,
        Method::POST,
        &format!("/v1/bills/{bill_id}/post"),
        Auth::Session(OWNER_SESSION),
        true,
        Some(json!({})),
    );
    assert_eq!(post_response.status, StatusCode::OK, "bill post failed");

    post_response.json()["entry"]["entry_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Verifies that a document id is in the `source_documents` of a journal entry.
fn assert_document_in_entry(fx: &Fixture, entry_id: &str, doc_id: &str) {
    let entry_rows =
        pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
            "SELECT source_documents FROM journal_entry WHERE company_id = ? AND id = ?".to_owned(),
            vec![
                sea_query::Value::from(common::COMPANY),
                sea_query::Value::from(entry_id),
            ],
        )))
        .expect("query entry");
    let source_docs: String = entry_rows
        .first()
        .and_then(|r| r.get::<String>("source_documents"))
        .unwrap_or_default();
    assert!(
        source_docs.contains(&format!("\"{doc_id}\"")),
        "document id should be in source_documents: {source_docs}"
    );
}

/// Posts a journal entry with a document in `source_documents`, returning the entry id.
fn post_entry_with_document(fx: &Fixture, doc_id: &str) -> String {
    let mut entry_input = common::entry_json("doc-test-1", "5010", "4000", "100000");
    if let Some(entry_obj) = entry_input["entry"].as_object_mut() {
        entry_obj.insert("source_documents".to_owned(), serde_json::json!([doc_id]));
    }

    let post_response = call(
        fx,
        Method::POST,
        "/v1/entries",
        Auth::Session(OWNER_SESSION),
        true,
        Some(entry_input),
    );
    assert_eq!(post_response.status, StatusCode::OK, "entry post failed");

    post_response.json()["entry"]["entry_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn deleting_an_unlinked_document_succeeds() {
    let fx = person_fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let response = delete_as_human(&fx, &doc_id);
    assert_eq!(response.status, StatusCode::OK, "{}", response.text);
    let body = serde_json::from_str::<serde_json::Value>(&response.text).unwrap();
    let sha256 = body["sha256"].as_str().expect("sha256 in response");
    assert_eq!(sha256.len(), 64);

    assert_blob_deleted(&fx, sha256);
}

#[test]
fn an_agent_key_cannot_delete_a_document() {
    let fx = fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let response = fx.send(
        Method::POST,
        &format!("/v1/documents/{doc_id}/delete"),
        &fx.keys.draft,
        Some("{}"),
    );
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(
        response.json()["detail"]
            .as_str()
            .unwrap_or("")
            .contains("human-only")
    );
}

#[test]
fn deleting_a_document_linked_to_a_posted_entry_fails() {
    let fx = person_fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let entry_id = extract_and_post_bill(&fx, &doc_id);
    assert_document_in_entry(&fx, &entry_id, &doc_id);

    assert_delete_fails_linked(&fx, &doc_id);
    assert_document_exists(&fx, &doc_id);
}

#[test]
fn anchors_roundtrip_correctly() {
    let fx = fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let anchors = vec![
        Anchor {
            field: "page_anchor".to_owned(),
            location: Location::Page {
                page: 1,
                bbox: BoundingBox {
                    x: 1000,
                    y: 2000,
                    w: 3000,
                    h: 500,
                },
            },
        },
        Anchor {
            field: "invoice_number".to_owned(),
            location: Location::Xml {
                path: "/Invoice/ID[1]".to_owned(),
            },
        },
    ];

    let set_response = fx.send(
        Method::POST,
        &format!("/v1/documents/{doc_id}/set-anchors"),
        &fx.keys.draft,
        Some(&serde_json::to_string(&json!({"id": doc_id, "anchors": anchors.clone()})).unwrap()),
    );
    assert_eq!(set_response.status, StatusCode::OK);
    assert_eq!(set_response.json()["count"], 2);

    let get_response = fx.send(
        Method::GET,
        &format!("/v1/documents/{doc_id}/anchors"),
        &fx.keys.read,
        None,
    );
    assert_eq!(get_response.status, StatusCode::OK);
    let body = get_response.json();
    let returned = body["anchors"].as_array().unwrap();
    assert_eq!(returned.len(), 2);
    assert_eq!(returned[0]["field"], "invoice_number"); // Sorted by field
    assert_eq!(returned[1]["field"], "page_anchor");

    // Deserialize the page anchor and test on_page()
    let page_anchor_raw = &returned[1];
    assert_eq!(page_anchor_raw["field"], "page_anchor");

    #[allow(clippy::cast_possible_truncation)]
    let page = page_anchor_raw["location"]["page"].as_u64().unwrap() as u32;
    assert_eq!(page, 1);

    #[allow(clippy::cast_possible_truncation)]
    let bbox_x = page_anchor_raw["location"]["bbox"]["x"].as_u64().unwrap() as u32;
    #[allow(clippy::cast_possible_truncation)]
    let bbox_y = page_anchor_raw["location"]["bbox"]["y"].as_u64().unwrap() as u32;
    #[allow(clippy::cast_possible_truncation)]
    let bbox_w = page_anchor_raw["location"]["bbox"]["w"].as_u64().unwrap() as u32;
    #[allow(clippy::cast_possible_truncation)]
    let bbox_h = page_anchor_raw["location"]["bbox"]["h"].as_u64().unwrap() as u32;

    let bbox = BoundingBox {
        x: bbox_x,
        y: bbox_y,
        w: bbox_w,
        h: bbox_h,
    };

    // A4 page at 96 DPI: 1240×1754 px; rounding outward for coverage.
    // x: floor(1000 * 1240 / 10000) = floor(124.0) = 124
    // x_right: ceil((1000+3000) * 1240 / 10000) = ceil(496.0) = 496, w = 496-124 = 372
    // y: floor(2000 * 1754 / 10000) = floor(350.8) = 350
    // y_bottom: ceil((2000+500) * 1754 / 10000) = ceil(438.5) = 439, h = 439-350 = 89
    let pixel_rect = bbox.on_page(1240, 1754);
    assert_eq!(pixel_rect.x, 124, "x: floor(1000*1240/10000) = 124");
    assert_eq!(pixel_rect.y, 350, "y: floor(2000*1754/10000) = 350");
    assert_eq!(pixel_rect.w, 372, "w: 496-124 = 372");
    assert_eq!(pixel_rect.h, 89, "h: 439-350 = 89");
}

#[test]
fn invalid_anchors_are_rejected() {
    let fx = fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let bad_bbox = json!({
        "id": doc_id,
        "anchors": [
            {
                "field": "total",
                "page": 1,
                "bbox": {
                    "x": 15000,
                    "y": 0,
                    "w": 1,
                    "h": 1
                }
            }
        ]
    });

    let response = fx.send(
        Method::POST,
        &format!("/v1/documents/{doc_id}/set-anchors"),
        &fx.keys.draft,
        Some(&serde_json::to_string(&bad_bbox).unwrap()),
    );
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn both_page_and_xml_location_are_forbidden() {
    let fx = fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let bad_anchor = json!({
        "id": doc_id,
        "anchors": [
            {
                "field": "total",
                "page": 1,
                "bbox": {
                    "x": 0,
                    "y": 0,
                    "w": 100,
                    "h": 100
                },
                "path": "/Invoice/Total"
            }
        ]
    });

    let response = fx.send(
        Method::POST,
        &format!("/v1/documents/{doc_id}/set-anchors"),
        &fx.keys.draft,
        Some(&serde_json::to_string(&bad_anchor).unwrap()),
    );
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn deleting_a_document_referenced_by_a_posted_entry_fails() {
    let fx = person_fx();
    let doc_id = upload_document(&fx, &fx.keys.draft, &invoice_pdf(), "application/pdf");

    let entry_id = post_entry_with_document(&fx, &doc_id);
    assert_document_in_entry(&fx, &entry_id, &doc_id);

    assert_delete_fails_linked(&fx, &doc_id);
    assert_document_exists(&fx, &doc_id);
}
