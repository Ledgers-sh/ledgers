//! IBAN validation: the country's length, the ISO 13616 mod-97 check.
//!
//! An IBAN is checked on the bytes a person typed, after spaces are removed
//! and letters are upper-cased. A country missing from the table is refused:
//! a bank account must name a country whose IBAN length we can verify.

/// The IBAN length per country (ISO 3166 alpha-2), from the SWIFT registry.
const LENGTHS: &[(&str, usize)] = &[
    ("AD", 24),
    ("AE", 23),
    ("AT", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("CH", 21),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DK", 18),
    ("EE", 20),
    ("ES", 24),
    ("FI", 18),
    ("FR", 27),
    ("GB", 22),
    ("GI", 23),
    ("GR", 27),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IS", 26),
    ("IT", 27),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("MC", 27),
    ("MT", 31),
    ("NL", 18),
    ("NO", 15),
    ("PL", 28),
    ("PT", 25),
    ("RO", 24),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("TR", 26),
];

/// Why an IBAN is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IbanError {
    /// Not two letters, two digits and alphanumerics.
    Format,
    /// The country is not in the table of IBAN countries.
    UnknownCountry,
    /// The length is not the country's length.
    Length,
    /// The mod-97 check digits do not agree.
    Checksum,
}

impl std::fmt::Display for IbanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            IbanError::Format => "the IBAN has the wrong shape",
            IbanError::UnknownCountry => "the IBAN country is not supported",
            IbanError::Length => "the IBAN has the wrong length for its country",
            IbanError::Checksum => "the IBAN check digits do not match",
        };
        f.write_str(text)
    }
}

/// Normalises an IBAN as typed: spaces removed, upper case.
#[must_use]
pub fn normalise(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Validates an IBAN and returns it normalised.
///
/// # Errors
/// [`IbanError`] naming the first rule the IBAN breaks.
pub fn validate(input: &str) -> Result<String, IbanError> {
    let iban = normalise(input);
    let bytes = iban.as_bytes();
    if bytes.len() < 5
        || !bytes[..2].iter().all(u8::is_ascii_uppercase)
        || !bytes[2..4].iter().all(u8::is_ascii_digit)
        || !bytes[4..].iter().all(u8::is_ascii_alphanumeric)
    {
        return Err(IbanError::Format);
    }
    let country = &iban[..2];
    let expected = LENGTHS
        .iter()
        .find(|(code, _)| *code == country)
        .map(|(_, len)| *len)
        .ok_or(IbanError::UnknownCountry)?;
    if bytes.len() != expected {
        return Err(IbanError::Length);
    }
    if mod97(&iban) == 1 {
        Ok(iban)
    } else {
        Err(IbanError::Checksum)
    }
}

/// The mod-97 remainder of the rearranged IBAN, computed digit by digit so
/// no big integer is needed: letters A..Z are the values 10..35.
fn mod97(iban: &str) -> u32 {
    let rearranged = iban[4..].chars().chain(iban[..4].chars());
    let mut rem: u32 = 0;
    for c in rearranged {
        if let Some(digit) = c.to_digit(10) {
            rem = (rem * 10 + digit) % 97;
        } else {
            // A letter contributes its two-digit value, 10..=35.
            let value = u32::from(c) - u32::from('A') + 10;
            rem = (rem * 100 + value) % 97;
        }
    }
    rem
}

/// The IBAN with its check digits recomputed. Used by the fixtures and the
/// demo seed to produce valid invented account numbers.
///
/// # Panics
/// Never for a country in the table with a 4-character prefix and BBAN of
/// the right length; callers pass fixed, invented inputs.
#[must_use]
pub fn with_check_digits(country: &str, bban: &str) -> String {
    let body = format!("{country}00{bban}");
    let rem = mod97(&body);
    let check = 98 - rem;
    format!("{country}{check:02}{bban}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_ibans_validate_and_normalise() {
        assert_eq!(
            validate("DE89 3704 0044 0532 0130 00").unwrap(),
            "DE89370400440532013000"
        );
        assert_eq!(
            validate("gb82 west 1234 5698 7654 32").unwrap(),
            "GB82WEST12345698765432"
        );
        assert_eq!(
            validate("FR14 2004 1010 0505 0001 3M02 606").unwrap(),
            "FR1420041010050500013M02606"
        );
    }

    #[test]
    fn a_wrong_check_digit_is_a_checksum_error() {
        assert_eq!(validate("DE89370400440532013001"), Err(IbanError::Checksum));
    }

    #[test]
    fn a_wrong_length_is_refused_before_the_checksum() {
        assert_eq!(validate("DE8937040044053201300"), Err(IbanError::Length));
    }

    #[test]
    fn an_unknown_country_is_refused() {
        assert_eq!(
            validate("XX89370400440532013000"),
            Err(IbanError::UnknownCountry)
        );
    }

    #[test]
    fn malformed_shapes_are_format_errors() {
        assert_eq!(validate("1289370400440532013000"), Err(IbanError::Format));
        assert_eq!(validate("DE"), Err(IbanError::Format));
        assert_eq!(validate("DE89-3704-0044"), Err(IbanError::Format));
    }

    #[test]
    fn generated_ibans_always_pass_the_check() {
        let iban = with_check_digits("DE", "370400440532013000");
        assert_eq!(validate(&iban).unwrap(), iban);
        assert_eq!(iban, "DE89370400440532013000");
    }

    #[test]
    fn every_single_digit_change_breaks_the_checksum() {
        let good = "DE89370400440532013000";
        for index in 4..good.len() {
            let original = good.as_bytes()[index];
            if !original.is_ascii_digit() {
                continue;
            }
            for replacement in b'0'..=b'9' {
                if replacement == original {
                    continue;
                }
                let mut bytes = good.as_bytes().to_vec();
                bytes[index] = replacement;
                let changed = String::from_utf8(bytes).unwrap();
                assert_eq!(validate(&changed), Err(IbanError::Checksum), "{changed}");
            }
        }
    }
}
