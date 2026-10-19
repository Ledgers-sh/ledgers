//! Canonical JSON and the two hashes built on it.
//!
//! A hash over "the event" is only meaningful if there is exactly one
//! byte sequence for the event, so this module owns the encoding
//! outright rather than using `serde_json`'s: `serde_json::Map` is a
//! `BTreeMap` or an `IndexMap` depending on the `preserve_order`
//! feature, which workspace feature unification can switch on under a
//! crate that never asked, and the same event would then hash two ways.
//!
//! The rules: UTF-8, no insignificant whitespace; object keys sorted by
//! the byte order of their UTF-8 encoding; strings escaping only `"`,
//! `\` and U+0000–U+001F (`\b \t \n \f \r` by name, else `\u00xx`
//! lowercase hex), non-ASCII literal and `/` bare; numbers as plain
//! integers. That is RFC 8785 restricted to the integer-only subset —
//! every number Ledgers audits is a sequence number or a minor-unit
//! amount, and a float has no place in a ledger. Because the rules are
//! fixed, a verifier can re-serialise a parsed line and compare
//! **bytes**. docs/audit-format.md is the normative statement; this
//! module and the test vectors are pinned to it.

use crate::AuditError;
use crate::event::AuditEvent;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// Serialises `value` to its canonical JSON form.
///
/// # Errors
///
/// [`AuditError::Canonical`] for a non-integer number or a value the
/// walk cannot represent. There is no lossy fallback: an event that
/// cannot be hashed over its own bytes must not be recorded.
pub fn canonical_json(value: &Value) -> Result<String, AuditError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// Lowercase hex SHA-256 over the canonical JSON of `input` — an
/// event's `input_hash`. The input itself is never stored (it can hold
/// personal data and it can be large); the hash lets an auditor prove
/// two runs were given the same command without seeing either.
#[must_use]
pub fn input_hash(input: &Value) -> String {
    // A float in a command input is a caller bug, but a hash must still
    // fingerprint exactly what was passed, so fall back to serde's own
    // rendering rather than panicking.
    match canonical_json(input) {
        Ok(text) => sha256_hex(text.as_bytes()),
        Err(_) => sha256_hex(input.to_string().as_bytes()),
    }
}

/// Lowercase hex SHA-256 over `bytes` — the chain's one hash primitive.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` cannot fail; discarding it is by
        // design.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The chain hash of `event` appended onto `prev_hash`.
///
/// Two properties do the work. **Linkage**: `prev_hash` is inside the
/// hashed bytes, so an event cannot move to another chain position
/// without changing its own hash. **Unambiguous concatenation**:
/// `prev_hash` is a 64-hex digest or the one-character genesis `"0"`,
/// and an event's canonical JSON always opens with `{`, so
/// `prev_hash || canonical` has exactly one split.
///
/// # Errors
///
/// [`AuditError::Canonical`] when the event cannot be serialised
/// exactly — propagated rather than defaulted, so an event is never
/// hashed over bytes other than the ones stored.
pub fn event_hash(prev_hash: &str, event: &AuditEvent) -> Result<String, AuditError> {
    let canonical =
        canonical_json(&serde_json::to_value(event).map_err(|err| {
            AuditError::Canonical(format!("the event cannot be serialised: {err}"))
        })?)?;
    Ok(sha256_hex(
        [prev_hash.as_bytes(), canonical.as_bytes()]
            .concat()
            .as_slice(),
    ))
}

