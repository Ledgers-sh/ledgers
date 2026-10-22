//! Translations for the bundled charts: flat key → string tables, one
//! TOML file per locale under `templates/i18n/`, embedded at compile time
//! and parsed once on first use.
//!
//! The files contain flat quoted keys only:
//!
//! ```toml
//! "account.generic_sme.cash" = "Cash"
//! "template.generic_sme.name" = "Generic small business chart"
//! ```
//!
//! Keys are looked up exactly (no fallback chain, no interpolation): a
//! missing key is `None` and the caller decides what that means — tests
//! treat it as a broken data file, the read paths as an untranslatable
//! name. Names are data, never instructions (docs/PLAN.md §3 rule 8).

use std::collections::HashMap;
use std::sync::OnceLock;

/// The locales the bundled charts ship: English, German, Indonesian, Dutch.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Locale {
    /// English (`"en"`).
    En,
    /// German (`"de"`).
    De,
    /// Indonesian (`"id"`).
    Id,
    /// Dutch (`"nl"`).
    Nl,
}

impl Locale {
    /// Parses a locale from its code; `None` for anything else.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "de" => Some(Self::De),
            "id" => Some(Self::Id),
            "nl" => Some(Self::Nl),
            _ => None,
        }
    }

    /// The locale's code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::De => "de",
            Self::Id => "id",
            Self::Nl => "nl",
        }
    }

    /// Every locale, in declaration order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::En, Self::De, Self::Id, Self::Nl]
    }
}

/// The English translation table (`templates/i18n/en.toml`).
pub const I18N_EN_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/i18n/en.toml"
));
/// The German translation table (`templates/i18n/de.toml`).
pub const I18N_DE_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/i18n/de.toml"
));
/// The Indonesian translation table (`templates/i18n/id.toml`).
pub const I18N_ID_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/i18n/id.toml"
));

/// The Dutch translation table (`templates/i18n/nl.toml`).
pub const I18N_NL_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/i18n/nl.toml"
));

/// The parsed table for `locale`, built on first use. A file that fails
/// to parse yields an empty table (lookups return `None`); the bundled
/// files are covered by tests that would fail loudly first.
fn table(locale: Locale) -> &'static HashMap<String, String> {
    static TABLES: OnceLock<[HashMap<String, String>; 4]> = OnceLock::new();
    &TABLES.get_or_init(|| {
        Locale::all().map(|locale| {
            let text = match locale {
                Locale::En => I18N_EN_TOML,
                Locale::De => I18N_DE_TOML,
                Locale::Id => I18N_ID_TOML,
                Locale::Nl => I18N_NL_TOML,
            };
            parse_table(text).unwrap_or_default()
        })
    })[locale as usize]
}

/// Parses a flat key → string TOML file. Every top-level entry must be a
/// string value; anything else is a data error (the files are ours).
fn parse_table(text: &str) -> Option<HashMap<String, String>> {
    let parsed: HashMap<String, toml::Value> = toml::from_str(text).ok()?;
    let mut table = HashMap::with_capacity(parsed.len());
    for (key, value) in parsed {
        let text = value.as_str()?.to_owned();
        table.insert(key, text);
    }
    Some(table)
}

/// Looks `key` up in `locale`'s table; `None` when the key is absent.
#[must_use]
pub fn translate(key: &str, locale: Locale) -> Option<&'static str> {
    table(locale).get(key).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_codes_round_trip() {
        for locale in Locale::all() {
            assert_eq!(Locale::from_code(locale.code()), Some(locale));
        }
        assert_eq!(Locale::from_code("en"), Some(Locale::En));
        assert_eq!(Locale::from_code("de"), Some(Locale::De));
        assert_eq!(Locale::from_code("id"), Some(Locale::Id));
        assert_eq!(Locale::from_code("nl"), Some(Locale::Nl));
        assert_eq!(Locale::from_code("fr"), None);
        assert_eq!(Locale::from_code(""), None);
    }

    #[test]
    fn every_bundled_locale_parses_and_is_flat_strings() {
        // The compile-time embed plus this parse: a broken file fails
        // here rather than as mystery `None`s in lookups.
        for locale in Locale::all() {
            let text = match locale {
                Locale::En => I18N_EN_TOML,
                Locale::De => I18N_DE_TOML,
                Locale::Id => I18N_ID_TOML,
                Locale::Nl => I18N_NL_TOML,
            };
            let parsed = parse_table(text).expect("each bundled i18n file parses");
            assert!(!parsed.is_empty(), "{} table is empty", locale.code());
        }
    }

    #[test]
    fn translate_hits_and_misses() {
        // A key no bundled chart uses: absent everywhere.
        for locale in Locale::all() {
            assert_eq!(translate("account.nope.missing", locale), None);
        }
        // The lookup returns a stable reference into the static table.
        let first = translate("template.generic_sme.name", Locale::En);
        let second = translate("template.generic_sme.name", Locale::En);
        assert_eq!(first, second);
    }

    #[test]
    fn flat_table_parser_rejects_nested_values() {
        assert!(parse_table("\"k\" = \"v\"\n").is_some());
        assert!(parse_table("[section]\n\"k\" = \"v\"\n").is_none());
        assert!(parse_table("\"k\" = 1\n").is_none());
    }
}
