//! The acceptance criterion, exhaustively: **changing any byte of any
//! event makes `verify` report that event's id**, plus one case per
//! [`BreakKind`] so a reader of the kinds can match them to what
//! actually happens.
//!
//! "Any byte" is the word doing the work. A hash chain is normally
//! demonstrated by flipping one digit of one amount; that shows the
//! digest works, not that *every* byte is covered. So this walks every
//! line of a four-event export and, within each line, every byte
//! position — a few thousand single-byte mutations — and requires for
//! each that verification fails, that the break is on **that** line,
//! and that it names the **original** event.
//!
//! Bytes *inside* the `"id":"…"` token are exempt from the
//! id-equality requirement: changing one changes the token the verifier
//! reads, so demanding the original would demand a contradiction. Those
//! positions still have to break the line, which is the coverage that
//! matters. The same fallback is why the actor carries no field named
//! `id` (see [`ledgers_audit::Actor`]).

use ledgers_audit::{
    Actor, AuditContext, AuditEvent, AuditRecord, BreakKind, Outcome, canonical_json, event_hash,
    verify,
};
use serde_json::{Value, json};

const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// The canonical export of a four-event chain in memory: the exact
/// bytes `export` would write, without a database. Event `n`
/// (0-based) is `seq` `n + 1`.
fn export_bytes() -> (Vec<u8>, Vec<AuditEvent>) {
    let (mut bytes, mut events) = (Vec::new(), Vec::new());
    let mut prev_hash = ledgers_db::chain::GENESIS_HASH.to_owned();

    for index in 0..4_usize {
        let context = AuditContext::new(
            Actor::Person {
                person_id: concat!("usr_01HQZX9K3", "M7VN2P4R6T8W0", "Y1B3").to_owned(),
            },
            format!("req_01HQZX9K3M7VN2P4R6T8W0Y1B{index}"),
            format!("01HQZX9K3M7VN2P4R6T8W0Y1B3{index}"),
        );
        let record = AuditRecord::from_input(
            "journal.post_entry",
            &json!({"n": index, "amount_minor": 100 * index}),
            Outcome::Allowed,
            vec![COMPANY_ID.to_owned()],
            None,
        );
        let event = context.event(
            COMPANY_ID,
            i64::try_from(index).expect("small") + 1,
            "2026-01-15T09:30:00Z",
            &record,
        );
        let hash = event_hash(&prev_hash, &event).expect("hashed");
        let line = json!({
            "event": serde_json::to_value(&event).expect("a value"),
            "hash": hash,
            "prev_hash": prev_hash,
        });
        bytes.extend_from_slice(canonical_json(&line).expect("canonical").as_bytes());
        bytes.push(b'\n');
        events.push(event);
        prev_hash = hash;
    }
    (bytes, events)
}

/// The line bodies, and the same lines in `order`, re-terminated.
fn bodies(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect()
}

fn arrange(bodies: &[&[u8]], order: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    for index in order {
        out.extend_from_slice(bodies[*index]);
        out.push(b'\n');
    }
    out
}

/// Line `index`, edited at the top level by `edit`, canonicalised back,
/// and returned with the rest of the chain after it.
fn rewrite(bytes: &[u8], index: usize, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let bodies = bodies(bytes);
    let mut value: Value = serde_json::from_slice(bodies[index]).expect("a line is JSON");
    edit(&mut value);
    let tail: Vec<usize> = (0..bodies.len() - index - 1).collect();
    let mut out = arrange(&bodies[..index], &(0..index).collect::<Vec<_>>());
    out.extend_from_slice(canonical_json(&value).expect("canonical").as_bytes());
    out.push(b'\n');
    out.extend(arrange(&bodies[index + 1..], &tail));
    out
}

/// Line `index`'s stored hash, read back out of the export.
fn hash_of(bytes: &[u8], index: usize) -> String {
    serde_json::from_slice::<Value>(bodies(bytes)[index]).expect("JSON")["hash"]
        .as_str()
        .expect("a hash")
        .to_owned()
}

/// The byte range of the line's whole `"id":"<value>"` **token**,
/// quotes included: changing a delimiting quote changes the token the
/// verifier reads, exactly as changing a character of the value does.
fn id_span(line: &[u8]) -> Option<std::ops::Range<usize>> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let token = format!("\"id\":\"{}\"", value["event"]["id"].as_str()?);
    let haystack = std::str::from_utf8(line).ok()?;
    let start = haystack.find(&token)?;
    Some(start..start + token.len())
}

