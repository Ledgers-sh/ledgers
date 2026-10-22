//! `DocumentsModule`: the upload and download routes for source documents
//! (docs/PLAN.md section 6, issue #20).
//!
//! `POST /v1/documents/upload` reads the body (at most 10 MiB), sniffs the
//! type from its bytes, hashes it, and registers it through the command
//! `documents.register` with the caller's own credential, so the tier check
//! and the audit trail are the executor's. A file already stored is not
//! stored twice: the response says so and the upload is still recorded.
//!
//! `GET /v1/documents/{id}/file?expires=&signature=` serves the original to
//! anyone holding a link made by `documents.download_url` before it expires.

use crate::dispatch::Deps;
use crate::people::presented_from;
use crate::rate_limiting::admit;
use crate::routes::{Dispatch, DomainModule};
use axum::Json;
use axum::http::StatusCode;
use bytes::{Bytes, BytesMut};
use cratefield_core::axum::{
    self, extract::Query, extract::State, http::header, response::IntoResponse, routing::get,
    routing::post,
};
use cratefield_core::{
    Blob, BlobError, Clock, Config, ConfigError, Database, Migrations, Module, ModuleContext, Port,
    Problem, RequestStream, Scope, StreamError, StreamRoute,
};
use futures_util::StreamExt;
use ledgers_commands::CommandError;
use ledgers_documents::sign::{self, LinkError};
use ledgers_documents::sniff::{self, Refusal};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

/// The blob key of a document's original: content-addressed by SHA-256.
#[must_use]
pub fn blob_key(sha256: &str) -> String {
    format!("documents/{sha256}")
}

/// Upload and download state, shared by the handlers.
#[derive(Clone)]
struct DocState {
    blob: Arc<dyn Blob>,
    db: Arc<dyn Database>,
    clock: Arc<dyn Clock>,
    deps: Arc<Deps>,
    dispatch: Arc<dyn Dispatch>,
    company_id: String,
    signing_key: Arc<Vec<u8>>,
}

/// The documents module: the upload and download routes, and the command
/// routes of the `documents` domain (`routes` from [`routes::domain_routes`]).
pub struct DocumentsModule {
    company_id: String,
    signing_key: Vec<u8>,
    dispatch: Arc<dyn Dispatch>,
    routes: Vec<crate::routes::CommandRoute>,
}

impl DocumentsModule {
    /// A module serving `company_id` through `dispatch`, signing download
    /// links with `signing_key`, and mounting the `documents` command routes.
    #[must_use]
    pub fn new(
        company_id: impl Into<String>,
        signing_key: Vec<u8>,
        dispatch: Arc<dyn Dispatch>,
        routes: Vec<crate::routes::CommandRoute>,
    ) -> Self {
        Self {
            company_id: company_id.into(),
            signing_key,
            dispatch,
            routes,
        }
    }
}

impl Module for DocumentsModule {
    fn name(&self) -> &'static str {
        "documents"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[Port::Blob, Port::IdGen, Port::Db, Port::Clock]
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        let deps = Deps::from_ports(&ctx.ports).expect("db, clock and id generator are required");
        let state = DocState {
            blob: ctx.ports.blob.clone().expect("blob port is required"),
            db: deps.db.clone(),
            clock: deps.clock.clone(),
            deps: Arc::new(deps),
            dispatch: self.dispatch.clone(),
            company_id: self.company_id.clone(),
            signing_key: Arc::new(self.signing_key.clone()),
        };
        let commands =
            DomainModule::new("documents", self.dispatch.clone(), self.routes.clone()).router(ctx);
        let files = axum::Router::new()
            .route("/upload", post(upload))
            .route("/{id}/file", get(download))
            .with_state(state);
        commands.merge(files)
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }

    fn max_blob_object_bytes(&self) -> u64 {
        sniff::MAX_BYTES as u64
    }

    fn streaming_routes(&self) -> &'static [StreamRoute] {
        #[allow(clippy::cast_possible_truncation)]
        const ROUTES: &[StreamRoute] = &[StreamRoute::post(
            "/upload",
            cratefield_core::MAX_LARGE_BLOB_BYTES as usize,
        )];
        ROUTES
    }
}

