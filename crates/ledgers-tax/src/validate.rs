//! Registration-number validators, one per kind a country pack names.
//!
//! Each validator checks the shape the authority publishes and, where the
//! authority publishes a check-digit rule, that rule too. Where a rule is not
//! verified yet the validator checks the shape only, and the pack file says so
//! in its `validator_note`. A validator never contacts an authority.

use crate::error::TaxError;

/// The validator a pack names in its `registration.validator` key.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Validator {
    /// Any text is accepted (generic packs: the company types its own number).
    Any,
    /// A two-letter prefix and nine digits with the ISO 7064 Mod 11,10 check
    /// digit (the national VAT number of some EU states). The prefix must match the company's country;
    /// the server checks that.
    Iso7064Nine,
    /// Singapore UEN: the three ACRA shapes (business, local company, other entity).
    SgUen,
    /// Indonesian NPWP: 15 digits (legacy) or 16 digits (NIK-based, since 2024).
    IdNpwp,
    /// Dutch `BTW-identificatienummer`: `NL`, nine digits, `B`, two digits.
    NlBtw,
    /// UK VAT number: `GB` and nine or twelve digits, with the nine-digit mod-97 check.
    GbVat,
    /// Australian Business Number: eleven digits with the ATO mod-89 check.
    AuAbn,
    /// US Employer Identification Number: `NN-NNNNNNN`.
    UsEin,
    /// EU VAT number: a member-state prefix and an alphanumeric body.
    EuVat,
    /// Dutch KvK-nummer (Chamber of Commerce number): eight digits.
    NlKvk,
}

impl Validator {
    /// The validator named by a pack file.
    ///
    /// # Errors
    /// [`TaxError::Invalid`] for an unknown name.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        Ok(match code {
            "any" => Validator::Any,
            "iso7064-nine" => Validator::Iso7064Nine,
            "sg-uen" => Validator::SgUen,
            "id-npwp" => Validator::IdNpwp,
            "nl-btw" => Validator::NlBtw,
            "gb-vat" => Validator::GbVat,
            "au-abn" => Validator::AuAbn,
            "us-ein" => Validator::UsEin,
            "eu-vat" => Validator::EuVat,
            "nl-kvk" => Validator::NlKvk,
            other => {
                return Err(TaxError::Invalid {
                    reason: format!("unknown registration validator {other:?}"),
                });
            }
        })
    }

    /// The name a pack file uses for this validator.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Validator::Any => "any",
            Validator::Iso7064Nine => "iso7064-nine",
            Validator::SgUen => "sg-uen",
            Validator::IdNpwp => "id-npwp",
            Validator::NlBtw => "nl-btw",
            Validator::GbVat => "gb-vat",
            Validator::AuAbn => "au-abn",
            Validator::UsEin => "us-ein",
            Validator::EuVat => "eu-vat",
            Validator::NlKvk => "nl-kvk",
        }
    }

    /// Strips the separators people type (spaces, dots, dashes) and upper-cases.
    #[must_use]
    pub fn normalize(raw: &str) -> String {
        raw.chars()
            .filter(|c| !matches!(c, ' ' | '.' | '-' | '\t'))
            .collect::<String>()
            .to_ascii_uppercase()
    }

    /// Checks a normalized number (see [`Validator::normalize`]).
    ///
    /// # Errors
    /// A sentence naming what the number must look like.
    pub fn check(self, number: &str) -> Result<(), String> {
        match self {
            Validator::Any => Ok(()),
            Validator::Iso7064Nine => check_iso7064_nine(number),
            Validator::SgUen => check_sg_uen(number),
            Validator::IdNpwp => check_id_npwp(number),
            Validator::NlBtw => check_nl_btw(number),
            Validator::GbVat => check_gb_vat(number),
            Validator::AuAbn => check_au_abn(number),
            Validator::UsEin => check_us_ein(number),
            Validator::EuVat => check_eu_vat(number),
            Validator::NlKvk => check_nl_kvk(number),
        }
    }
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

fn digit_values(text: &str) -> Vec<u32> {
    text.bytes().map(|b| u32::from(b - b'0')).collect()
}

