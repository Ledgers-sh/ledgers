//! Turns the path and query parameters of a GET command into its input.
//!
//! HTTP delivers every parameter as a string. The command's input schema
//! (the `schemars` JSON schema on its descriptor) says what each field is,
//! so each value is converted to that type: integers, numbers and booleans
//! are parsed, lists of strings are split on commas (or repeated), and
//! everything else stays a string. Account numbers such as `1030` and
//! external ids are strings in the schema, so they stay strings.

use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;
use std::fmt;

/// A parameter whose value does not fit the type its command field has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamError {
    /// The input field the parameter maps to.
    pub field: String,
    /// What the value should have been.
    pub reason: String,
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parameter `{}` {}", self.field, self.reason)
    }
}

/// The JSON type a parameter converts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Kept as the string it arrived as.
    Text,
    /// Parsed as a whole number.
    Integer,
    /// Parsed as a floating-point number.
    Number,
    /// `true`/`1` or `false`/`0`.
    Boolean,
    /// A list of strings: comma-separated, or repeated.
    TextList,
}

/// Maximum `$ref` hops followed while resolving one property.
const MAX_REF_DEPTH: usize = 8;

/// The kind of the input property `property` of a command whose input schema
/// is `root`. Unknown or unresolvable schemas are [`Kind::Text`].
#[must_use]
pub fn kind_of(root: &Value, property: &Value) -> Kind {
    schema_kind(root, property, 0).unwrap_or(Kind::Text)
}

/// The kind a schema converts to, ignoring `null` alternatives. `None` for
/// `null` itself and for schemas that name no usable type.
fn schema_kind(root: &Value, schema: &Value, depth: usize) -> Option<Kind> {
    if depth > MAX_REF_DEPTH {
        return None;
    }
    if let Some(target) = schema.get("$ref").and_then(Value::as_str) {
        let resolved = root.pointer(target.strip_prefix('#')?)?;
        return schema_kind(root, resolved, depth + 1);
    }
    match schema.get("type") {
        Some(Value::String(name)) => return named_kind(root, schema, name, depth),
        Some(Value::Array(names)) => {
            return names
                .iter()
                .filter_map(Value::as_str)
                .find(|name| *name != "null")
                .and_then(|name| named_kind(root, schema, name, depth));
        }
        _ => {}
    }
    ["anyOf", "oneOf"].iter().find_map(|key| {
        schema
            .get(*key)
            .and_then(Value::as_array)?
            .iter()
            .find_map(|variant| schema_kind(root, variant, depth + 1))
    })
}

fn named_kind(root: &Value, schema: &Value, name: &str, depth: usize) -> Option<Kind> {
    match name {
        "integer" => Some(Kind::Integer),
        "number" => Some(Kind::Number),
        "boolean" => Some(Kind::Boolean),
        "string" => Some(Kind::Text),
        "array" => {
            let items = schema.get("items")?;
            (schema_kind(root, items, depth + 1)? == Kind::Text).then_some(Kind::TextList)
        }
        _ => None,
    }
}

/// The kind of the field `field` of the input schema `root`. Fields the
/// schema does not name are [`Kind::Text`].
#[must_use]
pub fn field_kind(root: &Value, field: &str) -> Kind {
    let object = resolve_ref(root, root, 0);
    object
        .get("properties")
        .and_then(|properties| properties.get(field))
        .map_or(Kind::Text, |property| kind_of(root, property))
}

/// Follows `$ref` hops from `schema` to the schema they name. A reference
/// that does not resolve stays where it stopped.
fn resolve_ref<'a>(root: &'a Value, schema: &'a Value, depth: usize) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(target) if depth < MAX_REF_DEPTH => {
            match target.strip_prefix('#').and_then(|path| root.pointer(path)) {
                Some(resolved) => resolve_ref(root, resolved, depth + 1),
                None => schema,
            }
        }
        _ => schema,
    }
}

