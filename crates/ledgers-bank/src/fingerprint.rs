//! Duplicate detection for imported lines.
//!
//! A line is identified by its bank reference, booking date, signed amount
//! and currency on one account. A file with no bank reference (MT940
//! `NONREF`, most CSV exports) falls back to the counterparty IBAN and the
//! remittance text. Two genuinely identical lines (same reference, date and
//! amount) are told apart by occurrence: the k-th identical line of a file
//! is stored as occurrence k. A re-import therefore finds every occurrence
//! already stored and adds nothing, while an overlapping file that carries
//! one more identical line adds exactly that one.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// The identity of a line without its occurrence: the group it belongs to.
#[must_use]
pub fn group_key(
    account_id: &str,
    booking_date: &str,
    minor: i64,
    currency: &str,
    bank_reference: Option<&str>,
    counterparty_iban: Option<&str>,
    remittance: &str,
) -> String {
    let basis = match bank_reference.filter(|r| !r.trim().is_empty()) {
        Some(reference) => {
            format!("ref|{account_id}|{booking_date}|{minor}|{currency}|{reference}")
        }
        None => format!(
            "noref|{account_id}|{booking_date}|{minor}|{currency}|{}|{}",
            counterparty_iban.unwrap_or(""),
            sha256_hex(remittance)
        ),
    };
    sha256_hex(&basis)
}

/// The stored fingerprint of the k-th line of a group.
#[must_use]
pub fn fingerprint(group: &str, occurrence: u32) -> String {
    sha256_hex(&format!("{group}#{occurrence}"))
}

/// For each line of a file, in order: `Some(occurrence)` when it is new and
/// must be stored, `None` when it is already stored.
///
/// `existing` counts the stored lines per group before this import.
#[must_use]
pub fn assign_occurrences<S: std::hash::BuildHasher>(
    groups: &[String],
    existing: &HashMap<String, u32, S>,
) -> Vec<Option<u32>> {
    let mut seen: HashMap<&str, u32> = HashMap::new();
    groups
        .iter()
        .map(|group| {
            let k = seen.entry(group.as_str()).or_insert(0);
            let occurrence = *k;
            *k += 1;
            let stored = existing.get(group).copied().unwrap_or(0);
            (occurrence >= stored).then_some(occurrence)
        })
        .collect()
}

/// SHA-256 as lowercase hex.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(reference: Option<&str>, remittance: &str) -> String {
        group_key(
            "acct",
            "2026-03-02",
            -1250,
            "EUR",
            reference,
            Some("DE89370400440532013000"),
            remittance,
        )
    }

    #[test]
    fn the_same_reference_date_and_amount_is_the_same_group() {
        assert_eq!(g(Some("R1"), "a"), g(Some("R1"), "something else"));
        assert_ne!(g(Some("R1"), "a"), g(Some("R2"), "a"));
    }

    #[test]
    fn without_a_reference_the_remittance_tells_lines_apart() {
        assert_eq!(g(None, "Miete"), g(None, "Miete"));
        assert_ne!(g(None, "Miete"), g(None, "Strom"));
    }

    #[test]
    fn a_reimport_adds_nothing() {
        let groups = vec!["a".to_owned(), "b".to_owned(), "a".to_owned()];
        let existing = HashMap::from([("a".to_owned(), 2), ("b".to_owned(), 1)]);
        assert_eq!(
            assign_occurrences(&groups, &existing),
            vec![None, None, None]
        );
    }

    #[test]
    fn an_overlapping_file_adds_only_its_new_lines() {
        let groups = vec![
            "a".to_owned(),
            "b".to_owned(),
            "a".to_owned(),
            "a".to_owned(),
        ];
        // Stored before: one "a" and one "b". The file has two "a" and one "b".
        let existing = HashMap::from([("a".to_owned(), 1), ("b".to_owned(), 1)]);
        assert_eq!(
            assign_occurrences(&groups, &existing),
            vec![None, None, Some(1), Some(2)]
        );
    }

    #[test]
    fn fingerprints_differ_by_occurrence() {
        assert_ne!(fingerprint("grp", 0), fingerprint("grp", 1));
        assert_eq!(fingerprint("grp", 0).len(), 64);
    }
}