/// A replacement byte for `original`: different, and never a newline
/// (which would shorten the file and change which line we are on).
fn replacement(original: u8) -> u8 {
    match original {
        b'\n' | b' ' => b'X',
        other => other ^ 0x01,
    }
}

#[test]
fn an_untampered_export_verifies_to_the_last_hash() {
    let empty = verify(b"").expect("an empty chain is an intact empty chain");
    assert_eq!(
        (empty.events, empty.head_seq, empty.head_hash),
        (0, 0, ledgers_db::chain::GENESIS_HASH.to_owned())
    );

    let (bytes, events) = export_bytes();
    let verified = verify(&bytes).expect("the export verifies");
    assert_eq!((verified.events, verified.head_seq), (4, 4));
    let mut hash = ledgers_db::chain::GENESIS_HASH.to_owned();
    for event in &events {
        hash = event_hash(&hash, event).expect("hashed");
    }
    assert_eq!(verified.head_hash, hash);
    // Four lines, so five newline-separated pieces: the last line's
    // terminator leaves one empty tail.
    assert_eq!(bytes.split(|byte| *byte == b'\n').count(), 5);
}

#[test]
fn changing_any_byte_of_any_event_breaks_that_event() {
    let (bytes, events) = export_bytes();
    let mut offsets = Vec::new();
    let mut cursor = 0;
    for line in bodies(&bytes) {
        offsets.push((cursor, line.len()));
        cursor += line.len() + 1;
    }

    let mut mutations = 0_usize;
    for (index, (start, len)) in offsets.iter().enumerate() {
        let span = id_span(&bytes[*start..*start + *len]).expect("every line has an id");
        let original_id = events[index].id.clone();

        for position in 0..*len {
            let mut tampered = bytes.clone();
            let original = tampered[start + position];
            tampered[start + position] = replacement(original);
            mutations += 1;

            let break_ = verify(&tampered).expect_err("a changed byte must break the chain");
            assert_eq!(
                break_.line,
                index + 1,
                "a byte changed on line {} (position {position}) must break THAT line",
                index + 1
            );
            if span.contains(&position) {
                // Inside the `"id":"…"` token: the token itself
                // changed, so only the line is required.
                continue;
            }
            assert_eq!(
                break_.event_id.as_deref(),
                Some(original_id.as_str()),
                "byte {position} (0x{original:02x} -> 0x{:02x}) of line {} is outside the id token, \
                 so the break must name the original event; kind {:?}",
                replacement(original),
                index + 1,
                break_.kind
            );
        }
    }
    // The point of the test is its breadth: a few thousand mutations,
    // not one. If the walk broke, it would silently cover nothing.
    assert!(
        mutations > 1_000,
        "only {mutations} single-byte mutations were exercised — the walk itself broke, and the \
         coverage claim would be vacuous"
    );
}

