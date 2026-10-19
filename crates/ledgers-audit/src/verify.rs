//! Exporting the chain as JSON Lines and the offline verifier that
//! checks one.
//!
//! An export is not a convenience format a tolerant reader interprets:
//! because every line is *canonical JSON*, a line's bytes are the only
//! thing that has to be true. The verifier parses each line,
//! re-serialises it and requires the two byte sequences to be equal, so
//! re-indenting a line, re-escaping a `Müller`, reordering keys and
//! changing one space fail the same check as rewriting an amount — it
//! never has to decide which spelling was "meant".
//!
//! Per line, stopping at the **first** break (a chain is only
//! trustworthy up to where it was last checked): valid UTF-8; valid
//! JSON; the bytes are exactly their own canonical form; the line is a
//! line, strictly; `seq` is 1 on the first line and the previous
//! line's `seq + 1` after; `company_id` is the same throughout;
//! `prev_hash` is `GENESIS_HASH` on line 1 and the previous line's
//! `hash` after; and `hash == event_hash(prev_hash, event)`.
//!
//! Because the hash covers `prev_hash` and the event, a rewritten event
//! needs a rewritten hash *and* every hash after it; with an anchor
//! published externally even a wholesale rewrite is detectable. An
//! empty export is valid — head `seq` 0 on the genesis hash — and
//! indistinguishable from one that lost everything, hence the anchors.

use crate::event::AuditEvent;
use crate::{AuditError, canonical_json, event_hash};
use cratefield_core::{Database, Statement};
use ledgers_db::chain::GENESIS_HASH;
use serde::Serialize;
use std::fmt;

/// One export line: the event with the two hashes stored beside it.
#[derive(Debug, Serialize)]
struct Line<'a> {
    event: &'a AuditEvent,
    hash: &'a str,
    prev_hash: &'a str,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLine {
    event: AuditEvent,
    hash: String,
    prev_hash: String,
}

/// Exports `company_id`'s whole audit chain as JSON Lines: one line
/// per event in `seq` order, each exactly the canonical JSON of
/// `{"event":…,"hash":…,"prev_hash":…}` and `\n`-terminated. The file
/// is complete by construction — a partial export is one the verifier
/// rejects, since its first line's `seq` would not be 1 — and an empty
/// chain is an empty file, which [`verify`] accepts.
///
/// # Errors
///
/// [`AuditError::Database`] when the query fails; [`AuditError::Malformed`]
/// when a row is not the shape the table declares or its payload is not
/// a parseable event, which would mean the row was not written by this
/// crate's canonical form.
pub async fn export(db: &dyn Database, company_id: &str) -> Result<Vec<u8>, AuditError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT seq, prev_hash, hash, payload FROM audit_event\n\
             WHERE company_id = ? ORDER BY seq"
                .to_owned(),
            vec![sea_query::Value::from(company_id)],
        ))
        .await?;

    let mut out: Vec<u8> = Vec::new();
    for row in &rows.rows {
        // `seq` is read first so a malformed row can name itself.
        let seq = row.get::<i64>("seq").ok_or(AuditError::Malformed {
            seq: None,
            detail: "column seq missing or not an integer".to_owned(),
        })?;
        let text = |column: &str| {
            row.get::<String>(column)
                .ok_or_else(|| AuditError::Malformed {
                    seq: Some(seq),
                    detail: format!("column {column} missing or not text"),
                })
        };
        let event: AuditEvent =
            serde_json::from_str(&text("payload")?).map_err(|err| AuditError::Malformed {
                seq: Some(seq),
                detail: format!("payload is not an audit event: {err}"),
            })?;
        let line = Line {
            event: &event,
            hash: &text("hash")?,
            prev_hash: &text("prev_hash")?,
        };
        let value = serde_json::to_value(line).map_err(|err| {
            AuditError::Canonical(format!("the export line cannot be serialised: {err}"))
        })?;
        out.extend_from_slice(canonical_json(&value)?.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

/// Exports `company_id`'s chain and verifies it in one step — the
/// in-process equivalent of exporting a file and running [`verify`] on
/// it. A disagreement would mean the table and the file tell different
/// stories.
///
/// # Errors
///
/// [`AuditError`] for the export, and [`AuditError::Break`] for a chain
/// that does not verify — the same break a file would produce, from the
/// same bytes.
pub async fn verify_db(db: &dyn Database, company_id: &str) -> Result<Verified, AuditError> {
    let bytes = export(db, company_id).await?;
    Ok(verify(&bytes)?)
}

/// A verified export: how many events, and the head it ends at.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Verified {
    /// The number of events, all of them chained.
    pub events: usize,
    /// The last event's `seq`, or 0 for an empty export.
    pub head_seq: i64,
    /// The last event's `hash`, or [`GENESIS_HASH`] for an empty one.
    pub head_hash: String,
}

/// What failed, in the verifier's own words.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BreakKind {
    /// The line is not valid UTF-8.
    NotUtf8,
    /// The line is not valid JSON.
    NotJson,
    /// The bytes differ from the line's own canonical re-serialisation.
    NotCanonical,
    /// JSON but not a line shape: an unknown or missing field, or an
    /// unparseable event. What it does not understand it refuses.
    NotALine,
    /// `seq` is not 1 first, or not the previous line's `seq + 1`.
    SeqGap {
        /// The `seq` this line must carry.
        expected: i64,
        /// The `seq` it carried.
        found: i64,
    },
    /// `company_id` differs from the first line's.
    ForeignCompany {
        /// The company the export started with.
        expected: String,
        /// The company this line names.
        found: String,
    },
    /// The first line does not link to the genesis hash.
    BadGenesis {
        /// The `prev_hash` the first line carried.
        found: String,
    },
    /// `prev_hash` is not the previous line's `hash`: out of order, or
    /// a link was cut.
    BrokenLink {
        /// The `prev_hash` this line must carry.
        expected: String,
    },
    /// `hash` is not the hash of this event on this `prev_hash`.
    HashMismatch,
    /// The file does not end in a newline: the export is truncated.
    MissingFinalNewline,
}

