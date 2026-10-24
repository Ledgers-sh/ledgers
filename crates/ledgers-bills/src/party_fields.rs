//! The party directory's fields and their refusals: the typed problem slugs a
//! party command can answer with, and the format checks behind the metadata
//! (website, phone, tags, further tax IDs, currency, logo).
//!
//! Every refusal is a [`CommandError::Domain`] carrying one of the `party-*`
//! problem types below and a `field` extension naming the input field. The app
//! maps each slug to a message in its own language; the detail is the server's
//! English, kept for the API and the CLI.

use cratefield_core::axum::http::StatusCode;
use cratefield_core::{Problem, ProblemDef};
use ledgers_commands::CommandError;
use ledgers_tax::{check_vat_id, normalize_vat_id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const fn def(
    slug: &'static str,
    status: cratefield_core::axum::http::StatusCode,
    title: &'static str,
    description: &'static str,
) -> ProblemDef {
    ProblemDef {
        slug,
        status,
        title,
        description,
    }
}

/// The party's name is missing.
pub const NAME_REQUIRED: ProblemDef = def(
    "party-name-required",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Party name required",
    "A party needs a legal name.",
);
/// The VAT ID is malformed or fails its country's check.
pub const VAT_INVALID: ProblemDef = def(
    "party-vat-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "VAT ID invalid",
    "The VAT ID does not pass its country's check.",
);
/// Another party already has this VAT ID.
pub const VAT_TAKEN: ProblemDef = def(
    "party-vat-taken",
    StatusCode::CONFLICT,
    "VAT ID already used",
    "Another party in this company already has this VAT ID.",
);
/// The IBAN is malformed or fails its checksum.
pub const IBAN_INVALID: ProblemDef = def(
    "party-iban-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "IBAN invalid",
    "The IBAN is malformed or fails its checksum.",
);
/// The BIC is malformed.
pub const BIC_INVALID: ProblemDef = def(
    "party-bic-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "BIC invalid",
    "A BIC is 8 or 11 letters and digits.",
);
/// The role is not customer, vendor or both.
pub const ROLES_INVALID: ProblemDef = def(
    "party-roles-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Role invalid",
    "Roles must be customer, vendor or both.",
);
/// Payment terms are negative or not a whole number of days.
pub const TERMS_INVALID: ProblemDef = def(
    "party-payment-terms-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Payment terms invalid",
    "Payment terms are a whole number of days, zero or more.",
);
/// An address is incomplete or has an unknown kind.
pub const ADDRESS_INVALID: ProblemDef = def(
    "party-address-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Address invalid",
    "An address needs its first line, a city and a two-letter country code.",
);
/// A contact is incomplete or has a malformed email.
pub const CONTACT_INVALID: ProblemDef = def(
    "party-contact-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Contact invalid",
    "A contact needs a name, and an email address must look right.",
);
/// More than one contact is marked primary.
pub const CONTACT_PRIMARY_DUPLICATE: ProblemDef = def(
    "party-contact-primary-duplicate",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Several primary contacts",
    "Only one contact can be the primary contact.",
);
/// The party's email address does not look right.
pub const EMAIL_INVALID: ProblemDef = def(
    "party-email-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Email invalid",
    "The email address does not look right.",
);
/// The website is not an http or https address.
pub const WEBSITE_INVALID: ProblemDef = def(
    "party-website-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Website invalid",
    "A website starts with http:// or https:// and names a domain.",
);
/// The phone number has characters or a length that is not a phone number.
pub const PHONE_INVALID: ProblemDef = def(
    "party-phone-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Phone invalid",
    "A phone number has 6 to 20 digits, optionally starting with +.",
);
/// The registration number has characters that are not allowed.
pub const REGISTRATION_INVALID: ProblemDef = def(
    "party-registration-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Registration number invalid",
    "A registration number is up to 64 letters, digits, spaces, dashes, dots or slashes.",
);
/// The currency is not a three-letter code.
pub const CURRENCY_INVALID: ProblemDef = def(
    "party-currency-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Currency invalid",
    "A currency is a three-letter code, such as EUR.",
);
/// The language is not a language code.
pub const LANGUAGE_INVALID: ProblemDef = def(
    "party-language-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Language invalid",
    "A language is a code such as en, de, id or ar.",
);
/// A further tax ID is malformed or fails its check.
pub const TAX_ID_INVALID: ProblemDef = def(
    "party-tax-id-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Tax ID invalid",
    "A tax ID needs a two-letter country, a kind (vat, tax or other) and a value that passes its check.",
);
/// The same tax ID is listed twice.
pub const TAX_ID_DUPLICATE: ProblemDef = def(
    "party-tax-id-duplicate",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Tax ID listed twice",
    "The same tax ID is listed more than once.",
);
/// The tags are too many, too long or contain separators.
pub const TAGS_INVALID: ProblemDef = def(
    "party-tags-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Tags invalid",
    "Up to 20 tags of 32 characters or fewer, without commas or semicolons.",
);
/// The notes are longer than the limit.
pub const NOTES_TOO_LONG: ProblemDef = def(
    "party-notes-too-long",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Notes too long",
    "Notes are at most 4000 characters.",
);
/// The default business unit is not an identifier.
pub const UNIT_INVALID: ProblemDef = def(
    "party-unit-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Business unit invalid",
    "A business unit is named by its identifier: letters, digits, dashes or underscores.",
);
/// A bank change came from an agent key.
pub const BANK_NEEDS_PERSON: ProblemDef = def(
    "party-bank-needs-person",
    StatusCode::FORBIDDEN,
    "Bank details need a person",
    "Bank details change only by a person, never by an agent key.",
);
/// A bank change has no reason.
pub const BANK_REASON_REQUIRED: ProblemDef = def(
    "party-bank-reason-required",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Bank change needs a reason",
    "A bank change needs a reason, kept in the history.",
);
/// The party does not exist in this company.
pub const NOT_FOUND: ProblemDef = def(
    "party-not-found",
    StatusCode::NOT_FOUND,
    "Party not found",
    "No party with this id exists in this company.",
);
/// The party has been merged into another.
pub const MERGED: ProblemDef = def(
    "party-merged",
    StatusCode::CONFLICT,
    "Party merged",
    "This party has been merged and cannot be changed.",
);
/// A party is merged into itself.
pub const MERGE_SELF: ProblemDef = def(
    "party-merge-self",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Cannot merge a party into itself",
    "Choose two different parties to merge.",
);
/// The VIES service did not answer.
pub const VIES_UNAVAILABLE: ProblemDef = def(
    "party-vies-unavailable",
    StatusCode::SERVICE_UNAVAILABLE,
    "VAT check unavailable",
    "The VIES service did not answer; try again later.",
);
/// The logo names no stored document of this company.
pub const LOGO_NOT_STORED: ProblemDef = def(
    "party-logo-not-stored",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Logo not found",
    "The logo must be an image this company has uploaded.",
);
/// The logo is not a PNG or JPEG.
pub const LOGO_NOT_IMAGE: ProblemDef = def(
    "party-logo-not-image",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Logo type refused",
    "A logo must be a PNG or JPEG image.",
);
/// The logo is larger than 2 MiB.
pub const LOGO_TOO_LARGE: ProblemDef = def(
    "party-logo-too-large",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Logo too large",
    "A logo may be at most 2 MiB.",
);
/// An import has rows with errors and was not run.
pub const IMPORT_INVALID: ProblemDef = def(
    "party-import-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Import has errors",
    "Some rows have errors. Check the file first; nothing was imported.",
);
/// The CSV file is malformed or has unknown columns.
pub const CSV_INVALID: ProblemDef = def(
    "party-csv-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "CSV invalid",
    "The CSV file has an unknown column, an unclosed quote or no header.",
);
/// An import has more rows than one batch takes.
pub const IMPORT_TOO_MANY: ProblemDef = def(
    "party-import-too-many-rows",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Too many rows",
    "An import takes at most 500 rows; split the file.",
);

/// A CSV cell that is not a number, a yes or no, or a JSON list.
pub const VALUE_INVALID: ProblemDef = def(
    "party-value-invalid",
    StatusCode::UNPROCESSABLE_ENTITY,
    "Value invalid",
    "A cell is not a number, a true or false, or valid JSON.",
);
/// The most rows one import takes.
pub const MAX_IMPORT_ROWS: usize = 500;
/// The most parties one export returns.
pub const MAX_EXPORT_ROWS: usize = 5000;

/// A refusal of `def` for `field`, with the server's English detail.
#[must_use]
pub fn refuse(def: &'static ProblemDef, field: &str, detail: impl Into<String>) -> CommandError {
    CommandError::Domain(
        Problem::new(def)
            .with_detail(detail)
            .with_extension("field", field.to_owned()),
    )
}

/// The slug of a party refusal, when the error is one (for tests and the CSV report).
#[must_use]
pub fn slug_of(error: &CommandError) -> Option<&'static str> {
    match error {
        CommandError::Domain(problem) => Some(problem.slug),
        _ => None,
    }
}