/// Converts one field's raw values to JSON. Repeated values of a list field
/// are joined; for every other kind the last value wins. An empty value of a
/// non-text field means "not given" and yields `None`.
///
/// # Errors
///
/// Returns a [`ParamError`] when a value does not parse as the field's type.
pub fn convert(field: &str, kind: Kind, values: &[&str]) -> Result<Option<Value>, ParamError> {
    let Some(&last) = values.last() else {
        return Ok(None);
    };
    let bad = |what: &str| ParamError {
        field: field.to_owned(),
        reason: format!("must be {what}, got {last:?}"),
    };
    if last.is_empty() && !matches!(kind, Kind::Text | Kind::TextList) {
        return Ok(None);
    }
    match kind {
        Kind::Text => Ok(Some(Value::String(last.to_owned()))),
        Kind::TextList => {
            let items: Vec<Value> = values
                .iter()
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_owned()))
                .collect();
            Ok((!items.is_empty()).then_some(Value::Array(items)))
        }
        Kind::Integer => last
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| last.parse::<u64>().map(Value::from))
            .map(Some)
            .map_err(|_| bad("an integer")),
        Kind::Number => last
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(|number| Some(Value::Number(number)))
            .ok_or_else(|| bad("a number")),
        Kind::Boolean => match last {
            "true" | "1" => Ok(Some(Value::Bool(true))),
            "false" | "0" => Ok(Some(Value::Bool(false))),
            _ => Err(bad("true or false")),
        },
    }
}