impl fmt::Display for BreakKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => write!(f, "not valid UTF-8"),
            Self::NotJson => write!(f, "not valid JSON"),
            Self::NotCanonical => write!(
                f,
                "line is not canonical JSON (whitespace, key order or escaping changed)"
            ),
            Self::NotALine => write!(f, "not a valid audit line (unknown or missing field)"),
            Self::SeqGap { expected, found } => {
                write!(f, "seq gap: expected {expected}, found {found}")
            }
            Self::ForeignCompany { expected, found } => {
                write!(f, "company mismatch: expected {expected}, found {found}")
            }
            Self::BadGenesis { found } => write!(
                f,
                "first event does not link to the genesis hash {GENESIS_HASH} (prev_hash {found})"
            ),
            Self::BrokenLink { expected } => {
                write!(f, "prev_hash is not the previous event's hash ({expected})")
            }
            Self::HashMismatch => write!(f, "hash mismatch: the event does not match its hash"),
            Self::MissingFinalNewline => write!(f, "the last line is not newline-terminated"),
        }
    }
}

/// Where an export stops being true: the first break, named as
/// precisely as the bytes allow.
///
/// `event_id` is the offending event's id when the line parsed far
/// enough to have one, otherwise recovered by scanning the raw bytes
/// for the first `"id":"`. That fallback is why no field inside
/// [`crate::Actor`] is named exactly `id`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Break {
    /// The 1-based line number in the export.
    pub line: usize,
    /// The `seq` on that line, when it was readable.
    pub seq: Option<i64>,
    /// The event's id, recovered when possible.
    pub event_id: Option<String>,
    /// What failed.
    pub kind: BreakKind,
}

impl fmt::Display for Break {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}", self.line)?;
        if let Some(seq) = self.seq {
            write!(f, " (seq {seq}")?;
            if let Some(id) = &self.event_id {
                write!(f, ", event {id}")?;
            }
            write!(f, ")")?;
        } else if let Some(id) = &self.event_id {
            write!(f, " (event {id})")?;
        }
        write!(f, ": {}", self.kind)
    }
}

impl std::error::Error for Break {}

/// Verifies an export byte for byte, stopping at the first break.
///
/// # Errors
///
/// The [`Break`] describing the first untrue line, with everything
/// before it verified.
pub fn verify(export: &[u8]) -> Result<Verified, Break> {
    if export.is_empty() {
        return Ok(Verified {
            events: 0,
            head_seq: 0,
            head_hash: GENESIS_HASH.to_owned(),
        });
    }

    // Without a final `\n` the last line is incomplete, and
    // "incomplete" is a break, not a detail.
    if !export.ends_with(b"\n") {
        let body = body_after_last_newline(export);
        return Err(Break {
            line: export.split(|byte| *byte == b'\n').count(),
            seq: scan_seq(body),
            event_id: scan_event_id(body),
            kind: BreakKind::MissingFinalNewline,
        });
    }

    let mut walked = Walked::default();
    for (index, raw) in export.split(|byte| *byte == b'\n').enumerate() {
        // `split` on a trailing `\n` yields one empty tail: the last
        // line's terminator, not a line of its own.
        if index > 0 && raw.is_empty() {
            break;
        }
        if let Some(break_) = check_line(raw, index + 1, &walked) {
            return Err(break_);
        }
        walked.advance(raw);
    }
    Ok(Verified {
        events: walked.events,
        head_seq: walked.seq,
        head_hash: walked.hash,
    })
}

/// The chain state a verified line leaves behind.
#[derive(Debug)]
struct Walked {
    events: usize,
    seq: i64,
    /// The last verified line's `hash`; genesis before the first.
    hash: String,
    /// The company the first line named; every later line must agree.
    company: Option<String>,
}