/// Reads the streamed body, refusing anything over the upload limit.
async fn read_body(mut stream: RequestStream) -> Result<Bytes, Problem> {
    let mut buffer = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk: Result<Bytes, StreamError> = chunk;
        let chunk = chunk.map_err(|_| Problem::validation_failed("the upload was interrupted"))?;
        if buffer.len() + chunk.len() > sniff::MAX_BYTES {
            return Err(refusal_problem(&Refusal::TooLarge {
                size: buffer.len() + chunk.len(),
            }));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer.freeze())
}

fn refusal_problem(refusal: &Refusal) -> Problem {
    match refusal {
        Refusal::TooLarge { .. } => Problem::request_too_large().with_detail(refusal.to_string()),
        Refusal::UnsupportedType | Refusal::TypeMismatch { .. } => {
            Problem::validation_failed(refusal.to_string())
        }
        _ => Problem::validation_failed(refusal.to_string()),
    }
}

#[allow(clippy::too_many_lines)]
/// `POST /v1/documents/upload`: stores a PDF, PNG or JPEG (201), or records
/// another upload of a file already stored (200 with `duplicate: true`).
async fn upload(
    scope: Scope,
    State(state): State<DocState>,
    headers: axum::http::HeaderMap,
    stream: RequestStream,
) -> axum::response::Response {
    let request_id = scope.request_id.clone();
    let presented = presented_from(&headers, false);
    if presented.is_none() {
        return CommandError::Unauthenticated
            .to_problem()
            .instance(&request_id)
            .into_response();
    }
    // The upload is registered in the company the request names (its
    // `X-Ledgers-Company` header), or the deployment's company when it names none.
    let dispatch = match crate::routes::company_dispatch(&state.dispatch, &headers) {
        Ok(scoped) => scoped,
        Err(problem) => return problem.instance(&request_id).into_response(),
    };
    if let Err(limited) = admit(dispatch.as_ref(), &state.deps, presented.as_ref()).await {
        return limited.into_response();
    }
    let bytes = match read_body(stream).await {
        Ok(bytes) => bytes,
        Err(problem) => return problem.instance(&request_id).into_response(),
    };
    let kind = match sniff::sniff(&bytes) {
        Ok(kind) => kind,
        Err(refusal) => {
            return refusal_problem(&refusal)
                .instance(&request_id)
                .into_response();
        }
    };
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if let Err(refusal) = sniff::check_declared(declared, kind) {
        return refusal_problem(&refusal)
            .instance(&request_id)
            .into_response();
    }
    let text = match kind {
        sniff::Kind::Pdf => match ledgers_extract::pdf::text_layer(&bytes) {
            Ok(text) => text,
            Err(err) => {
                return Problem::validation_failed(format!("this PDF cannot be read: {err}"))
                    .instance(&request_id)
                    .into_response();
            }
        },
        // A bank statement is read from its stored text, so the whole file must
        // fit the text layer: a truncated statement would lose lines.
        sniff::Kind::Camt053 | sniff::Kind::Mt940 | sniff::Kind::BankCsv => {
            if bytes.len() > ledgers_documents::commands::MAX_TEXT_LAYER_BYTES {
                return Problem::validation_failed(
                    "a bank statement is read up to 512 KiB: split the file by period",
                )
                .instance(&request_id)
                .into_response();
            }
            // The sniffer accepted only UTF-8 text for these kinds.
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
        sniff::Kind::Png | sniff::Kind::Jpeg => None,
    };
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let original_name = headers
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .map_or_else(|| "document".to_owned(), ToString::to_string);
    let input = json!({
        "id": "",
        "sha256": sha256,
        "size_bytes": bytes.len(),
        "mime": kind.mime(),
        "original_name": original_name,
        "uploaded_by": "",
        "text_layer": text,
    });
    let result = dispatch
        .dispatch(
            &state.deps,
            "documents.register",
            input,
            &request_id,
            presented,
        )
        .await;
    let registered = match result {
        Ok(value) => value,
        Err(problem) => return problem.into_response(),
    };
    let duplicate = registered
        .get("duplicate")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let key = blob_key(&sha256);
    let stored = match state.blob.get(&key).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(err) => {
            tracing::error!("blob read failed: {err:?}");
            return Problem::internal()
                .with_detail("document storage is unavailable")
                .into_response();
        }
    };
    if !stored && let Err(err) = state.blob.put(&key, &bytes, kind.mime()).await {
        return blob_failure(err);
    }
    let body = json!({
        "document": registered.get("document").cloned().unwrap_or(Value::Null),
        "duplicate": duplicate,
        "warning": if duplicate {
            Value::String("This file was already stored. It is linked to the existing document and no second copy was kept.".to_owned())
        } else {
            Value::Null
        },
    });
    let status = if duplicate {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (status, Json(body)).into_response()
}

fn blob_failure(err: BlobError) -> axum::response::Response {
    match err {
        BlobError::TooLarge(msg) => Problem::request_too_large()
            .with_detail(msg)
            .into_response(),
        other => {
            tracing::error!("blob write failed: {other:?}");
            Problem::internal()
                .with_detail("document upload failed")
                .into_response()
        }
    }
}

/// `GET /v1/documents/{id}/file`: serves the original to a valid, unexpired link.
async fn download(
    State(state): State<DocState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> axum::response::Response {
    // Links are off until LEDGERS_DOCUMENT_SIGNING_KEY is set. An empty key
    // must never verify a signature (an HMAC under it is forgeable), so the
    // route answers 503 before it reads anything.
    if state.signing_key.is_empty() {
        return document_links_off();
    }
    let forbidden = |detail: &str| {
        Problem::new(&FORBIDDEN_LINK)
            .with_detail(detail)
            .into_response()
    };
    let (Some(expires), Some(signature)) = (query.get("expires"), query.get("signature")) else {
        return forbidden("the link has no expiry or signature");
    };
    let Ok(expires_at) = expires.parse::<i64>() else {
        return forbidden("the link expiry is not a timestamp");
    };
    let document =
        match ledgers_documents::store::get(state.db.as_ref(), &state.company_id, &id).await {
            Ok(Some(document)) => document,
            Ok(None) => return Problem::not_found().into_response(),
            Err(err) => {
                tracing::error!("document read failed: {err:?}");
                return Problem::internal().into_response();
            }
        };
    let now = state.clock.now().unix_timestamp();
    match sign::verify(
        &state.signing_key,
        &id,
        &document.sha256,
        expires_at,
        signature,
        now,
    ) {
        Ok(()) => {}
        Err(LinkError::Expired) => return forbidden("the link has expired; ask for a new one"),
        Err(LinkError::BadSignature) => return forbidden("the link signature does not match"),
    }
    let object = match state.blob.get(&blob_key(&document.sha256)).await {
        Ok(Some(object)) => object,
        Ok(None) => return Problem::not_found().into_response(),
        Err(err) => {
            tracing::error!("blob read failed: {err:?}");
            return Problem::internal().into_response();
        }
    };
    let safe_name: String = document
        .original_name
        .chars()
        .filter(|c| *c != '"' && !c.is_control())
        .collect();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, document.mime.clone()),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{safe_name}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        object.bytes,
    )
        .into_response()
}