/// The further tax identifiers on a party: a VAT ID of another country, or a
/// tax number. `vat_id` on the party stays the primary VAT ID.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaxId {
    /// ISO 3166-1 alpha-2 country code.
    pub country: String,
    /// `vat`, `tax` or `other`.
    pub kind: String,
    /// The identifier. A VAT ID carries its country prefix.
    pub value: String,
}

/// A currency code: three letters, upper-cased. Blank is `None`.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_currency(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() == 3 && value.bytes().all(|b| b.is_ascii_alphabetic()) {
        Ok(Some(value.to_ascii_uppercase()))
    } else {
        Err(refuse(
            &CURRENCY_INVALID,
            "currency",
            "a currency is a three-letter code",
        ))
    }
}

/// A language code such as `en`, `de-DE` or `id`. Blank is `None`.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_language(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let (base, region) = match value.split_once('-') {
        Some((base, region)) => (base, Some(region)),
        None => (value, None),
    };
    let base_ok =
        (base.len() == 2 || base.len() == 3) && base.bytes().all(|b| b.is_ascii_alphabetic());
    let region_ok = region
        .is_none_or(|r| (2..=8).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric()));
    if base_ok && region_ok {
        // BCP 47 casing: the language lower-case, the region upper-case.
        let tagged = match region {
            Some(r) => format!("{}-{}", base.to_ascii_lowercase(), r.to_ascii_uppercase()),
            None => base.to_ascii_lowercase(),
        };
        Ok(Some(tagged))
    } else {
        Err(refuse(
            &LANGUAGE_INVALID,
            "language",
            "a language is a code such as en or de-DE",
        ))
    }
}

