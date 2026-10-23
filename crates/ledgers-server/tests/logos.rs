//! Logos over HTTP (company, business unit): a logo is a stored PNG or JPEG of
//! at most 2 MiB in the company's own documents. Anything else is refused with
//! 422 and nothing is saved, and an uploaded file that is not an image never
//! becomes a logo.
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

/// A PNG-signed file one byte over the logo limit: the sniffer reads the
/// signature, the logo rule reads the size.
fn oversize_png() -> Vec<u8> {
    let mut bytes = png();
    let over = usize::try_from(ledgers_documents::logo::MAX_LOGO_BYTES).expect("fits") + 1;
    bytes.resize(over, 0);
    bytes
}

/// The owner's session, the app origin, the fixture with documents mounted.
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
        .header("x-file-name", "logo.png")
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

fn company_with_logo(fx: &Fixture, logo: &str) -> Reply {
    as_owner(
        fx,
        Method::POST,
        "/v1/company",
        Some(json!({"name": "Halden Works GmbH", "profile": {"logo_document_id": logo}})),
    )
}

#[test]
fn a_png_logo_is_saved_on_the_company_profile_and_read_back() {
    let fx = owned();
    let id = stored_id(&upload(&fx, &png(), "image/png"));
    let saved = company_with_logo(&fx, &id);
    assert_eq!(saved.status.as_u16(), 200, "{}", saved.text);
    let read = as_owner(&fx, Method::GET, "/v1/company", None).json();
    assert_eq!(read["profile"]["logo_document_id"], id);
}

#[test]
fn a_pdf_or_an_unstored_id_is_refused_as_a_logo() {
    let fx = owned();
    let pdf = ledgers_extract::fixture::pdf_with_text(&["Halden Works GmbH", "Invoice"]);
    let pdf_id = stored_id(&upload(&fx, &pdf, "application/pdf"));
    let refused = company_with_logo(&fx, &pdf_id);
    assert_eq!(refused.status.as_u16(), 422, "{}", refused.text);
    assert!(refused.text.contains("PNG or JPEG"), "{}", refused.text);

    let unknown = company_with_logo(&fx, "01NOSUCHDOCUMENT0000000000");
    assert_eq!(unknown.status.as_u16(), 422, "{}", unknown.text);

    let path_like = company_with_logo(&fx, "../../etc/passwd");
    assert_eq!(path_like.status.as_u16(), 422, "{}", path_like.text);
}

#[test]
fn a_logo_over_two_mebibytes_is_refused_though_the_upload_itself_is_allowed() {
    let fx = owned();
    let id = stored_id(&upload(&fx, &oversize_png(), "image/png"));
    let refused = company_with_logo(&fx, &id);
    assert_eq!(refused.status.as_u16(), 422, "{}", refused.text);
    assert!(refused.text.contains("at most"), "{}", refused.text);
    let read = as_owner(&fx, Method::GET, "/v1/company", None).json();
    assert_eq!(read["profile"]["logo_document_id"], "", "nothing was saved");
}

#[test]
fn a_business_unit_takes_a_png_logo_and_refuses_a_pdf() {
    let fx = owned();
    let png_id = stored_id(&upload(&fx, &png(), "image/png"));
    let created = as_owner(
        &fx,
        Method::POST,
        "/v1/units",
        Some(
            json!({"code": "LDG", "name": "Ledgers", "colour": "#7FA6C9", "logo_document_id": png_id}),
        ),
    );
    assert!(created.status.is_success(), "{}", created.text);
    let listed = as_owner(&fx, Method::GET, "/v1/units", None).json();
    let logo = listed["units"]
        .as_array()
        .and_then(|units| units.iter().find(|unit| unit["code"] == "LDG"))
        .map(|unit| unit["logo_document_id"].clone());
    assert_eq!(
        logo,
        Some(json!(png_id)),
        "the unit keeps its logo: {listed}"
    );

    let pdf = ledgers_extract::fixture::pdf_with_text(&["Unit logo"]);
    let pdf_id = stored_id(&upload(&fx, &pdf, "application/pdf"));
    let refused = as_owner(
        &fx,
        Method::POST,
        "/v1/units",
        Some(
            json!({"code": "SQZ", "name": "Sqeezin", "colour": "#E9B44C", "logo_document_id": pdf_id}),
        ),
    );
    assert_eq!(refused.status.as_u16(), 422, "{}", refused.text);
}