#[test]
fn every_break_kind_is_reachable() {
    let (bytes, _) = export_bytes();
    let lines = bodies(&bytes);
    let sixty_four = "f".repeat(64);

    // Whitespace that changes the bytes without changing the value: a
    // space after the opening brace, on one line.
    let reindented = {
        let text = std::str::from_utf8(lines[0]).expect("UTF-8");
        let mut out = format!("{{ {}", &text[1..]).into_bytes();
        out.push(b'\n');
        out.extend(arrange(&lines, &[1, 2, 3]));
        out
    };

    // A correctly re-hashed event of another company: linked, hashed
    // and canonical, so only the constant-company check can refuse it.
    // That is the splice a chain's own hashes cannot catch.
    let mut foreign: Value = serde_json::from_slice(lines[1]).expect("a line is JSON");
    foreign["event"]["company_id"] = json!("01OTHERCOMPANY000000000000A");
    let event: AuditEvent = serde_json::from_value(foreign["event"].clone()).expect("an event");
    let prev = hash_of(&bytes, 0);
    foreign["hash"] = json!(event_hash(&prev, &event).expect("hashed"));
    let mut spliced = lines[0].to_vec();
    spliced.push(b'\n');
    spliced.extend_from_slice(canonical_json(&foreign).expect("canonical").as_bytes());
    spliced.push(b'\n');
    spliced.extend(arrange(&lines, &[2, 3]));

    let cases: Vec<(Vec<u8>, BreakKind)> = vec![
        (vec![0xff, 0xfe, b'\n'], BreakKind::NotUtf8),
        (b"{not json\n".to_vec(), BreakKind::NotJson),
        (reindented, BreakKind::NotCanonical),
        (
            rewrite(&bytes, 0, |line| line["extra"] = json!(true)),
            BreakKind::NotALine,
        ),
        (
            arrange(&lines, &[0, 2, 1, 3]),
            BreakKind::SeqGap {
                expected: 2,
                found: 3,
            },
        ),
        (
            spliced,
            BreakKind::ForeignCompany {
                expected: COMPANY_ID.to_owned(),
                found: "01OTHERCOMPANY000000000000A".to_owned(),
            },
        ),
        (
            rewrite(&bytes, 0, |line| line["prev_hash"] = json!(sixty_four)),
            BreakKind::BadGenesis {
                found: sixty_four.clone(),
            },
        ),
        (
            rewrite(&bytes, 1, |line| line["prev_hash"] = json!(sixty_four)),
            BreakKind::BrokenLink {
                expected: hash_of(&bytes, 0),
            },
        ),
        (
            rewrite(&bytes, 0, |line| line["hash"] = json!(sixty_four)),
            BreakKind::HashMismatch,
        ),
        (
            bytes[..bytes.len() - 1].to_vec(),
            BreakKind::MissingFinalNewline,
        ),
    ];

    for (export, kind) in cases {
        let break_ = verify(&export).expect_err("this export is broken");
        assert_eq!(break_.kind, kind, "for {break_:?}");
        // The Display is what the CLI prints, and it is the only place
        // a line is rendered.
        assert!(
            break_.to_string().starts_with("line "),
            "a break must name its line: {break_}"
        );
    }
}

#[test]
fn a_deleted_swapped_or_duplicated_line_breaks_the_chain() {
    let (bytes, _) = export_bytes();
    let lines = bodies(&bytes);

    // Delete line 2: seq jumps 1 -> 3.
    let break_ = verify(&arrange(&lines, &[0, 2, 3])).expect_err("a deleted line breaks");
    assert_eq!(
        break_.kind,
        BreakKind::SeqGap {
            expected: 2,
            found: 3
        }
    );
    // Duplicate line 2: seq repeats.
    let break_ = verify(&arrange(&lines, &[0, 1, 1, 2, 3])).expect_err("a duplicate breaks");
    assert_eq!(
        break_.kind,
        BreakKind::SeqGap {
            expected: 3,
            found: 2
        }
    );
    // Swap an adjacent pair: the link, not the seq, is what objects.
    let break_ = verify(&arrange(&lines, &[0, 2, 1, 3])).expect_err("swapped lines break");
    assert_eq!(break_.line, 2);

    // A missing terminator is an incomplete line, by the format's own
    // rule that every line ends there.
    let break_ = verify(&bytes[..bytes.len() - 1]).expect_err("a truncated export breaks");
    assert_eq!(
        (break_.line, break_.kind),
        (4, BreakKind::MissingFinalNewline)
    );
}

#[test]
fn a_whole_chain_rewritten_consistently_still_cannot_match_an_anchor() {
    // The one attack the chain's own hashes do not stop: rewrite
    // everything, re-hash from genesis. Nothing inside the file
    // contradicts itself — which is exactly why the anchors are
    // published outside it. This shows the rewritten chain is
    // internally valid, so the anchor is the only thing left.
    let (bytes, _) = export_bytes();
    let first: Value =
        serde_json::from_slice(bytes.split(|byte| *byte == b'\n').next().expect("a line"))
            .expect("JSON");
    let anchored_hash = first["hash"].as_str().expect("a hash").to_owned();

    // One changed event instead of four, correctly re-chained.
    let mut event: AuditEvent = serde_json::from_value(first["event"].clone()).expect("an event");
    event.request_id = "req_TAMPERED".to_owned();
    let honest_hash = event_hash(ledgers_db::chain::GENESIS_HASH, &event).expect("hashed");
    let line = json!({
        "event": serde_json::to_value(&event).expect("a value"),
        "hash": honest_hash,
        "prev_hash": ledgers_db::chain::GENESIS_HASH,
    });
    let mut file = canonical_json(&line).expect("canonical").into_bytes();
    file.push(b'\n');
    assert!(verify(&file).is_ok(), "the rewrite is self-consistent");
    // ...and it contradicts the anchor published for seq 4, which is
    // the property an auditor actually checks.
    assert_ne!(anchored_hash, honest_hash);
}
