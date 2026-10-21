//! `DocumentsModule`: document uploads via streaming to the Blob port.
//!
//! Provides `/v1/documents/upload` for streaming document uploads with
//! configurable size limits. Default cap is `MAX_BLOB_BYTES` (10 MiB);
//! opt-in to larger objects via `DOCUMENTS_MAX_LARGE_BLOB_BYTES`.

use axum::Json;
use axum::http::StatusCode;
use bytes::Bytes;
use cratefield_core::axum::{
    self, extract::State, http::header, response::IntoResponse, routing::post,
};
use cratefield_core::{
    Blob, BlobError, BoxStream, Config, ConfigError, IdGen, MAX_BLOB_BYTES, MAX_LARGE_BLOB_BYTES,
    Migrations, Module, ModuleContext, Port, Problem, RequestStream, StreamError, StreamRoute,
};
use serde::Serialize;
use std::sync::Arc;

/// Default per-object cap = `MAX_BLOB_BYTES` (10 MiB).
const DEFAULT_MAX_OBJECT_BYTES: u64 = MAX_BLOB_BYTES as u64;

/// Content types allowed for document uploads.
const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "application/pdf",
    "image/png",
    "image/jpeg",
    "application/xml",
    "text/xml",
];

/// Upload state: shared across all handlers.
#[derive(Clone)]
struct UploadState {
    blob: Arc<dyn Blob>,
    id_gen: Arc<dyn IdGen>,
    max_object_bytes: u64,
}

/// The documents module: document uploads via streaming to the Blob port.
#[derive(Debug)]
pub struct DocumentsModule {
    /// The configured per-object ceiling, or `MAX_BLOB_BYTES` if not set.
    max_object_bytes: u64,
}

impl DocumentsModule {
    /// Creates a module with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_object_bytes: DEFAULT_MAX_OBJECT_BYTES,
        }
    }

    /// Creates a module reading `DOCUMENTS_MAX_LARGE_BLOB_BYTES` from config.
    ///
    /// # Errors
    ///
    /// Returns a string error if the config value is invalid or exceeds
    /// the harness ceiling.
    pub fn from_config(cfg: &dyn Config) -> Result<Self, String> {
        let max_object_bytes = match cfg.get("DOCUMENTS_MAX_LARGE_BLOB_BYTES") {
            None => DEFAULT_MAX_OBJECT_BYTES,
            Some(s) => {
                let bytes: u64 = s.parse().map_err(|_| {
                    format!("DOCUMENTS_MAX_LARGE_BLOB_BYTES must be a number, got '{s}'")
                })?;
                if bytes > MAX_LARGE_BLOB_BYTES {
                    return Err(format!(
                        "DOCUMENTS_MAX_LARGE_BLOB_BYTES must be <= {MAX_LARGE_BLOB_BYTES}, got {bytes}"
                    ));
                }
                bytes
            }
        };
        Ok(Self { max_object_bytes })
    }
}

impl Default for DocumentsModule {
    fn default() -> Self {
        Self::new()
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
        &[Port::Blob, Port::IdGen]
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        // Create the shared upload state
        let state = UploadState {
            blob: ctx.ports.blob.clone().expect("blob port is required"),
            id_gen: ctx.ports.id_gen.clone().expect("id_gen port is required"),
            max_object_bytes: self.max_object_bytes,
        };

        // Mount the streaming upload route
        axum::Router::new()
            .route("/upload", post(handle_upload_handler))
            .with_state(state)
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, cfg: &dyn Config) -> Result<(), ConfigError> {
        // Validate config by trying to create a module
        match Self::from_config(cfg) {
            Ok(_) => Ok(()),
            Err(e) => {
                let mut errors = ConfigError::default();
                errors.push(e);
                Err(errors)
            }
        }
    }

    fn max_blob_object_bytes(&self) -> u64 {
        self.max_object_bytes
    }

    fn streaming_routes(&self) -> &'static [StreamRoute] {
        // Set max_bytes to MAX_LARGE_BLOB_BYTES (the harness ceiling) so the
        // streamer cannot buffer more than we're willing to accept. Per-request
        // enforcement happens in the handler via the configured max_object_bytes.
        #[allow(clippy::cast_possible_truncation)]
        const ROUTES: &[StreamRoute] =
            &[StreamRoute::post("/upload", MAX_LARGE_BLOB_BYTES as usize)];
        ROUTES
    }
}

/// Response body for successful document upload (201).
#[derive(Debug, Serialize)]
struct UploadResponse {
    /// The document's key in blob storage (module-scoped).
    key: String,
    /// The number of bytes uploaded.
    bytes: u64,
    /// The content type stored with the document.
    content_type: String,
}

/// Axum handler for streaming uploads: extracts stream, state, and headers.
#[allow(clippy::result_large_err)]
async fn handle_upload_handler(
    State(state): State<UploadState>,
    headers: axum::http::HeaderMap,
    stream: RequestStream,
) -> Result<(StatusCode, Json<UploadResponse>), axum::response::Response> {
    // Extract content-type from headers
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);

    // Check content-type early to return 415 instead of 400
    if let Some(ref ct) = content_type
        && !ALLOWED_CONTENT_TYPES.contains(&ct.as_str())
    {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!(
                "content-type '{}' not allowed; supported: {}",
                ct,
                ALLOWED_CONTENT_TYPES.join(", ")
            ),
        )
            .into_response());
    }

    handle_upload(
        stream,
        content_type,
        Some(Arc::clone(&state.blob)),
        Some(Arc::clone(&state.id_gen)),
        state.max_object_bytes,
    )
    .await
    .map_err(IntoResponse::into_response)
}

