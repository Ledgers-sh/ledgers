//! OpenAPI 3.1.0 document generation from the command registry.
//!
//! Serves OpenAPI specification at `/v1/openapi`. The module name "openapi"
//! is nested by the harness at `/v1/{name}` (cratefield-core harness.rs:442),
//! so routes within this module are relative to that prefix.

use cratefield_core::axum::{self, extract::Json, routing::get};
use cratefield_core::{Config, ConfigError, Migrations, Module, ModuleContext, Port};
use ledgers_commands::Registry;
use serde_json::{Value, json};

/// OpenAPI module serving /v1/openapi
pub struct OpenApiModule {
    doc: Value,
}

impl OpenApiModule {
    /// Create a new OpenAPI module with the given registry.
    pub fn new(registry: &Registry) -> Self {
        Self {
            doc: document(registry),
        }
    }
}

impl Module for OpenApiModule {
    fn name(&self) -> &'static str {
        "openapi"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn requires(&self) -> &'static [Port] {
        &[]
    }

    fn router(&self, _ctx: ModuleContext) -> axum::Router {
        let doc = self.doc.clone();
        axum::Router::new().route(
            "/",
            get(move || {
                let doc = doc.clone();
                async move { Json(doc) }
            }),
        )
    }

    fn migrations(&self) -> Migrations {
        Migrations::default()
    }

    fn validate_config(&self, _cfg: &dyn Config) -> Result<(), ConfigError> {
        Ok(())
    }
}

/// Generates an OpenAPI 3.1.0 document from the registry.
/// Self-contained: all $refs are resolved within the document.
pub fn document(registry: &Registry) -> Value {
    let mut paths: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut components_schemas: serde_json::Map<String, Value> = serde_json::Map::new();

    // Build paths and collect schemas
    for descriptor in registry.iter() {
        let path = &descriptor.http_path;
        let method = descriptor.http_method.to_lowercase();

        // Rewrite refs in input/output schemas and collect definitions
        let input_schema =
            rewrite_refs_and_collect(&descriptor.input_schema, &mut components_schemas);
        let output_schema =
            rewrite_refs_and_collect(&descriptor.output_schema, &mut components_schemas);

        // Operation ID: replace dots with underscores
        let operation_id = descriptor.name.replace('.', "_");

        // Build path operation
        let mut operation = serde_json::Map::new();
        operation.insert("operationId".to_string(), json!(operation_id));
        operation.insert(
            "description".to_string(),
            json!(format!("Execute command: {}", descriptor.name)),
        );

        // Security: bearer token
        operation.insert("security".to_string(), json!([{ "bearerAuth": [] }]));

        // Tags for grouping
        let domain = path
            .strip_prefix("/v1/")
            .and_then(|p| p.split('/').next())
            .unwrap_or("default");
        operation.insert("tags".to_string(), json!([domain]));

        // Request body (for POST)
        if method == "post" {
            let mut request_body = serde_json::Map::new();
            request_body.insert("required".to_string(), json!(true));
            let mut content = serde_json::Map::new();
            content.insert(
                "application/json".to_string(),
                json!({ "schema": input_schema }),
            );
            request_body.insert("content".to_string(), Value::Object(content));
            operation.insert("requestBody".to_string(), Value::Object(request_body));
        }

        // Response: 200
        let mut response_200 = serde_json::Map::new();
        response_200.insert("description".to_string(), json!("Successful response"));
        let mut response_content = serde_json::Map::new();
        response_content.insert(
            "application/json".to_string(),
            json!({ "schema": output_schema }),
        );
        response_200.insert("content".to_string(), Value::Object(response_content));

        // Response: 501 if Unwired
        let mut responses = serde_json::Map::new();
        responses.insert("200".to_string(), Value::Object(response_200));
        responses.insert(
            "501".to_string(),
            json!({
                "description": "Not yet implemented",
                "content": {
                    "application/problem+json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string" },
                                "title": { "type": "string" },
                                "status": { "type": "integer" },
                                "detail": { "type": "string" },
                                "instance": { "type": "string" }
                            }
                        }
                    }
                }
            }),
        );

        operation.insert("responses".to_string(), Value::Object(responses));

        // Add x-ledgers extensions
        operation.insert("x-ledgers-tier".to_string(), json!(descriptor.tier));
        if descriptor.human_only {
            operation.insert("x-ledgers-human-only".to_string(), json!(true));
        }

        // Add to paths
        let path_item = paths
            .entry(path.clone())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));

        if let Value::Object(item) = path_item {
            item.insert(method, Value::Object(operation));
        }
    }

    // Components schema definitions
    let mut components = serde_json::Map::new();
    if !components_schemas.is_empty() {
        components.insert("schemas".to_string(), Value::Object(components_schemas));
    }

    // Security scheme
    let mut security_schemes = serde_json::Map::new();
    let mut bearer_scheme = serde_json::Map::new();
    bearer_scheme.insert("type".to_string(), json!("http"));
    bearer_scheme.insert("scheme".to_string(), json!("bearer"));
    security_schemes.insert("bearerAuth".to_string(), Value::Object(bearer_scheme));
    components.insert(
        "securitySchemes".to_string(),
        Value::Object(security_schemes),
    );

    // Build OpenAPI document
    let mut doc = serde_json::Map::new();
    doc.insert("openapi".to_string(), json!("3.1.0"));

    let mut info = serde_json::Map::new();
    info.insert("title".to_string(), json!("Ledgers"));
    info.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
    doc.insert("info".to_string(), Value::Object(info));

    doc.insert("paths".to_string(), Value::Object(paths));

    if !components.is_empty() {
        doc.insert("components".to_string(), Value::Object(components));
    }

    Value::Object(doc)
}