impl Default for Walked {
    fn default() -> Self {
        Self {
            events: 0,
            seq: 0,
            hash: GENESIS_HASH.to_owned(),
            company: None,
        }
    }
}

impl Walked {
    /// Records that `raw` verified: the next line must link to it.
    fn advance(&mut self, raw: &[u8]) {
        let line: StoredLine =
            serde_json::from_slice(raw).expect("a line that verified deserialises again");
        self.events += 1;
        self.seq = line.event.seq;
        self.hash = line.hash;
        if self.company.is_none() {
            self.company = Some(line.event.company_id.clone());
        }
    }
}

/// Checks one line against the chain walked so far, returning the first
/// thing untrue about it, or `None` when it verifies. The order is the
/// module docs' list: byte equality comes before the meaning checks,
/// which are about what the line *says*.
fn check_line(raw: &[u8], number: usize, walked: &Walked) -> Option<Break> {
    let named = |kind: BreakKind| Break {
        line: number,
        seq: None,
        event_id: scan_event_id(raw),
        kind,
    };

    // 1. The line is UTF-8 and JSON.
    let Ok(text) = std::str::from_utf8(raw) else {
        return Some(named(BreakKind::NotUtf8));
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Some(named(BreakKind::NotJson));
    };

    // 2. The bytes are exactly their own canonical form.
    if canonical_json(&value).ok().as_deref() != Some(text) {
        return Some(Break {
            seq: value
                .get("event")
                .and_then(|event| event.get("seq"))
                .and_then(serde_json::Value::as_i64),
            ..named(BreakKind::NotCanonical)
        });
    }

    // 3. The line is a line, strictly.
    let Ok(line) = serde_json::from_value::<StoredLine>(value) else {
        return Some(named(BreakKind::NotALine));
    };
    let describe = |kind: BreakKind| Break {
        line: number,
        seq: Some(line.event.seq),
        event_id: Some(line.event.id.clone()),
        kind,
    };

    let expected_seq = walked.seq + 1;
    if line.event.seq != expected_seq {
        return Some(describe(BreakKind::SeqGap {
            expected: expected_seq,
            found: line.event.seq,
        }));
    }

    // One company's chain: another's events cannot be spliced in,
    // however well they are hashed.
    if let Some(first) = &walked.company
        && *first != line.event.company_id
    {
        return Some(describe(BreakKind::ForeignCompany {
            expected: first.clone(),
            found: line.event.company_id.clone(),
        }));
    }

    // Linked: genesis on the first line, the previous hash after.
    if line.prev_hash != walked.hash {
        return Some(if walked.events == 0 {
            describe(BreakKind::BadGenesis {
                found: line.prev_hash.clone(),
            })
        } else {
            describe(BreakKind::BrokenLink {
                expected: walked.hash.clone(),
            })
        });
    }

    // The hash covers this event on this prev_hash.
    match event_hash(&line.prev_hash, &line.event) {
        Ok(computed) if computed == line.hash => None,
        Ok(_) => Some(describe(BreakKind::HashMismatch)),
        // The event could not be re-serialised at all, so the chain
        // stores bytes this verifier cannot reproduce: the same "not
        // canonical" finding, not a mismatch between hashes.
        Err(_) => Some(describe(BreakKind::NotCanonical)),
    }
}

fn body_after_last_newline(export: &[u8]) -> &[u8] {
    match export.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => &export[index + 1..],
        None => export,
    }
}

/// The `seq` recoverable from raw bytes, for lines too broken to parse.
fn scan_seq(raw: &[u8]) -> Option<i64> {
    let after = bytes_after(raw, b"\"seq\":")?;
    after
        .iter()
        .take_while(|byte| byte.is_ascii_digit() || **byte == b'-')
        .map(|byte| *byte as char)
        .collect::<String>()
        .parse()
        .ok()
}

/// The event id recoverable from raw bytes: the first `"id":"` and the
/// bytes up to the next `"`. The first occurrence in a canonical line
/// is the event's own id — keys sort, so `approval_id`, `company_id`
/// and `request_id` cannot match the marker.
fn scan_event_id(raw: &[u8]) -> Option<String> {
    let marker = b"\"id\":\"";
    let start = raw
        .windows(marker.len())
        .position(|window| window == marker)
        .map(|index| index + marker.len())?;
    let rest = &raw[start..];
    let end = rest.iter().position(|byte| *byte == b'"')?;
    std::str::from_utf8(&rest[..end]).ok().map(str::to_owned)
}

fn bytes_after<'a>(raw: &'a [u8], needle: &[u8]) -> Option<&'a [u8]> {
    let index = raw.windows(needle.len()).position(|w| w == needle)?;
    Some(&raw[index + needle.len()..])
}
