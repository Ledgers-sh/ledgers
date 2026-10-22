//! Maps a registry command plus parsed arguments to an HTTP request.
//!
//! This module is pure: it builds the method, path, query and body from a
//! [`CommandDescriptor`] and an [`Invocation`], and does no I/O. The HTTP layer
//! in [`crate::http`] sends the result.
//!
//! - Path placeholders (`{id}`, `{external_id}`, ...) are filled from the
//!   positional arguments and percent-encoded.
//! - `GET` commands send the remaining input as query parameters.
//! - Other methods send the input as a JSON body; path values are included in
//!   the body too, so the server sees the same object as its path-bound input.
//! - `dry_run: true` is sent only when `--dry-run` was given. The server
//!   strips it (the same convention the MCP tool surface uses).

use crate::error::{CliError, Result};
use ledgers_commands::CommandDescriptor;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Parsed arguments for one command call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Invocation {
    /// Values for the `{placeholder}` segments of the HTTP path.
    pub path_params: BTreeMap<String, String>,
    /// The command input object.
    pub input: Map<String, Value>,
    /// Whether `--dry-run` was given.
    pub dry_run: bool,
}

/// A fully formed HTTP request, ready to send.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRequest {
    /// The HTTP method, `GET` or `POST`.
    pub method: String,
    /// The path with placeholders filled in, e.g. `/v1/entries/E-1/reverse`.
    pub path: String,
    /// Query parameters (GET only).
    pub query: Vec<(String, String)>,
    /// The JSON body (non-GET only).
    pub body: Option<Value>,
}

/// The `{placeholder}` names in an HTTP path template, in order.
#[must_use]
pub fn path_param_names(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        names.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    names
}

/// Fills the `{placeholder}` segments of a path template.
///
/// # Errors
/// Returns [`CliError::InvalidInput`] if a placeholder has no value.
pub fn expand_path(template: &str, params: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| CliError::Internal(format!("malformed path template {template}")))?;
        let name = &after[..end];
        let value = params
            .get(name)
            .ok_or_else(|| CliError::InvalidInput(format!("missing path argument {name}")))?;
        out.push_str(&encode_segment(value));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Percent-encodes one path segment (RFC 3986 unreserved characters are kept).
fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The properties of an input schema, sorted by name.
#[must_use]
pub fn schema_properties(schema: &Value) -> Vec<(String, Value)> {
    let mut props: Vec<(String, Value)> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    props.sort_by(|a, b| a.0.cmp(&b.0));
    props
}

/// The names of the required input fields, in schema order.
#[must_use]
pub fn required_fields(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON Schema type of a property, skipping `null` in nullable unions.
/// Returns `"any"` when the schema does not name one.
#[must_use]
pub fn property_type(prop: &Value) -> &str {
    match prop.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("any"),
        _ => "any",
    }
}

/// Builds the HTTP request for a command.
///
/// # Errors
/// Returns [`CliError::InvalidInput`] for a missing path argument, a missing
/// required field, or a non-scalar query value on a GET command.
pub fn plan(descriptor: &CommandDescriptor, inv: &Invocation) -> Result<PlannedRequest> {
    let path = expand_path(&descriptor.http_path, &inv.path_params)?;

    for field in required_fields(&descriptor.input_schema) {
        if !inv.input.contains_key(&field) && !inv.path_params.contains_key(&field) {
            return Err(CliError::InvalidInput(format!(
                "missing required field --{}",
                field.replace('_', "-")
            )));
        }
    }

    if descriptor.http_method == "GET" {
        let mut query = Vec::new();
        for (key, value) in &inv.input {
            match value {
                Value::Null => {}
                Value::String(s) => query.push((key.clone(), s.clone())),
                Value::Number(_) | Value::Bool(_) => query.push((key.clone(), value.to_string())),
                Value::Array(_) | Value::Object(_) => {
                    return Err(CliError::InvalidInput(format!(
                        "field {key} must be a scalar for a GET command"
                    )));
                }
            }
        }
        if inv.dry_run {
            query.push(("dry_run".to_string(), "true".to_string()));
        }
        return Ok(PlannedRequest {
            method: descriptor.http_method.clone(),
            path,
            query,
            body: None,
        });
    }

    let mut body = inv.input.clone();
    for (key, value) in &inv.path_params {
        body.insert(key.clone(), Value::String(value.clone()));
    }
    if inv.dry_run {
        body.insert("dry_run".to_string(), Value::Bool(true));
    }
    Ok(PlannedRequest {
        method: descriptor.http_method.clone(),
        path,
        query: Vec::new(),
        body: Some(Value::Object(body)),
    })
}