/// A registration number: up to 64 letters, digits, spaces, dashes, dots or slashes.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_registration(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let shape_ok = value.chars().count() <= 64
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '/' | '.'));
    if shape_ok {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &REGISTRATION_INVALID,
            "registration_number",
            "up to 64 letters, digits, spaces, dashes, dots or slashes",
        ))
    }
}

/// A website: `http://` or `https://`, then a host with a dot and no spaces.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_website(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    let host = rest.map(|r| r.split(['/', '?', '#']).next().unwrap_or(""));
    let ok = value.len() <= 255
        && !value.chars().any(char::is_whitespace)
        && host.is_some_and(|h| {
            h.contains('.') && !h.starts_with('.') && !h.ends_with('.') && !h.contains('@')
        });
    if ok {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &WEBSITE_INVALID,
            "website",
            "a website starts with http:// or https:// and names a domain",
        ))
    }
}

/// A phone number: 6 to 20 digits, with an optional leading `+` and spaces,
/// dashes, dots, slashes or brackets between them.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_phone(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let body = value.strip_prefix('+').unwrap_or(value);
    let chars_ok = body
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '-' | '.' | '/' | '(' | ')'));
    let digits = body.chars().filter(char::is_ascii_digit).count();
    if chars_ok && (6..=20).contains(&digits) {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &PHONE_INVALID,
            "phone",
            "6 to 20 digits, optionally starting with +",
        ))
    }
}

/// Whether an email address has one `@`, no spaces and a dotted domain.
#[must_use]
pub fn plausible_email(email: &str) -> bool {
    if email.chars().any(char::is_whitespace) {
        return false;
    }
    match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

/// The party's email address, checked. Blank is `None`.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_email(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if plausible_email(value) {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &EMAIL_INVALID,
            "email",
            "the email address does not look right",
        ))
    }
}