fn write_value(value: &Value, out: &mut String) -> Result<(), AuditError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(number, out)?,
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(fields) => {
            // Sorted by the UTF-8 byte order of the key, which for `str`
            // is `str`'s own `Ord`: a sort of the keys we hold, never
            // the map's own iteration order behind the feature flag.
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                let Some(field) = fields.get(key) else {
                    // Unreachable: `key` came from this map. Reported
                    // rather than an index panic: this crate never unwinds.
                    return Err(AuditError::Canonical(format!(
                        "object key {key:?} disappeared mid-walk"
                    )));
                };
                write_value(field, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(number: &serde_json::Number, out: &mut String) -> Result<(), AuditError> {
    if let Some(signed) = number.as_i64() {
        let _ = write!(out, "{signed}");
        return Ok(());
    }
    if let Some(unsigned) = number.as_u64() {
        let _ = write!(out, "{unsigned}");
        return Ok(());
    }
    Err(AuditError::Canonical(format!(
        "non-integer number {number} cannot be canonicalised"
    )))
}

/// `"` `\` and C0 only, two-char forms where they exist, `\u00xx`
/// lowercase otherwise, everything else literal.
fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            // Every non-ASCII character stays literal (a `Müller` is
            // six bytes, not a `\u` escape), and `/` is not escaped.
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canon(value: &Value) -> String {
        canonical_json(value).expect("canonicalisable")
    }

    #[test]
    fn keys_sort_by_byte_order_at_every_depth() {
        // Uppercase before lowercase, byte-wise. `z` before `ä`:
        // 0x7a < 0xc3.
        assert_eq!(
            canon(&json!({"b": 1, "B": 2, "a": 3, "A": 4, "ä": 5, "z": 6})),
            r#"{"A":4,"B":2,"a":3,"b":1,"z":6,"ä":5}"#
        );
        assert_eq!(
            canon(&json!({
                "outer": {"z": 1, "a": {"n": null, "m": [{"y": true, "x": false}]}},
                "alpha": [3, 1, 2]
            })),
            r#"{"alpha":[3,1,2],"outer":{"a":{"m":[{"x":false,"y":true}],"n":null},"z":1}}"#
        );
    }

    #[test]
    fn strings_escape_exactly_what_the_rule_names() {
        // The two-char forms where they exist, `\u00xx` below that, `/`
        // bare and nothing else escaped.
        assert_eq!(
            canon(&json!({"s": "a\"b\\c/d\nt\rf\u{8}g\u{c}h\ti\u{1}j"})),
            "{\"s\":\"a\\\"b\\\\c/d\\nt\\rf\\bg\\fh\\ti\\u0001j\"}"
        );
        // NUL, VT, SO, US: no two-char form, so lowercase hex.
        assert_eq!(
            canon(&json!("\u{0}\u{b}\u{e}\u{1f}")),
            "\"\\u0000\\u000b\\u000e\\u001f\""
        );
        // Non-ASCII stays literal: both spellings parse to one string,
        // so only one of them can be the canonical bytes.
        assert_eq!(
            canon(&json!({"party": "Müller GmbH", "euro": "€"})),
            "{\"euro\":\"€\",\"party\":\"Müller GmbH\"}"
        );
    }

    #[test]
    fn integers_are_plain_decimal_and_floats_are_refused() {
        assert_eq!(
            canon(&json!({"a": [1, 2], "b": {"c": true}})),
            r#"{"a":[1,2],"b":{"c":true}}"#
        );
        assert_eq!(canon(&json!(0)), "0");
        assert_eq!(canon(&json!(-1)), "-1");
        assert_eq!(canon(&json!(u64::MAX)), "18446744073709551615");
        assert!(canonical_json(&json!(1.5)).is_err());
        // 1.0 is integral in value but not in wire form: its bytes are
        // not this crate's to choose.
        assert!(canonical_json(&json!(1.0)).is_err());
    }

    #[test]
    fn the_input_hash_is_the_digest_of_the_canonical_bytes() {
        let a = json!({"amount": 100, "currency": "EUR"});
        let b = json!({"currency": "EUR", "amount": 100});
        assert_eq!(input_hash(&a), input_hash(&b), "key order is not the input");
        assert_eq!(input_hash(&a), sha256_hex(canon(&a).as_bytes()));
        assert!(
            input_hash(&a)
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(
            input_hash(&a),
            input_hash(&json!({"amount": 101, "currency": "EUR"}))
        );
    }
}