/// The download routes' answer while links are off: 503 with the reason.
#[must_use]
pub fn document_links_off() -> axum::response::Response {
    Problem::not_ready(
        "document download links are not set up: LEDGERS_DOCUMENT_SIGNING_KEY is missing or shorter than 32 characters",
    )
    .into_response()
}

const FORBIDDEN_LINK: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "document-link-refused",
    status: StatusCode::FORBIDDEN,
    title: "The download link is not valid",
    description: "A document download link was expired or altered.",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_keys_are_content_addressed() {
        assert_eq!(blob_key("ab12"), "documents/ab12");
    }

    #[test]
    fn module_declares_its_ports_and_the_upload_route() {
        struct NoDispatch;
        #[async_trait::async_trait]
        impl Dispatch for NoDispatch {
            fn scoped(self: Arc<Self>, _company_id: &str) -> Arc<dyn Dispatch> {
                self
            }

            async fn dispatch(
                &self,
                _deps: &Deps,
                _command: &str,
                _input: Value,
                _request_id: &str,
                _presented: Option<crate::routes::Presented>,
            ) -> Result<Value, Problem> {
                Err(Problem::internal())
            }
            async fn principal(
                &self,
                _deps: &Deps,
                _credential: &str,
            ) -> Result<ledgers_commands::auth::Principal, Problem> {
                Err(Problem::internal())
            }
        }
        let module = DocumentsModule::new("c", b"k".to_vec(), Arc::new(NoDispatch), Vec::new());
        assert_eq!(module.name(), "documents");
        assert!(module.requires().contains(&Port::Blob));
        let routes = module.streaming_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/upload");
        assert_eq!(routes[0].method, axum::http::Method::POST);
    }
}