/// The document upload handler: streams the body to blob storage.
async fn handle_upload(
    stream: RequestStream,
    content_type: Option<String>,
    blob: Option<std::sync::Arc<dyn Blob>>,
    id_gen: Option<std::sync::Arc<dyn IdGen>>,
    max_bytes: u64,
) -> Result<(StatusCode, Json<UploadResponse>), Problem> {
    // Get the blob port; documents requires it.
    let blob = blob.ok_or_else(|| Problem::internal().with_detail("blob port not available"))?;

    // Get the IdGen port; documents requires it.
    let id_gen = id_gen
        .ok_or_else(|| Problem::internal().with_detail("id generation port not available"))?;

    // Validate content-type is present (content-type check happens in the handler)
    let ct =
        content_type.ok_or_else(|| Problem::validation_failed("content-type header required"))?;

    // Convert RequestStream into a BoxStream. RequestStream implements Stream
    // over Result<Bytes, StreamError>, so we can box it directly.
    let box_stream: BoxStream<'static, Result<Bytes, StreamError>> = Box::pin(stream);

    // Generate a unique document id using the IdGen port (wasm-safe).
    let doc_id = format!("doc_{}", id_gen.ulid());

    // Upload via put_stream. The blob port will enforce max_bytes
    // and return the actual number of bytes written.
    let uploaded_bytes = blob
        .put_stream(&doc_id, box_stream, &ct, max_bytes)
        .await
        .map_err(|e| match e {
            BlobError::TooLarge(msg) => Problem::request_too_large().with_detail(msg),
            BlobError::BadKey(msg) => Problem::validation_failed(msg),
            other => {
                tracing::error!("blob error: {:?}", other);
                Problem::internal().with_detail("document upload failed")
            }
        })?;

    Ok((
        StatusCode::CREATED,
        Json(UploadResponse {
            key: format!("documents/{doc_id}"),
            bytes: uploaded_bytes,
            content_type: ct,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_has_correct_name() {
        let module = DocumentsModule::new();
        assert_eq!(module.name(), "documents");
    }

    #[test]
    fn module_requires_blob_port() {
        let module = DocumentsModule::new();
        assert_eq!(module.requires(), &[Port::Blob, Port::IdGen]);
    }

    #[test]
    fn module_from_config_no_large_blob_bytes() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, _key: &str) -> Option<String> {
                None
            }
        }
        let result = DocumentsModule::from_config(&TestConfig);
        assert!(result.is_ok());
        let module = result.unwrap();
        assert_eq!(module.max_object_bytes, DEFAULT_MAX_OBJECT_BYTES);
    }

    #[test]
    fn module_from_config_with_valid_large_blob_bytes() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, key: &str) -> Option<String> {
                if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                    Some("33554432".to_string())
                } else {
                    None
                }
            }
        }
        let result = DocumentsModule::from_config(&TestConfig);
        assert!(result.is_ok());
        let module = result.unwrap();
        assert_eq!(module.max_object_bytes, 33_554_432);
    }

    #[test]
    fn module_from_config_invalid_large_blob_bytes() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, key: &str) -> Option<String> {
                if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                    Some("not-a-number".to_string())
                } else {
                    None
                }
            }
        }
        let result = DocumentsModule::from_config(&TestConfig);
        assert!(result.is_err());
    }

    #[test]
    fn module_from_config_exceeds_harness_ceiling() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, key: &str) -> Option<String> {
                if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                    Some("99999999999999".to_string())
                } else {
                    None
                }
            }
        }
        let result = DocumentsModule::from_config(&TestConfig);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be <="));
    }

    #[test]
    fn max_blob_object_bytes_default() {
        let module = DocumentsModule::new();
        assert_eq!(module.max_blob_object_bytes(), DEFAULT_MAX_OBJECT_BYTES);
    }

    #[test]
    fn max_blob_object_bytes_from_config() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, key: &str) -> Option<String> {
                if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                    Some("20971520".to_string()) // 20 MiB
                } else {
                    None
                }
            }
        }
        let module = DocumentsModule::from_config(&TestConfig).unwrap();
        assert_eq!(module.max_blob_object_bytes(), 20_971_520);
    }

    #[test]
    fn streaming_routes_declared() {
        let module = DocumentsModule::new();
        let routes = module.streaming_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/upload");
        assert_eq!(routes[0].method, axum::http::Method::POST);
        #[allow(clippy::cast_possible_truncation)]
        {
            assert_eq!(routes[0].max_bytes, MAX_LARGE_BLOB_BYTES as usize);
        }
    }

    #[test]
    fn validate_config_success() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, _key: &str) -> Option<String> {
                None
            }
        }
        let module = DocumentsModule::new();
        let result = module.validate_config(&TestConfig);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_config_invalid_value() {
        struct TestConfig;
        impl Config for TestConfig {
            fn get(&self, key: &str) -> Option<String> {
                if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                    Some("invalid".to_string())
                } else {
                    None
                }
            }
        }
        let module = DocumentsModule::new();
        let result = module.validate_config(&TestConfig);
        assert!(result.is_err());
    }
}
