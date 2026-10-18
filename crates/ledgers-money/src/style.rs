//! Decimal styles: the separator characters and grouping patterns used to
//! parse and format amounts for humans.
//!
//! ## Why not ICU4X
//!
//! Grouping needs a tiny, deterministic table, not a locale database:
//! this crate must stay dependency-light and float-free, and full CLDR
//! locale data (currency symbol placement, compact notation, pluralised
//! unit names) is out of scope for exact money arithmetic. The six
//! [`DecimalStyle`] constants below cover the interchange format plus the
//! locales Ledgers formats documents for. Revisit this decision if symbol
//! rendering is ever needed.

/// A decimal style: decimal separator, accepted group separators and a
/// CLDR-style grouping pattern.
///
/// `groups` is read from the decimal point outward and the **last element
/// repeats**: `[3]` groups by three (`1,234,567`), `[3, 2]` is the Indian
/// pattern (`1,23,45,678` — three, then twos). An empty `groups` (with
/// empty `group_seps`, as in [`DecimalStyle::CANONICAL`]) never groups.
///
/// Formatting always uses `group_seps[0]`; parsing accepts every separator
/// in `group_seps`, so a style can accept historical variants while
/// emitting exactly one canonical form ([`DecimalStyle::FR_FR`] formats
/// with the narrow no-break space but also accepts the no-break and plain
/// spaces).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DecimalStyle {
    pub(super) decimal_sep: char,
    pub(super) group_seps: &'static [char],
    pub(super) groups: &'static [u8],
}

impl DecimalStyle {
    /// The interchange style: `.` decimal separator, no group separators.
    /// [`Amount::to_canonical_string`](crate::Amount::to_canonical_string)
    /// and
    /// [`Amount::parse_canonical`](crate::Amount::parse_canonical) always
    /// use it; any group separator is rejected.
    pub const CANONICAL: Self = Self {
        decimal_sep: '.',
        group_seps: &[],
        groups: &[],
    };

    /// German: `1.234,56`.
    pub const DE_DE: Self = Self {
        decimal_sep: ',',
        group_seps: &['.'],
        groups: &[3],
    };

    /// English (US): `1,234.56`.
    pub const EN_US: Self = Self {
        decimal_sep: '.',
        group_seps: &[','],
        groups: &[3],
    };

    /// French: decimal `,`, grouped with the narrow no-break space
    /// (U+202F); parsing also accepts the no-break space (U+00A0) and the
    /// plain space, so older documents in all three variants keep parsing.
    pub const FR_FR: Self = Self {
        decimal_sep: ',',
        group_seps: &['\u{202f}', '\u{a0}', ' '],
        groups: &[3],
    };

    /// Swiss: `1'234.56`.
    pub const DE_CH: Self = Self {
        decimal_sep: '.',
        group_seps: &['\''],
        groups: &[3],
    };

    /// Indian: `1,23,45,678.90` — three digits, then groups of two, read
    /// from the decimal point outward.
    pub const EN_IN: Self = Self {
        decimal_sep: '.',
        group_seps: &[','],
        groups: &[3, 2],
    };

    /// The decimal separator this style formats and parses with.
    #[must_use]
    pub const fn decimal_sep(self) -> char {
        self.decimal_sep
    }

    /// The group separators this style accepts on parse; element 0 is the
    /// one formatting emits.
    #[must_use]
    pub const fn group_seps(self) -> &'static [char] {
        self.group_seps
    }

    /// The grouping pattern, read from the decimal point outward; the last
    /// element repeats.
    #[must_use]
    pub const fn groups(self) -> &'static [u8] {
        self.groups
    }
}

#[cfg(test)]
mod tests {
    use super::DecimalStyle;

    #[test]
    fn constants_are_distinct() {
        let all = [
            DecimalStyle::CANONICAL,
            DecimalStyle::DE_DE,
            DecimalStyle::EN_US,
            DecimalStyle::FR_FR,
            DecimalStyle::DE_CH,
            DecimalStyle::EN_IN,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn indian_pattern_is_three_then_twos() {
        assert_eq!(DecimalStyle::EN_IN.groups(), &[3, 2]);
        assert_eq!(DecimalStyle::EN_US.groups(), &[3]);
        assert!(DecimalStyle::CANONICAL.groups().is_empty());
    }

    #[test]
    fn french_accepts_three_space_variants_on_parse() {
        assert_eq!(
            DecimalStyle::FR_FR.group_seps(),
            &['\u{202f}', '\u{a0}', ' ']
        );
        assert_eq!(DecimalStyle::FR_FR.group_seps()[0], '\u{202f}');
    }
}