/// The command input for a GET request: the query parameters (repeated keys
/// kept in order) and the path parameters, each converted by the type of its
/// field in `schema`. A path parameter overrides a query parameter of the
/// same name.
///
/// # Errors
///
/// Returns the first [`ParamError`] in field-name order.
pub fn get_input(
    schema: &Value,
    query: &[(String, String)],
    path: &BTreeMap<String, String>,
) -> Result<Map<String, Value>, ParamError> {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (key, value) in query {
        grouped.entry(key).or_default().push(value);
    }
    for (key, value) in path {
        grouped.insert(key, vec![value.as_str()]);
    }

    let mut input = Map::new();
    for (field, values) in grouped {
        let kind = field_kind(schema, field);
        if let Some(value) = convert(field, kind, &values)? {
            input.insert(field.to_owned(), value);
        }
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A schema shaped like schemars output: optional integer, optional enum
    /// behind `$ref`, a string id, an optional string list and a boolean.
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {"type": ["integer", "null"], "format": "uint32", "minimum": 0},
                "status": {"anyOf": [{"$ref": "#/$defs/Status"}, {"type": "null"}]},
                "number": {"type": "string"},
                "id": {"type": "string"},
                "codes": {"type": ["array", "null"], "items": {"type": "string"}},
                "ids": {"type": "array", "items": {"type": "integer"}},
                "include_void": {"type": "boolean"},
                "ratio": {"type": "number"},
                "meta": {"type": "object"},
                "note": {"anyOf": [{"type": "string"}, {"type": "null"}]}
            },
            "$defs": {
                "Status": {"type": "string", "enum": ["draft", "posted"]}
            }
        })
    }

    fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn no_path() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn integer_field_parses_to_a_number() {
        let input = get_input(&schema(), &q(&[("limit", "100")]), &no_path()).unwrap();
        assert_eq!(input["limit"], json!(100));
    }

    #[test]
    fn large_unsigned_integer_parses() {
        let input = get_input(
            &schema(),
            &q(&[("limit", "18446744073709551615")]),
            &no_path(),
        )
        .unwrap();
        assert_eq!(input["limit"], json!(u64::MAX));
    }

    #[test]
    fn non_integer_is_refused_naming_the_field() {
        let err = get_input(&schema(), &q(&[("limit", "abc")]), &no_path()).unwrap_err();
        assert_eq!(err.field, "limit");
        assert_eq!(
            err.to_string(),
            "parameter `limit` must be an integer, got \"abc\""
        );
    }

    #[test]
    fn empty_value_of_a_non_text_field_is_not_given() {
        let input = get_input(&schema(), &q(&[("limit", ""), ("ratio", "")]), &no_path()).unwrap();
        assert!(!input.contains_key("limit"));
        assert!(!input.contains_key("ratio"));
    }

    #[test]
    fn string_field_keeps_digits_as_text() {
        let input = get_input(&schema(), &q(&[("number", "1030")]), &no_path()).unwrap();
        assert_eq!(input["number"], json!("1030"));
    }

    #[test]
    fn path_parameter_keeps_account_number_a_string() {
        let mut path = BTreeMap::new();
        path.insert("number".to_owned(), "1030".to_owned());
        let input = get_input(&schema(), &[], &path).unwrap();
        assert_eq!(input["number"], json!("1030"));
    }

    #[test]
    fn path_parameter_overrides_query_parameter() {
        let mut path = BTreeMap::new();
        path.insert("id".to_owned(), "bill-2".to_owned());
        let input = get_input(&schema(), &q(&[("id", "bill-1")]), &path).unwrap();
        assert_eq!(input["id"], json!("bill-2"));
    }

    #[test]
    fn enum_behind_ref_and_null_stays_a_string() {
        let input = get_input(&schema(), &q(&[("status", "posted")]), &no_path()).unwrap();
        assert_eq!(input["status"], json!("posted"));
    }

    #[test]
    fn list_splits_commas_and_repeats() {
        let input = get_input(
            &schema(),
            &q(&[("codes", "a, b"), ("codes", "c")]),
            &no_path(),
        )
        .unwrap();
        assert_eq!(input["codes"], json!(["a", "b", "c"]));
    }

    #[test]
    fn list_of_non_strings_is_kept_as_text() {
        // Only lists of strings are converted; other arrays stay strings.
        assert_eq!(field_kind(&schema(), "ids"), Kind::Text);
    }

    #[test]
    fn boolean_accepts_true_false_one_zero() {
        let input = get_input(&schema(), &q(&[("include_void", "1")]), &no_path()).unwrap();
        assert_eq!(input["include_void"], json!(true));
        for (raw, want) in [("true", true), ("false", false), ("0", false)] {
            let input = get_input(&schema(), &q(&[("include_void", raw)]), &no_path()).unwrap();
            assert_eq!(input["include_void"], json!(want), "raw {raw}");
        }
    }

    #[test]
    fn boolean_refuses_other_text() {
        let err = get_input(&schema(), &q(&[("include_void", "yes")]), &no_path()).unwrap_err();
        assert_eq!(err.field, "include_void");
        assert!(err.reason.contains("true or false"));
    }

    #[test]
    fn number_parses_and_refuses_nan() {
        let input = get_input(&schema(), &q(&[("ratio", "0.5")]), &no_path()).unwrap();
        assert_eq!(input["ratio"], json!(0.5));
        let err = get_input(&schema(), &q(&[("ratio", "NaN")]), &no_path()).unwrap_err();
        assert_eq!(err.field, "ratio");
    }

    #[test]
    fn unknown_and_untyped_keys_stay_strings() {
        let input = get_input(
            &schema(),
            &q(&[("unknown", "5"), ("meta", "7"), ("note", "12")]),
            &no_path(),
        )
        .unwrap();
        assert_eq!(input["unknown"], json!("5"));
        assert_eq!(input["meta"], json!("7"));
        assert_eq!(input["note"], json!("12"));
    }

    #[test]
    fn repeated_scalar_keeps_the_last_value() {
        let input =
            get_input(&schema(), &q(&[("limit", "1"), ("limit", "2")]), &no_path()).unwrap();
        assert_eq!(input["limit"], json!(2));
    }

    #[test]
    fn root_schema_behind_ref_is_followed() {
        let root = json!({
            "$ref": "#/$defs/ListInput",
            "$defs": {"ListInput": {"type": "object", "properties": {"limit": {"type": "integer"}}}}
        });
        let input = get_input(&root, &q(&[("limit", "7")]), &no_path()).unwrap();
        assert_eq!(input["limit"], json!(7));
    }

    #[test]
    fn no_schema_properties_means_all_strings() {
        let input = get_input(&json!({}), &q(&[("limit", "100")]), &no_path()).unwrap();
        assert_eq!(input["limit"], json!("100"));
    }
}