/// ISO 7064 Mod 11,10 over the first eight digits; the ninth is the check digit.
fn check_iso7064_nine(number: &str) -> Result<(), String> {
    let body = number
        .get(2..)
        .filter(|b| b.len() == 9 && all_digits(b))
        .ok_or_else(|| "This VAT ID is two letters followed by nine digits.".to_owned())?;
    let digits = digit_values(body);
    let mut product = 10u32;
    for digit in &digits[..8] {
        let mut sum = (digit + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let mut check = 11 - product;
    if check == 10 {
        check = 0;
    }
    if check == digits[8] {
        Ok(())
    } else {
        Err("The check digit of this VAT ID does not match.".to_owned())
    }
}

/// ACRA UEN shapes: `NNNNNNNNC` (8 digits + letter, businesses registered
/// before 2009), `NNNNNNNNNC` (9 digits + letter, local companies), and
/// `TYYPQNNNNC` / `SYYPQNNNNC` / `RYYPQNNNNC` (other entities). The check
/// letter itself is not verified here (see the pack's `validator_note`).
fn check_sg_uen(number: &str) -> Result<(), String> {
    let bytes = number.as_bytes();
    let letter = |b: u8| b.is_ascii_uppercase();
    let ok = match bytes.len() {
        9 => all_digits(&number[..8]) && letter(bytes[8]),
        10 => {
            (all_digits(&number[..9]) && letter(bytes[9]))
                || (matches!(bytes[0], b'T' | b'S' | b'R')
                    && all_digits(&number[1..3])
                    && letter(bytes[3])
                    && letter(bytes[4])
                    && all_digits(&number[5..9])
                    && letter(bytes[9]))
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err("A Singapore UEN is 9 or 10 characters, such as 53012345K or 201912345K.".to_owned())
    }
}

/// Indonesian NPWP: 15 digits (legacy) or 16 digits (NIK-based). Separators
/// are removed before the check.
fn check_id_npwp(number: &str) -> Result<(), String> {
    if all_digits(number) && (number.len() == 15 || number.len() == 16) {
        Ok(())
    } else {
        Err("An Indonesian NPWP is 15 or 16 digits.".to_owned())
    }
}

/// A KvK-nummer is eight digits. The number carries no check digit, so this
/// checks the shape only; existence is not checked against the register.
/// TODO: cite the `KvK` specification for the shape (kvk.nl), not yet fetched.
fn check_nl_kvk(number: &str) -> Result<(), String> {
    if number.len() == 8 && all_digits(number) {
        Ok(())
    } else {
        Err("A KvK-nummer is eight digits.".to_owned())
    }
}

/// `NL` + nine digits + `B` + two digits.
fn check_nl_btw(number: &str) -> Result<(), String> {
    let ok = number.len() == 14
        && number.starts_with("NL")
        && all_digits(&number[2..11])
        && number.as_bytes()[11] == b'B'
        && all_digits(&number[12..14]);
    if ok {
        Ok(())
    } else {
        Err(
            "A Dutch BTW-id is NL, nine digits, B and two digits, such as NL123456782B12."
                .to_owned(),
        )
    }
}

/// HMRC: `GB` + nine digits (mod-97 check, weights 8..2 on the first seven
/// digits, check = 97 - (sum mod 97)), or twelve digits, or `GBGD`/`GBHA`
/// + three digits (government and health-authority numbers).
fn check_gb_vat(number: &str) -> Result<(), String> {
    let body = number
        .strip_prefix("GB")
        .ok_or_else(|| "A UK VAT number starts with GB.".to_owned())?;
    let shape_ok = match body.len() {
        9 => all_digits(body) && gb_mod97(body),
        12 => all_digits(body),
        5 => (body.starts_with("GD") || body.starts_with("HA")) && all_digits(&body[2..]),
        _ => false,
    };
    if shape_ok {
        Ok(())
    } else {
        Err("A UK VAT number is GB followed by nine or twelve digits.".to_owned())
    }
}

fn gb_mod97(body: &str) -> bool {
    let digits = digit_values(body);
    let weights = [8u32, 7, 6, 5, 4, 3, 2];
    let sum: u32 = digits[..7].iter().zip(weights).map(|(d, w)| d * w).sum();
    let check = (97 - sum % 97) % 97;
    check == digits[7] * 10 + digits[8]
}

/// ATO: subtract one from the first digit, weight the eleven digits by
/// 10,1,3,5,7,9,11,13,15,17,19; the sum must be divisible by 89.
fn check_au_abn(number: &str) -> Result<(), String> {
    if number.len() != 11 || !all_digits(number) {
        return Err("An Australian ABN is eleven digits.".to_owned());
    }
    let mut digits = digit_values(number);
    if digits[0] == 0 {
        return Err("This ABN fails the ATO check digits.".to_owned());
    }
    digits[0] -= 1;
    let weights = [10u32, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19];
    let sum: u32 = digits.iter().zip(weights).map(|(d, w)| d * w).sum();
    if sum.is_multiple_of(89) {
        Ok(())
    } else {
        Err("This ABN fails the ATO check digits.".to_owned())
    }
}

/// `NN-NNNNNNN`: nine digits once the dash is removed by [`Validator::normalize`].
/// The IRS campus prefix list is not checked.
fn check_us_ein(number: &str) -> Result<(), String> {
    if number.len() == 9 && all_digits(number) {
        Ok(())
    } else {
        Err("A US EIN is nine digits, written NN-NNNNNNN.".to_owned())
    }
}

/// Per-state VAT-number patterns the EU validator knows. Class letters: `d`
/// digit, `b` digit 0 or 1, `a` capital letter, `x` capital letter or digit;
/// a repeat count follows (`d9` is nine digits). Other capitals are literal.
/// The pattern includes the country prefix. States not listed fall back to
/// the shape check.
const EU_PATTERNS: &[(&str, &[&str])] = &[
    ("AT", &["ATUd8"]),
    ("BE", &["BEbd9"]),
    ("DK", &["DKd8"]),
    ("ES", &["ESxd7x"]),
    ("FR", &["FRxxd9"]),
    ("IE", &["IEdxd5a", "IEdxd5aa"]),
    ("IT", &["ITd11"]),
    ("PL", &["PLd10"]),
    ("SE", &["SEd12"]),
];

fn matches_pattern(text: &str, pattern: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    let mut spec = pattern.chars().peekable();
    while let Some(class) = spec.next() {
        let mut count_text = String::new();
        while let Some(digit) = spec.peek().filter(|c| c.is_ascii_digit()) {
            count_text.push(*digit);
            spec.next();
        }
        if matches!(class, 'd' | 'b' | 'a' | 'x') {
            let count: usize = if count_text.is_empty() {
                1
            } else {
                count_text.parse().unwrap_or(1)
            };
            for _ in 0..count {
                let Some(&c) = chars.get(pos) else {
                    return false;
                };
                let ok = match class {
                    'd' => c.is_ascii_digit(),
                    'b' => c == '0' || c == '1',
                    'a' => c.is_ascii_uppercase(),
                    _ => c.is_ascii_uppercase() || c.is_ascii_digit(),
                };
                if !ok {
                    return false;
                }
                pos += 1;
            }
        } else if chars.get(pos) != Some(&class) {
            return false;
        } else {
            pos += 1;
        }
    }
    pos == chars.len()
}

/// Member-state prefix; listed states must match their pattern, the others
/// need a 2 to 12 character alphanumeric body.
fn check_eu_vat(number: &str) -> Result<(), String> {
    if number.len() < 4 || !number.is_char_boundary(2) {
        return Err("An EU VAT number starts with a member-state code.".to_owned());
    }
    // The prefix must be the company's country; the server checks that.
    let (prefix, body) = number.split_at(2);
    if !prefix.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err("An EU VAT number starts with a member-state code.".to_owned());
    }
    if let Some((_, patterns)) = EU_PATTERNS.iter().find(|(p, _)| *p == prefix) {
        return if patterns.iter().any(|pat| matches_pattern(number, pat)) {
            Ok(())
        } else {
            Err(format!(
                "The VAT number format for {prefix} does not match."
            ))
        };
    }
    if (2..=12).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err("The body of an EU VAT number is 2 to 12 letters or digits.".to_owned())
    }
}

/// Upper-cases a VAT ID and drops the spaces, dots and dashes people type
/// between its groups.
#[must_use]
pub fn normalize_vat_id(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, ' ' | '.' | '-' | '\t'))
        .flat_map(char::to_uppercase)
        .collect()
}

/// Checks a VAT ID against its country's pack (`ledgers_tax::country`). An
/// empty VAT ID is allowed (the company may not be VAT registered). A country
/// no pack covers accepts its own prefix and two to twelve letters or digits.
///
/// # Errors
/// A plain message, shown next to the field.
pub fn check_vat_id(country: &str, vat_id: &str) -> Result<(), String> {
    if vat_id.is_empty() {
        return Ok(());
    }
    if country.is_empty() {
        return Err("Choose the country before entering a VAT ID.".to_owned());
    }
    if !vat_id.starts_with(country) {
        return Err(format!("A {country} VAT ID starts with {country}."));
    }
    let number = Validator::normalize(vat_id);
    let pack = crate::country::pack_for_country(country).map_err(|err| err.to_string())?;
    if let Some(pack) = pack {
        return pack.registration.validator.check(&number);
    }
    let body = number.get(country.len()..).unwrap_or("");
    if (2..=12).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(format!("This is not a valid {country} VAT ID format."))
    }
}
