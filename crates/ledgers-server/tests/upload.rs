//! Integration tests for document upload, dedupe and signed download links
//! (issue #20). The fixture is the real server stack over SQLite, with an
//! in-memory blob store.

mod common;

use common::{COMPANY, Fixture, fixture};
use cratefield_core::axum::http::{Method, StatusCode};
use ledgers_documents::sign;
use serde_json::Value;

/// A raw reply from the router: the status and the body bytes.
struct Got {
    status: StatusCode,
    body: Vec<u8>,
}

const DOC_KEY: &[u8] = common::DOCUMENT_KEY;

/// The fixture: the real stack, with the documents module mounted.
fn fx() -> Fixture {
    fixture()
}

/// A valid invented one-page PDF with a text layer.
fn invoice_pdf() -> Vec<u8> {
    ledgers_extract::fixture::pdf_with_text(&[
        "Halden Works GmbH",
        "Invoice No: RE-2026-0042",
        "Total 119,00 EUR",
    ])
}

/// Uploads `body` with `key` and the declared `content_type`.
fn upload(fx: &Fixture, key: &str, body: &[u8], content_type: &str) -> Got {
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
    let status = response.status();
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body");
    Got {
        status,
        body: bytes.to_vec(),
    }
}

fn json_of(response: &Got) -> Value {
    serde_json::from_slice(&response.body).unwrap_or(Value::Null)
}

#[test]
fn a_pdf_is_stored_once_with_its_hash_and_uploader() {
    let fx = fx();
    let pdf = invoice_pdf();
    let response = upload(&fx, &fx.keys.draft, &pdf, "application/pdf");
    assert_eq!(
        response.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let body = json_of(&response);
    assert_eq!(body["duplicate"], false);
    let document = &body["document"];
    assert_eq!(document["size_bytes"], pdf.len());
    assert_eq!(document["mime"], "application/pdf");
    assert_eq!(document["original_name"], "Rechnung-0042.pdf");
    assert_eq!(document["retention_class"], "ten-years");
    assert!(
        document["uploaded_by"]
            .as_str()
            .unwrap_or("")
            .starts_with("agent:")
    );
    let sha = document["sha256"].as_str().expect("hash").to_owned();
    assert_eq!(sha.len(), 64);
    // The original is kept under its hash: the download link serves the same bytes.
    let id = document["id"].as_str().expect("id").to_owned();
    let link = fx.send(
        Method::GET,
        &format!("/v1/documents/{id}/url"),
        &fx.keys.read,
        None,
    );
    let url = link.json()["url"].as_str().expect("url").to_owned();
    let served = fx.send_anonymous(Method::GET, &url, None);
    assert_eq!(served.body().as_ref(), pdf.as_slice());
}

#[test]
fn the_same_file_twice_is_one_document_and_two_upload_events() {
    let fx = fx();
    let pdf = invoice_pdf();
    let first = json_of(&upload(&fx, &fx.keys.draft, &pdf, "application/pdf"));
    let second_response = upload(&fx, &fx.keys.draft, &pdf, "application/pdf");
    assert_eq!(second_response.status, StatusCode::OK);
    let second = json_of(&second_response);
    assert_eq!(second["duplicate"], true);
    assert!(
        second["warning"]
            .as_str()
            .unwrap_or("")
            .contains("already stored")
    );
    assert_eq!(first["document"]["id"], second["document"]["id"]);

    let events = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT COUNT(*) AS n FROM document_upload_event WHERE document_id = ?".to_owned(),
        vec![sea_query::Value::from(
            first["document"]["id"].as_str().unwrap_or(""),
        )],
    )))
    .expect("count");
    assert_eq!(events.first().and_then(|r| r.get::<i64>("n")), Some(2));
    let documents = pollster::block_on(fx.deps.db.query(&cratefield_core::Statement::with_values(
        "SELECT COUNT(*) AS n FROM document WHERE company_id = ?".to_owned(),
        vec![sea_query::Value::from(COMPANY)],
    )))
    .expect("count documents");
    assert_eq!(documents.first().and_then(|r| r.get::<i64>("n")), Some(1));
}

#[test]
fn an_executable_renamed_to_pdf_is_refused_by_its_bytes() {
    let fx = fx();
    let elf = b"\x7fELF\x02\x01\x01\x00 not a pdf at all".to_vec();
    let response = upload(&fx, &fx.keys.draft, &elf, "application/pdf");
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        json_of(&response)["detail"]
            .as_str()
            .unwrap_or("")
            .contains("PDF, PNG and JPEG")
    );
}

#[test]
fn a_declared_type_that_does_not_match_the_bytes_is_refused() {
    let fx = fx();
    let response = upload(&fx, &fx.keys.draft, &invoice_pdf(), "image/png");
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        json_of(&response)["detail"]
            .as_str()
            .unwrap_or("")
            .contains("does not match")
    );
}

