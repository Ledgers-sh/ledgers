//! Validation rules for the Settings screens: the company profile fields,
//! VAT identifiers per country, and the approval rule values. Pure functions,
//! so the commands and their tests share one definition of "valid".

use ledgers_money::Currency;
use std::collections::BTreeMap;

/// The longest company name, in characters.
pub const MAX_NAME_CHARS: usize = 120;
/// The longest legal form text, in characters.
pub const MAX_LEGAL_FORM_CHARS: usize = 80;
/// The longest registration number, in characters.
pub const MAX_REGISTRATION_CHARS: usize = 60;
/// The longest address line, in characters.
pub const MAX_ADDRESS_CHARS: usize = 200;
/// The longest postal code, in characters.
pub const MAX_POSTAL_CODE_CHARS: usize = 20;
/// The longest city name, in characters.
pub const MAX_CITY_CHARS: usize = 100;
/// The longest contact e-mail address, in bytes (RFC 5321).
pub const MAX_EMAIL_BYTES: usize = 254;
/// The most approval-limit currencies one company may configure.
pub const MAX_LOW_RISK_CURRENCIES: usize = 20;
/// The largest low-risk limit accepted, in major units of the currency.
pub const MAX_LOW_RISK_MAJOR: i128 = 1_000_000_000;
/// The shortest request expiry, in hours.
pub const MIN_REQUEST_TTL_HOURS: u32 = 1;
/// The longest request expiry, in hours (one week).
pub const MAX_REQUEST_TTL_HOURS: u32 = 168;

pub use ledgers_tax::validate::{check_vat_id, normalize_vat_id};

/// The text fields may hold printable characters only: no control characters,
/// so nothing can break a line in the audit trail or a PDF.
fn printable(text: &str) -> bool {
    text.chars().all(|c| !c.is_control())
}

/// Trims a free-text field and checks its length and characters.
///
/// # Errors
/// A message naming the field.
pub fn text_field(
    label: &str,
    raw: &str,
    max_chars: usize,
    required: bool,
) -> Result<String, String> {
    let value = raw.trim().to_owned();
    if value.is_empty() {
        return if required {
            Err(format!("{label} is required."))
        } else {
            Ok(value)
        };
    }
    if !printable(&value) {
        return Err(format!(
            "{label} contains characters that cannot be stored."
        ));
    }
    if value.chars().count() > max_chars {
        return Err(format!("{label} can be at most {max_chars} characters."));
    }
    Ok(value)
}

/// Checks a contact e-mail address. Empty is allowed.
///
/// # Errors
/// A message when the address is not one mailbox.
pub fn email_field(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_owned();
    if value.is_empty() {
        return Ok(value);
    }
    let bad = || Err("Enter one e-mail address, such as accounts@example.com.".to_owned());
    if value.len() > MAX_EMAIL_BYTES || value.chars().any(char::is_whitespace) {
        return bad();
    }
    let Some((local, domain)) = value.split_once('@') else {
        return bad();
    };
    if local.is_empty() || domain.contains('@') {
        return bad();
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let labels_ok = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        });
    if labels_ok { Ok(value) } else { bad() }
}

/// Checks a country code: empty, or two capital letters (ISO 3166-1 alpha-2).
///
/// # Errors
/// A message when the code is neither.
pub fn country_field(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_uppercase();
    if value.is_empty() {
        return Ok(value);
    }
    if value.len() == 2 && value.chars().all(|c| c.is_ascii_uppercase()) {
        Ok(value)
    } else {
        Err("Choose a two-letter country code, such as DE.".to_owned())
    }
}

/// Reads a decimal amount in a currency's own exponent. Digits, an optional
/// point, and at most `exponent` decimals (`100`, `100.5`, `100.50` for EUR;
/// `15000` for JPY). Returns the canonical text (`100.50`) and the amount in
/// minor units.
fn canonical_amount(raw: &str, currency: Currency) -> Option<(String, i128)> {
    let text = raw.trim();
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (!frac.is_empty() && !digits(frac)) {
        return None;
    }
    let exponent = usize::try_from(currency.exponent()).ok()?;
    if frac.len() > exponent {
        return None;
    }
    let whole_value: i128 = whole.parse().ok()?;
    let frac_padded = format!("{frac:0<exponent$}");
    let frac_value: i128 = if frac_padded.is_empty() {
        0
    } else {
        frac_padded.parse().ok()?
    };
    let minor = whole_value
        .checked_mul(currency.minor_per_major())?
        .checked_add(frac_value)?;
    let canonical = if exponent == 0 {
        whole_value.to_string()
    } else {
        format!("{whole_value}.{frac_padded}")
    };
    Some((canonical, minor))
}

/// Checks the per-currency low-risk limits and returns them in canonical form
/// (`100` becomes `100.00` for EUR). Each code must be an ISO 4217 currency,
/// each amount positive, in that currency's exponent, and at most the ceiling.
///
/// # Errors
/// A message naming the offending currency.
pub fn low_risk_limits(raw: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    if raw.len() > MAX_LOW_RISK_CURRENCIES {
        return Err(format!(
            "At most {MAX_LOW_RISK_CURRENCIES} currencies can have a low-risk limit."
        ));
    }
    let mut checked = BTreeMap::new();
    for (code, amount) in raw {
        let currency: Currency = code
            .parse()
            .map_err(|_| format!("{code} is not a currency code."))?;
        let (canonical, minor) = canonical_amount(amount, currency).ok_or_else(|| {
            format!(
                "The {code} limit must be an amount with at most {} decimal places.",
                currency.exponent()
            )
        })?;
        if minor <= 0 {
            return Err(format!("The {code} limit must be above zero."));
        }
        if minor / currency.minor_per_major() > MAX_LOW_RISK_MAJOR {
            return Err(format!("The {code} limit is above the ceiling."));
        }
        checked.insert(code.clone(), canonical);
    }
    Ok(checked)
}