/// A default business unit: an identifier of letters, digits, dashes or underscores.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_unit(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let ok = value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &UNIT_INVALID,
            "default_unit",
            "letters, digits, dashes or underscores",
        ))
    }
}

/// Free-text notes: at most 4000 characters. Blank is `None`.
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_notes(raw: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() <= 4000 {
        Ok(Some(value.to_string()))
    } else {
        Err(refuse(
            &NOTES_TOO_LONG,
            "notes",
            "notes are at most 4000 characters",
        ))
    }
}

/// Tags: lower-cased, trimmed, de-duplicated; at most 20, each 32 characters
/// or fewer, without commas or semicolons (those separate tags in a CSV cell).
///
/// # Errors
/// A refusal with its party problem when the value fails its check.
pub fn checked_tags(tags: &[String]) -> Result<Vec<String>, CommandError> {
    let mut out: Vec<String> = Vec::new();
    for raw in tags {
        let tag = raw.trim().to_lowercase();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > 32 || tag.contains([',', ';']) {
            return Err(refuse(
                &TAGS_INVALID,
                "tags",
                format!("the tag {tag:?} is too long or has a separator"),
            ));
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > 20 {
        return Err(refuse(&TAGS_INVALID, "tags", "at most 20 tags"));
    }
    Ok(out)
}

/// Checks the further tax IDs against each country's pack, and against each
/// other and the primary VAT ID. The result stores VAT IDs with their prefix.
/// # Errors
/// A refusal with its party problem when a tax ID fails its check or repeats.
pub fn checked_tax_ids(
    ids: &[TaxId],
    primary_vat: Option<&str>,
) -> Result<Vec<TaxId>, CommandError> {
    if ids.len() > 10 {
        return Err(refuse(
            &TAX_ID_INVALID,
            "tax_ids",
            "at most 10 further tax IDs",
        ));
    }
    let primary = primary_vat.map(normalize_vat_id);
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let number = index + 1;
        let country = id.country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(refuse(
                &TAX_ID_INVALID,
                "tax_ids",
                format!("tax ID {number} needs a two-letter country code"),
            ));
        }
        let kind = id.kind.trim().to_ascii_lowercase();
        if !["vat", "tax", "other"].contains(&kind.as_str()) {
            return Err(refuse(
                &TAX_ID_INVALID,
                "tax_ids",
                format!("tax ID {number} kind must be vat, tax or other"),
            ));
        }
        let raw = id.value.trim().to_ascii_uppercase();
        if raw.is_empty() {
            return Err(refuse(
                &TAX_ID_INVALID,
                "tax_ids",
                format!("tax ID {number} needs a value"),
            ));
        }
        let value = if kind == "vat" {
            let full = if raw.starts_with(&country) {
                raw
            } else {
                format!("{country}{raw}")
            };
            let normalised = normalize_vat_id(&full);
            check_vat_id(&country, &normalised).map_err(|message| {
                refuse(
                    &TAX_ID_INVALID,
                    "tax_ids",
                    format!("tax ID {number}: {message}"),
                )
            })?;
            normalised
        } else {
            if raw.chars().count() > 32
                || !raw
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | '.' | ' '))
            {
                return Err(refuse(
                    &TAX_ID_INVALID,
                    "tax_ids",
                    format!("tax ID {number} has characters that are not allowed"),
                ));
            }
            raw
        };
        let key = format!("{country}:{kind}:{value}");
        if primary.as_deref() == Some(value.as_str()) || seen.contains(&key) {
            return Err(refuse(
                &TAX_ID_DUPLICATE,
                "tax_ids",
                format!("tax ID {number} is listed more than once"),
            ));
        }
        seen.push(key);
        out.push(TaxId {
            country,
            kind,
            value,
        });
    }
    Ok(out)
}

/// Maps a logo refusal from the documents crate onto its party problem.
#[must_use]
pub fn logo_refusal(refusal: &ledgers_documents::logo::LogoRefusal) -> CommandError {
    match refusal {
        ledgers_documents::logo::LogoRefusal::NotImage => {
            refuse(&LOGO_NOT_IMAGE, "logo_document_id", refusal.to_string())
        }
        ledgers_documents::logo::LogoRefusal::TooLarge { .. } => {
            refuse(&LOGO_TOO_LARGE, "logo_document_id", refusal.to_string())
        }
    }
}