#[test]
fn an_eleven_mebibyte_file_is_refused_as_too_large() {
    let fx = fx();
    let mut pdf = b"%PDF-1.7\n".to_vec();
    pdf.resize(11 * 1024 * 1024, b'X');
    let response = upload(&fx, &fx.keys.draft, &pdf, "application/pdf");
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[test]
fn a_read_tier_key_cannot_upload_and_no_credential_is_unauthenticated() {
    let fx = fx();
    let read = upload(&fx, &fx.keys.read, &invoice_pdf(), "application/pdf");
    assert_eq!(read.status, StatusCode::FORBIDDEN);
    let anonymous = fx.send_anonymous(Method::POST, "/v1/documents/upload", Some("{}"));
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[test]
fn a_download_link_serves_the_original_until_it_expires() {
    let fx = fx();
    let pdf = invoice_pdf();
    let body = json_of(&upload(&fx, &fx.keys.draft, &pdf, "application/pdf"));
    let id = body["document"]["id"].as_str().expect("id").to_owned();
    let sha = body["document"]["sha256"].as_str().expect("sha").to_owned();

    let link = fx.send(
        Method::GET,
        &format!("/v1/documents/{id}/url"),
        &fx.keys.read,
        None,
    );
    assert_eq!(
        link.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(link.body().as_ref())
    );
    let url = link.json()["url"].as_str().expect("url").to_owned();
    assert!(url.starts_with(&format!(
        "/v1/documents/{id}/file?company={COMPANY}&expires="
    )));

    // The link works without any credential: the signature is the credential.
    let served = fx.send_anonymous(Method::GET, &url, None);
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.body().as_ref(), pdf.as_slice());

    // Once the expiry passes, the same signature is refused.
    let expired_at = 1_000;
    let expired_sig = sign::sign(DOC_KEY, COMPANY, &id, &sha, expired_at);
    let expired = fx.send_anonymous(
        Method::GET,
        &format!(
            "/v1/documents/{id}/file?company={COMPANY}&expires={expired_at}&signature={expired_sig}"
        ),
        None,
    );
    assert_eq!(expired.status, StatusCode::FORBIDDEN);
    assert!(
        expired.json()["detail"]
            .as_str()
            .unwrap_or("")
            .contains("expired")
    );

    // A forged signature, or a link moved to another document, is refused.
    let forged = fx.send_anonymous(
        Method::GET,
        &format!(
            "/v1/documents/{id}/file?company={COMPANY}&expires={}&signature=00",
            4_000_000_000i64
        ),
        None,
    );
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

/// One anonymous download through the router, naming `company` with the
/// `X-Ledgers-Company` header when given. Returns the status and body bytes.
fn download_as(fx: &Fixture, path: &str, company: Option<&str>) -> Got {
    let mut builder = cratefield_core::axum::http::Request::builder()
        .method(Method::GET)
        .uri(path);
    if let Some(company) = company {
        builder = builder.header("x-ledgers-company", company);
    }
    let request = builder
        .body(cratefield_core::axum::body::Body::empty())
        .expect("request builds");
    let response = pollster::block_on(tower::ServiceExt::oneshot(fx.router().clone(), request))
        .expect("router answers");
    let status = response.status();
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body");
    Got {
        status,
        body: bytes.to_vec(),
    }
}

#[test]
fn a_download_link_serves_only_the_company_it_was_made_for() {
    let fx = fx();
    let pdf = invoice_pdf();
    let body = json_of(&upload(&fx, &fx.keys.draft, &pdf, "application/pdf"));
    let id = body["document"]["id"].as_str().expect("id").to_owned();
    let link = fx.send(
        Method::GET,
        &format!("/v1/documents/{id}/url"),
        &fx.keys.read,
        None,
    );
    let url = link.json()["url"].as_str().expect("url").to_owned();

    // The link serves its own company, named or not.
    assert_eq!(download_as(&fx, &url, Some(COMPANY)).status, StatusCode::OK);
    assert_eq!(download_as(&fx, &url, None).status, StatusCode::OK);

    // Named against another company, the link is refused before any read.
    let other = download_as(&fx, &url, Some("01OTHERCOMPANY"));
    assert_eq!(other.status, StatusCode::FORBIDDEN);

    // Moving the link's company to another one breaks the signature: the
    // company is part of what was signed.
    let moved = url.replace(&format!("company={COMPANY}"), "company=01OTHERCOMPANY");
    let refused = download_as(&fx, &moved, Some("01OTHERCOMPANY"));
    // The document is not in the other company, so it is not found there;
    // either way no bytes of the original are served.
    assert_ne!(refused.status, StatusCode::OK);
    assert_ne!(refused.body, pdf);
    // A link signed for another company's id never verifies here.
    let sig = sign::sign(
        DOC_KEY,
        "01OTHERCOMPANY",
        &id,
        body["document"]["sha256"].as_str().expect("sha"),
        4_000_000_000,
    );
    let foreign =
        format!("/v1/documents/{id}/file?company={COMPANY}&expires=4000000000&signature={sig}");
    let foreign_reply = download_as(&fx, &foreign, None);
    assert_eq!(foreign_reply.status, StatusCode::FORBIDDEN);
    assert!(String::from_utf8_lossy(&foreign_reply.body).contains("signature"));
}