/// Checks the request expiry in hours.
///
/// # Errors
/// A message when the value is outside the allowed range.
pub fn request_ttl_hours(hours: u32) -> Result<u32, String> {
    if (MIN_REQUEST_TTL_HOURS..=MAX_REQUEST_TTL_HOURS).contains(&hours) {
        Ok(hours)
    } else {
        Err(format!(
            "Requests expire after {MIN_REQUEST_TTL_HOURS} to {MAX_REQUEST_TTL_HOURS} hours."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vat_ids_follow_their_country() {
        for (country, vat) in [
            ("DE", "DE136695976"),
            ("AT", "ATU12345678"),
            ("BE", "BE0123456789"),
            ("FR", "FR12345678901"),
            ("NL", "NL123456789B01"),
            ("IT", "IT12345678901"),
            ("ES", "ESX1234567Z"),
            ("GB", "GB999999973"),
            ("GB", "GBGD123"),
            ("PL", "PL1234567890"),
            ("SE", "SE123456789012"),
            ("DK", "DK12345678"),
            ("IE", "IE1234567T"),
            ("XX", "XX1234"),
        ] {
            assert_eq!(
                check_vat_id(country, vat),
                Ok(()),
                "{vat} should pass for {country}"
            );
        }
    }

    #[test]
    fn vat_ids_reject_wrong_shapes_with_a_reason() {
        assert!(check_vat_id("DE", "DE12345678").is_err(), "too short");
        assert!(check_vat_id("DE", "DE123456789").is_err(), "check digit");
        assert!(check_vat_id("DE", "DE1234567890").is_err(), "too long");
        assert!(
            check_vat_id("DE", "DEA23456789").is_err(),
            "letter in digits"
        );
        assert!(
            check_vat_id("DE", "FR123456789").is_err(),
            "other country prefix"
        );
        assert!(check_vat_id("", "DE123456789").is_err(), "country needed");
        assert!(
            check_vat_id("NL", "NL123456789B1").is_err(),
            "NL needs two digits"
        );
        let message = check_vat_id("DE", "FR1").unwrap_err();
        assert_eq!(message, "A DE VAT ID starts with DE.");
    }

    #[test]
    fn empty_vat_id_is_allowed_and_normalization_drops_separators() {
        assert_eq!(check_vat_id("DE", ""), Ok(()));
        assert_eq!(normalize_vat_id(" de 123.456-789 "), "DE123456789");
    }

    #[test]
    fn text_fields_trim_and_bound_length() {
        assert_eq!(
            text_field("Name", "  Halden  ", 20, true),
            Ok("Halden".to_owned())
        );
        assert!(text_field("Name", "   ", 20, true).is_err());
        assert_eq!(text_field("City", "", 20, false), Ok(String::new()));
        assert!(text_field("City", &"x".repeat(21), 20, false).is_err());
        assert!(text_field("City", "a\u{0007}b", 20, false).is_err());
    }

    #[test]
    fn email_needs_one_mailbox() {
        assert_eq!(email_field(" a@halden.de "), Ok("a@halden.de".to_owned()));
        assert_eq!(email_field(""), Ok(String::new()));
        for bad in [
            "plain", "a@b", "a b@c.de", "@c.de", "a@@c.de", "a@-c.de", "a@c..de",
        ] {
            assert!(email_field(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn country_is_two_capitals() {
        assert_eq!(country_field("de"), Ok("DE".to_owned()));
        assert_eq!(country_field(""), Ok(String::new()));
        assert!(country_field("DEU").is_err());
        assert!(country_field("D1").is_err());
    }

    #[test]
    fn low_risk_limits_use_each_currency_exponent() {
        let mut raw = BTreeMap::new();
        raw.insert("EUR".to_owned(), "100.00".to_owned());
        raw.insert("JPY".to_owned(), "15000".to_owned());
        assert_eq!(low_risk_limits(&raw).unwrap(), raw);
        let mut short = BTreeMap::new();
        short.insert("EUR".to_owned(), "100".to_owned());
        short.insert("USD".to_owned(), "7.5".to_owned());
        let normal = low_risk_limits(&short).unwrap();
        assert_eq!(normal["EUR"], "100.00");
        assert_eq!(normal["USD"], "7.50");
        let mut wrong = BTreeMap::new();
        wrong.insert("JPY".to_owned(), "15000.00".to_owned());
        assert!(low_risk_limits(&wrong).is_err(), "JPY has no decimals");
        let mut zero = BTreeMap::new();
        zero.insert("EUR".to_owned(), "0.00".to_owned());
        assert!(low_risk_limits(&zero).is_err());
        let mut unknown = BTreeMap::new();
        unknown.insert("ZZZ".to_owned(), "1.00".to_owned());
        assert!(low_risk_limits(&unknown).is_err());
    }

    #[test]
    fn request_expiry_is_bounded() {
        assert_eq!(request_ttl_hours(24), Ok(24));
        assert!(request_ttl_hours(0).is_err());
        assert!(request_ttl_hours(169).is_err());
    }
}
