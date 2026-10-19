//! The bundled-data contract: whatever lives under `templates/` when the
//! crate is built must parse, lint clean, translate in every bundled
//! locale, and map into the German chart — so a data edit cannot quietly
//! ship broken. These tests read the embedded consts (compile-time
//! copies) except the licence note, which is read from disk so it tracks
//! the files that actually ship.

use ledgers_ledger::chart::ChartTemplate;
use ledgers_ledger::i18n::{Locale, translate};
use ledgers_ledger::lint::{REQUIRED_SYSTEM_TAGS, lint_chart};
use ledgers_ledger::{
    DE_HGB_TOML, GENERIC_SME_TOML, bundled_mappings, de_hgb_template, generic_sme_template,
};

/// Every bundled data file by name — the same list `templates/LICENSES.md`
/// must mention.
const DATA_FILES: [&str; 7] = [
    "generic-sme.toml",
    "de-hgb.toml",
    "mappings/de-skr03.toml",
    "mappings/de-skr04.toml",
    "i18n/en.toml",
    "i18n/de.toml",
    "i18n/id.toml",
];

#[test]
fn both_bundled_charts_parse_and_lint_clean() {
    for (name, chart) in [
        (
            "generic-sme",
            generic_sme_template().expect("generic-sme parses"),
        ),
        ("de-hgb", de_hgb_template().expect("de-hgb parses")),
    ] {
        let violations = lint_chart(&chart);
        assert!(
            violations.is_empty(),
            "{name} left the portable chart contract: {violations:?}"
        );
        assert!(!chart.accounts.is_empty(), "{name} declares no accounts");
    }
}

#[test]
fn every_bundled_chart_carries_all_system_tags_on_leaves() {
    for chart in [
        generic_sme_template().expect("generic-sme parses"),
        de_hgb_template().expect("de-hgb parses"),
    ] {
        for tag in REQUIRED_SYSTEM_TAGS {
            let carriers: Vec<_> = chart
                .accounts
                .iter()
                .filter(|account| account.tags.iter().any(|candidate| candidate == tag))
                .collect();
            assert_eq!(carriers.len(), 1, "one {tag} account in {}", chart.id);
            let number = carriers[0].number.as_str();
            let is_leaf = chart
                .accounts
                .iter()
                .all(|account| account.parent.as_deref() != Some(number));
            assert!(is_leaf, "the {tag} account {number} must be a leaf");
        }
    }
}

#[test]
fn every_bundled_key_resolves_in_all_three_locales() {
    for chart in [
        generic_sme_template().expect("generic-sme parses"),
        de_hgb_template().expect("de-hgb parses"),
    ] {
        for locale in Locale::all() {
            let translated = translate(&chart.name, locale)
                .unwrap_or_else(|| panic!("{} name key missing in {}", chart.name, locale.code()));
            assert!(!translated.is_empty());
            for account in &chart.accounts {
                let translated = translate(&account.name, locale)
                    .unwrap_or_else(|| panic!("{} missing in {}", account.name, locale.code()));
                assert!(!translated.is_empty());
            }
        }
    }
}

#[test]
fn bundled_mappings_validate_against_the_german_chart() {
    let de_hgb = de_hgb_template().expect("de-hgb parses");
    let mappings = bundled_mappings().expect("the bundled mappings parse");
    assert_eq!(mappings.len(), 2, "SKR03 and SKR04");
    for mapping in &mappings {
        assert_eq!(mapping.target, "de-hgb");
        mapping
            .validate_against(&de_hgb)
            .unwrap_or_else(|violations| {
                panic!("{} invalid against de-hgb: {violations:?}", mapping.id)
            });
        // From-numbers: 4 digits and unique (also checked by
        // validate_against; asserted directly so a failure names the
        // file's own shape, not the cross-check).
        let mut seen = std::collections::HashSet::new();
        for entry in &mapping.map {
            assert_eq!(
                entry.from.len(),
                4,
                "{}: {} is not 4 digits",
                mapping.id,
                entry.from
            );
            assert!(
                entry.from.bytes().all(|b| b.is_ascii_digit()),
                "{}: {} is not all digits",
                mapping.id,
                entry.from
            );
            assert!(
                seen.insert(entry.from.as_str()),
                "{}: {} mapped twice",
                mapping.id,
                entry.from
            );
        }
    }
}

#[test]
fn the_licence_note_names_every_bundled_data_file() {
    let note = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/LICENSES.md"),
    )
    .expect("templates/LICENSES.md exists");
    for file in DATA_FILES {
        assert!(
            note.contains(file),
            "templates/LICENSES.md must mention {file}"
        );
    }
}

#[test]
fn the_embedded_consts_are_the_files_on_disk() {
    // The consts are compiled in; this pins them to the current files so
    // a stale build cannot pass the tests above on vanished data.
    let read = |relative: &str| {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates")
                .join(relative),
        )
        .unwrap_or_else(|err| panic!("cannot read templates/{relative}: {err}"))
    };
    assert_eq!(GENERIC_SME_TOML, read("generic-sme.toml"));
    assert_eq!(DE_HGB_TOML, read("de-hgb.toml"));
}

/// Parses both charts once more through the public parse path, proving
/// the consts round-trip (parse → lint → non-empty) for every file.
#[test]
fn consts_parse_through_the_public_path() {
    for text in [GENERIC_SME_TOML, DE_HGB_TOML] {
        let chart = ChartTemplate::parse(text).expect("bundled TOML parses");
        assert!(chart.lint().is_ok());
    }
    for text in [
        ledgers_ledger::SKR03_MAPPING_TOML,
        ledgers_ledger::SKR04_MAPPING_TOML,
    ] {
        ledgers_ledger::mapping::parse_mapping(text).expect("bundled mapping parses");
    }
}
