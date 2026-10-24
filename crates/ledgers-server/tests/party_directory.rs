//! The party directory over HTTP: the logo rule (a stored PNG or JPEG of at
//! most 2 MiB, in the company's own documents), the CSV round trip, the agent
//! key's refusal to write bank rows from a file, and the open-items read.
mod common;

use common::http::{APP_ORIGIN, Auth, Reply, SESSION_COOKIE, call};
use common::{Fixture, OWNER_SESSION, fixture_with_options, owner_session};
use cratefield_core::axum::body::Body;
use cratefield_core::axum::http::{Method, Request, header};
use serde_json::{Value, json};

/// A valid PNG: the bundled Ledgers logo, which the seed also uses.
fn png() -> Vec<u8> {
    include_bytes!("../../../fixtures/demo/logos/ledgers.png").to_vec()
}

/// A PNG-signed file one byte over the logo limit.
fn oversize_png() -> Vec<u8> {
    let mut bytes = png();
    let over = usize::try_from(ledgers_documents::logo::MAX_LOGO_BYTES).expect("fits") + 1;
    bytes.resize(over, 0);
    bytes
}

fn owned() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    fx
}

fn as_owner(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Reply {
    call(fx, method, path, Auth::Session(OWNER_SESSION), true, body)
}

/// Uploads raw bytes as the owner, declaring `content_type`.
fn upload(fx: &Fixture, bytes: &[u8], content_type: &str) -> Reply {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/documents/upload")
        .header(header::COOKIE, format!("{SESSION_COOKIE}={OWNER_SESSION}"))
        .header(header::ORIGIN, APP_ORIGIN)
        .header(header::CONTENT_TYPE, content_type)
        .header("x-file-name", "party.bin")
        .body(Body::from(bytes.to_vec()))
        .expect("request");
    let response = pollster::block_on(tower::ServiceExt::oneshot(fx.router().clone(), request))
        .expect("router answers");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = pollster::block_on(cratefield_core::axum::body::to_bytes(
        response.into_body(),
        usize::MAX,
    ))
    .expect("body reads");
    Reply {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn stored_id(reply: &Reply) -> String {
    assert_eq!(
        reply.status.as_u16(),
        201,
        "the upload is stored: {}",
        reply.text
    );
    reply.json()["document"]["id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

/// The problem slug in a refusal's `type` URI.
fn slug(reply: &Reply) -> String {
    reply.json()["type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_party_logo_is_a_stored_png_or_jpeg_of_at_most_two_mebibytes() {
    let fx = owned();
    let created = as_owner(
        &fx,
        Method::POST,
        "/v1/parties",
        Some(json!({"id": "p-logo", "name": "Logo Party GmbH", "roles": "customer"})),
    );
    assert_eq!(created.status.as_u16(), 200, "{}", created.text);

    let image = stored_id(&upload(&fx, &png(), "image/png"));
    let set = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/p-logo/set-logo",
        Some(json!({"logo_document_id": image})),
    );
    assert_eq!(set.status.as_u16(), 200, "{}", set.text);
    assert_eq!(set.json()["party"]["logo_document_id"], image.as_str());

    let read = as_owner(&fx, Method::GET, "/v1/parties/p-logo", None);
    assert_eq!(
        read.json()["party"]["logo_document_id"],
        image.as_str(),
        "the detail page reads the logo"
    );

    let pdf_bytes = ledgers_extract::fixture::pdf_with_text(&["Party logo refused"]);
    let pdf = stored_id(&upload(&fx, &pdf_bytes, "application/pdf"));
    let refused = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/p-logo/set-logo",
        Some(json!({"logo_document_id": pdf})),
    );
    assert_eq!(refused.status.as_u16(), 422, "{}", refused.text);
    assert_eq!(slug(&refused), "party-logo-not-image");

    let big = stored_id(&upload(&fx, &oversize_png(), "image/png"));
    let too_large = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/p-logo/set-logo",
        Some(json!({"logo_document_id": big})),
    );
    assert_eq!(too_large.status.as_u16(), 422, "{}", too_large.text);
    assert_eq!(slug(&too_large), "party-logo-too-large");

    let missing = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/p-logo/set-logo",
        Some(json!({"logo_document_id": "doc-not-stored"})),
    );
    assert_eq!(slug(&missing), "party-logo-not-stored");

    // The refused logos never reached the party.
    let still = as_owner(&fx, Method::GET, "/v1/parties/p-logo", None);
    assert_eq!(still.json()["party"]["logo_document_id"], image.as_str());

    let cleared = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/p-logo/set-logo",
        Some(json!({"logo_document_id": null})),
    );
    assert_eq!(cleared.status.as_u16(), 200, "{}", cleared.text);
    assert!(cleared.json()["party"]["logo_document_id"].is_null());
}

#[test]
fn a_party_file_round_trips_over_http_and_an_agent_key_cannot_import_bank_rows() {
    let fx = fixture_with_options(Vec::new(), None, vec![APP_ORIGIN.to_owned()]);
    owner_session(&fx);
    let file = "id,name,website,iban,bic,bank_reason\n\
                csv-1,Csv Party Ltd,https://csv.example,DE89370400440532013000,,Moved\n";

    // An agent key checks the file: the bank row is refused, and nothing is written.
    let agent_check = call(
        &fx,
        Method::POST,
        "/v1/parties/import/check",
        Auth::Bearer(&fx.keys.draft),
        false,
        Some(json!({"csv": file})),
    );
    assert_eq!(agent_check.status.as_u16(), 200, "{}", agent_check.text);
    assert_eq!(agent_check.json()["valid"], false);
    assert_eq!(
        agent_check.json()["rows"][0]["errors"][0]["slug"],
        "party-bank-needs-person"
    );

    let agent_import = call(
        &fx,
        Method::POST,
        "/v1/parties/import",
        Auth::Bearer(&fx.keys.draft),
        false,
        Some(json!({"csv": file})),
    );
    assert_eq!(agent_import.status.as_u16(), 422, "{}", agent_import.text);
    assert_eq!(slug(&agent_import), "party-import-invalid");

    // The owner imports the same file, bank details included.
    let owner_import = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/import",
        Some(json!({"csv": file})),
    );
    assert_eq!(owner_import.status.as_u16(), 200, "{}", owner_import.text);
    assert_eq!(owner_import.json()["created"], 1);

    // The export carries the party back in the same columns; a re-import of it changes nothing.
    let export = call(
        &fx,
        Method::GET,
        "/v1/parties/export",
        Auth::Bearer(&fx.keys.read),
        false,
        None,
    );
    assert_eq!(export.status.as_u16(), 200, "{}", export.text);
    let csv = export.json()["csv"].as_str().expect("csv text").to_owned();
    assert!(csv.starts_with("id,name,trading_name,roles,"), "{csv}");
    assert!(csv.contains("csv-1,Csv Party Ltd,"), "{csv}");
    let again = as_owner(
        &fx,
        Method::POST,
        "/v1/parties/import",
        Some(json!({"csv": csv})),
    );
    assert_eq!(again.status.as_u16(), 200, "{}", again.text);
    assert_eq!(
        (
            again.json()["created"].as_u64(),
            again.json()["updated"].as_u64()
        ),
        (Some(0), Some(1))
    );
}

#[test]
fn open_items_read_is_empty_for_a_party_without_items() {
    let fx = owned();
    let created = as_owner(
        &fx,
        Method::POST,
        "/v1/parties",
        Some(json!({"id": "p-open", "name": "Open Party GmbH"})),
    );
    assert_eq!(created.status.as_u16(), 200, "{}", created.text);
    let open = as_owner(&fx, Method::GET, "/v1/parties/p-open/open-items", None);
    assert_eq!(open.status.as_u16(), 200, "{}", open.text);
    assert_eq!(open.json()["open_items"], json!([]));
    assert_eq!(open.json()["balances"], json!([]));
}