/// Recursively rewrites $refs to be self-contained and collects definitions.
/// Moves $defs from schemas into the top-level components.
fn rewrite_refs_and_collect(
    schema: &Value,
    components_schemas: &mut serde_json::Map<String, Value>,
) -> Value {
    match schema {
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();

            // If this object has $defs, move them to components
            if let Some(Value::Object(defs)) = map.get("$defs") {
                for (name, def) in defs {
                    let collected = rewrite_refs_and_collect(def, components_schemas);
                    components_schemas.insert(name.clone(), collected);
                }
                // Don't include $defs in the rewritten schema
            } else if let Some(Value::Object(defs)) = map.get("definitions") {
                // Also handle "definitions" (alternate name)
                for (name, def) in defs {
                    let collected = rewrite_refs_and_collect(def, components_schemas);
                    components_schemas.insert(name.clone(), collected);
                }
            }

            for (k, v) in map {
                if k == "$defs" || k == "definitions" {
                    continue; // Skip, already processed
                }
                if k == "$ref" {
                    // Rewrite $ref from #/$defs/Foo to #/components/schemas/Foo
                    if let Value::String(ref_str) = v {
                        let rewritten = if ref_str.starts_with("#/$defs/") {
                            ref_str.replace("#/$defs/", "#/components/schemas/")
                        } else if ref_str.starts_with("#/definitions/") {
                            ref_str.replace("#/definitions/", "#/components/schemas/")
                        } else {
                            ref_str.clone()
                        };
                        new_map.insert(k.clone(), json!(rewritten));
                    } else {
                        new_map.insert(k.clone(), v.clone());
                    }
                } else {
                    new_map.insert(k.clone(), rewrite_refs_and_collect(v, components_schemas));
                }
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| rewrite_refs_and_collect(v, components_schemas))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_has_openapi_version() {
        let registry = Registry::new();
        let doc = document(&registry);
        assert_eq!(doc["openapi"], "3.1.0");
    }

    #[test]
    fn document_has_info() {
        let registry = Registry::new();
        let doc = document(&registry);
        assert_eq!(doc["info"]["title"], "Ledgers");
    }

    #[test]
    fn rewrite_defs_to_components() {
        let schema = json!({
            "type": "object",
            "$defs": {
                "Amount": {
                    "type": "integer"
                }
            },
            "properties": {
                "value": { "$ref": "#/$defs/Amount" }
            }
        });

        let mut components = serde_json::Map::new();
        let rewritten = rewrite_refs_and_collect(&schema, &mut components);

        // Check $defs are collected
        assert!(components.contains_key("Amount"));
        // Check $defs are removed from schema
        assert!(!rewritten.get("$defs").is_some());
        // Check $ref is rewritten
        assert_eq!(
            rewritten["properties"]["value"]["$ref"],
            "#/components/schemas/Amount"
        );
    }
}
