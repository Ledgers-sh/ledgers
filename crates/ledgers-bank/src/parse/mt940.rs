//! SWIFT MT940 customer statement reader.
//!
//! The message is split into tagged fields: a line starting `:NN:` or
//! `:NNA:` opens a field, any other line continues the field before it. A
//! `:20:` opens a statement, `:60F:` or `:60M:` is the opening balance,
//! `:61:` a line, `:86:` the information for the line before it, and
//! `:62F:` the closing balance. Reversals (`RC`, `RD`) flip the sign.
//!
//! The `:86:` field is read in its structured German form (`?20`..`?29`
//! purpose, `?31` account, `?32`/`?33` name) when it carries subfields, and
//! as plain text otherwise. Its text is data: nothing in it is acted on.

use super::{finish_currency, iban_like};
use crate::iban;
use crate::money::{Decimal, parse_minor};
use crate::statement::{BalanceRow, Format, ParseError, ParsedLine, ParsedStatement};
use std::collections::BTreeMap;

/// One tagged field: the tag without colons and the value (continuation
/// lines joined with newlines).
struct Field {
    tag: String,
    value: String,
}

/// Reads every statement (`:20:` block) of an MT940 message.
///
/// # Errors
/// [`ParseError::Malformed`] when a balance, a line or a date does not have
/// the MT940 shape, or the message has no statement.
pub fn parse(text: &str) -> Result<Vec<ParsedStatement>, ParseError> {
    let mut statements = Vec::new();
    let mut current: Option<Builder> = None;
    for field in split_fields(message_body(text)) {
        match field.tag.as_str() {
            "20" => {
                if let Some(done) = current.take() {
                    statements.push(done.finish()?);
                }
                current = Some(Builder::new(field.value.trim()));
            }
            "25" => {
                let account = field.value.trim();
                require(&mut current, "25")?.iban =
                    iban_like(account).then(|| iban::normalise(account));
            }
            "28C" | "28" => {
                require(&mut current, "28C")?.reference = Some(field.value.trim().to_owned());
            }
            "60F" | "60M" => {
                require(&mut current, "60F")?.opening = Some(raw_balance(&field.value, "60")?);
            }
            "61" => {
                let builder = require(&mut current, "61")?;
                builder.lines.push(raw_line(&field.value)?);
            }
            "86" => {
                let builder = require(&mut current, "86")?;
                if let Some(last) = builder.lines.last_mut() {
                    apply_information(last, &field.value);
                }
            }
            "62F" => {
                require(&mut current, "62F")?.closing = Some(raw_balance(&field.value, "62")?);
            }
            // Other tags (:62M:, :64:, :65:, :34F:, ...) do not change the lines.
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        statements.push(done.finish()?);
    }
    if statements.is_empty() {
        return Err(ParseError::Malformed(
            "no :20: statement in the message".to_owned(),
        ));
    }
    Ok(statements)
}

/// The text after the SWIFT `{4:` block opener and before its `-}`, or the
/// whole input when there is no block header.
fn message_body(text: &str) -> &str {
    let body = match text.find("{4:") {
        Some(start) => &text[start + 3..],
        None => text,
    };
    match body.find("\n-}") {
        Some(end) => &body[..end],
        None => body,
    }
}

fn split_fields(body: &str) -> Vec<Field> {
    let mut fields: Vec<Field> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(tag_end) = field_tag_end(line) {
            fields.push(Field {
                tag: line[1..tag_end].to_owned(),
                value: line[tag_end + 1..].to_owned(),
            });
        } else if let Some(last) = fields.last_mut() {
            last.value.push('\n');
            last.value.push_str(line);
        }
    }
    fields
}

/// Where a `:NN:` or `:NNA:` tag's closing colon sits, when the line opens
/// with one.
fn field_tag_end(line: &str) -> Option<usize> {
    let rest = line.strip_prefix(':')?;
    let end = rest.find(':')?;
    let tag = rest.get(..end)?;
    // `get` refuses a split inside a multibyte character: such a tag is not a tag.
    let valid = (2..=3).contains(&tag.len())
        && tag
            .get(..2)
            .is_some_and(|digits| digits.chars().all(|c| c.is_ascii_digit()))
        && tag
            .get(2..)
            .is_some_and(|letters| letters.chars().all(|c| c.is_ascii_uppercase()));
    valid.then_some(end + 1)
}

/// `text[range]`, or a malformed-field error when the range does not fall on
/// character boundaries or runs past the end. Untrusted text never panics.
fn cut<'a, I>(text: &'a str, range: I, field: &str) -> Result<&'a str, ParseError>
where
    I: std::slice::SliceIndex<str, Output = str>,
{
    text.get(range)
        .ok_or_else(|| ParseError::Malformed(format!("{field} is cut short or malformed")))
}

fn require<'a>(current: &'a mut Option<Builder>, tag: &str) -> Result<&'a mut Builder, ParseError> {
    current
        .as_mut()
        .ok_or_else(|| ParseError::Malformed(format!("field :{tag}: appears before :20:")))
}

/// `YYMMDD` to `YYYY-MM-DD`, in the 2000s.
fn yymmdd(text: &str) -> Result<String, ParseError> {
    let bytes = text.as_bytes();
    if bytes.len() != 6 || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(ParseError::Malformed(format!("bad date {text:?}")));
    }
    let month: u32 = text[2..4].parse().unwrap_or(0);
    let day: u32 = text[4..6].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(ParseError::Malformed(format!("bad date {text:?}")));
    }
    Ok(format!("20{}-{}-{}", &text[..2], &text[2..4], &text[4..6]))
}

/// A balance as the field states it, before the currency's exponent is known.
#[derive(Clone, Debug)]
struct RawBalance {
    date: String,
    sign: i64,
    amount: String,
    currency: String,
}

fn raw_balance(value: &str, which: &str) -> Result<RawBalance, ParseError> {
    let flat = value.replace('\n', "");
    let (sign, rest) = if let Some(rest) = flat.strip_prefix('C') {
        (1i64, rest)
    } else if let Some(rest) = flat.strip_prefix('D') {
        (-1i64, rest)
    } else {
        return Err(ParseError::Malformed(format!(
            ":{which}: balance has no C or D mark"
        )));
    };
    let too_short = || ParseError::Malformed(format!(":{which}: balance is too short"));
    let date = rest.get(..6).ok_or_else(too_short)?;
    let currency = rest.get(6..9).ok_or_else(too_short)?;
    let amount = rest.get(9..).ok_or_else(too_short)?;
    Ok(RawBalance {
        date: yymmdd(date)?,
        sign,
        currency: currency.to_owned(),
        amount: amount.to_owned(),
    })
}

/// One `:61:` line, before its currency is known and its information attached.
#[derive(Clone, Debug)]
struct RawLine {
    booking_date: String,
    value_date: String,
    sign: i64,
    amount: String,
    bank_reference: Option<String>,
    remittance: String,
    counterparty_name: Option<String>,
    counterparty_iban: Option<String>,
}

fn raw_line(value: &str) -> Result<RawLine, ParseError> {
    let first = value.split('\n').next().unwrap_or("");
    let bytes = first.as_bytes();
    if bytes.len() < 8 || !bytes[..6].iter().all(u8::is_ascii_digit) {
        return Err(ParseError::Malformed(
            ":61: line has no value date".to_owned(),
        ));
    }
    let value_date = yymmdd(cut(first, ..6, ":61:")?)?;
    let mut pos = 6;
    let entry_mmdd = match bytes.get(pos..pos + 4) {
        Some(four) if four.iter().all(u8::is_ascii_digit) => {
            pos += 4;
            Some(cut(first, 6..pos, ":61:")?.to_owned())
        }
        _ => None,
    };
    let rest = cut(first, pos.., ":61:")?;
    let (sign, mark_len) = if rest.starts_with("RC") {
        (-1i64, 2)
    } else if rest.starts_with("RD") {
        (1, 2)
    } else if rest.starts_with('C') {
        (1, 1)
    } else if rest.starts_with('D') {
        (-1, 1)
    } else {
        return Err(ParseError::Malformed(
            ":61: line has no C or D mark".to_owned(),
        ));
    };
    pos += mark_len;
    // An optional funds-code letter sits between the mark and the amount.
    if cut(first, pos.., ":61:")?
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
    {
        pos += 1;
    }
    let amount_len = cut(first, pos.., ":61:")?
        .find(|c: char| !(c.is_ascii_digit() || c == ','))
        .unwrap_or(first.len() - pos);
    let amount = cut(first, pos..pos + amount_len, ":61:")?.to_owned();
    pos += amount_len;
    // The transaction type is `N` plus three characters; the references follow.
    let references = cut(first, pos.., ":61:")?
        .strip_prefix('N')
        .and_then(|tail| tail.get(3..))
        .ok_or_else(|| ParseError::Malformed(":61: line has no transaction type".to_owned()))?;
    let (customer, bank) = match references.split_once("//") {
        Some((customer, bank)) => (customer.trim(), Some(bank.trim())),
        None => (references.trim(), None),
    };
    let customer = (!customer.is_empty() && customer != "NONREF").then(|| customer.to_owned());
    let bank = bank
        .filter(|text| !text.is_empty() && *text != "NONREF")
        .map(str::to_owned);
    let booking_date = match entry_mmdd {
        // The entry date names the month; its year is the value date's year,
        // moved on when the entry falls in a later year than the value date.
        Some(mmdd) => {
            let value_year: i32 = value_date[..4].parse().unwrap_or(2000);
            let value_month: u32 = value_date[5..7].parse().unwrap_or(0);
            let month: u32 = mmdd[..2].parse().unwrap_or(0);
            let year = value_year + i32::from(month < value_month);
            format!("{year:04}-{}-{}", &mmdd[..2], &mmdd[2..])
        }
        None => value_date.clone(),
    };
    Ok(RawLine {
        booking_date,
        value_date,
        sign,
        amount,
        bank_reference: bank.or(customer),
        remittance: String::new(),
        counterparty_name: None,
        counterparty_iban: None,
    })
}

/// Attaches a `:86:` field to the line it describes.
fn apply_information(line: &mut RawLine, value: &str) {
    let Some(fields) = subfields(value) else {
        value
            .replace('\n', " ")
            .trim()
            .clone_into(&mut line.remittance);
        return;
    };
    line.remittance = (20u8..=29)
        .filter_map(|key| fields.get(&key).map(|text| text.trim()))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let name: Vec<&str> = [32u8, 33]
        .iter()
        .filter_map(|key| fields.get(key).map(|text| text.trim()))
        .filter(|text| !text.is_empty())
        .collect();
    line.counterparty_name = (!name.is_empty()).then(|| name.join(" "));
    line.counterparty_iban = fields
        .get(&31)
        .map(|text| text.trim())
        .filter(|text| iban_like(text))
        .map(iban::normalise);
}

/// The `?NN` subfields of an information field, when it has them.
fn subfields(value: &str) -> Option<BTreeMap<u8, String>> {
    let flat: String = value.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if !flat.starts_with('?') {
        return None;
    }
    let mut fields = BTreeMap::new();
    for segment in flat.split('?').skip(1) {
        // `split_at_checked` yields nothing when the key would split a character.
        let Some((digits, text)) = segment.split_at_checked(2) else {
            continue;
        };
        if let Ok(key) = digits.parse::<u8>() {
            fields.insert(key, text.to_owned());
        }
    }
    Some(fields)
}

/// A statement being assembled from its fields.
struct Builder {
    transaction_ref: String,
    reference: Option<String>,
    iban: Option<String>,
    opening: Option<RawBalance>,
    closing: Option<RawBalance>,
    lines: Vec<RawLine>,
}

impl Builder {
    fn new(transaction_ref: &str) -> Self {
        Builder {
            transaction_ref: transaction_ref.to_owned(),
            reference: None,
            iban: None,
            opening: None,
            closing: None,
            lines: Vec::new(),
        }
    }

    fn finish(self) -> Result<ParsedStatement, ParseError> {
        let balances: Vec<&RawBalance> = self.opening.iter().chain(self.closing.iter()).collect();
        let currency = balances
            .first()
            .map(|balance| balance.currency.clone())
            .ok_or_else(|| {
                ParseError::Malformed(format!(
                    "statement {} has no balances",
                    self.transaction_ref
                ))
            })?;
        if balances.iter().any(|balance| balance.currency != currency) {
            return Err(ParseError::Malformed(format!(
                "statement {} mixes currencies in its balances",
                self.transaction_ref
            )));
        }
        let exponent = finish_currency(&currency)?;
        let balance_row = |raw: &RawBalance| -> Result<BalanceRow, ParseError> {
            let magnitude = parse_minor(&raw.amount, Decimal::Comma, exponent)
                .map_err(ParseError::Malformed)?;
            Ok(BalanceRow {
                date: raw.date.clone(),
                minor: raw.sign * magnitude,
            })
        };
        let opening = self.opening.as_ref().map(balance_row).transpose()?;
        let closing = self.closing.as_ref().map(balance_row).transpose()?;
        let mut lines = Vec::with_capacity(self.lines.len());
        for raw in self.lines {
            let magnitude = parse_minor(&raw.amount, Decimal::Comma, exponent)
                .map_err(ParseError::Malformed)?;
            lines.push(ParsedLine {
                booking_date: raw.booking_date,
                value_date: Some(raw.value_date),
                minor: raw.sign * magnitude,
                currency: currency.clone(),
                counterparty_name: raw.counterparty_name,
                counterparty_iban: raw.counterparty_iban,
                remittance: raw.remittance,
                bank_reference: raw.bank_reference,
            });
        }
        Ok(ParsedStatement {
            format: Format::Mt940,
            iban: self.iban,
            reference: self.reference.or(Some(self.transaction_ref)),
            currency,
            opening,
            closing,
            lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_statement_reads_balances_and_signed_lines() {
        let text = ":20:STMT1\n:25:DE89370400440532013000\n:28C:1/1\n\
:60F:C260301EUR1000,00\n:61:2603020302D12,50NTRFNONREF\n:86:Coffee\n\
:61:260305C100,00NTRFREF1//BANK1\n:86:Refund\n:62F:C260305EUR1087,50\n";
        let statements = parse(text).expect("parses");
        assert_eq!(statements.len(), 1);
        let s = &statements[0];
        assert_eq!(s.iban.as_deref(), Some("DE89370400440532013000"));
        assert_eq!(s.currency, "EUR");
        assert_eq!(s.opening.as_ref().unwrap().minor, 100_000);
        assert_eq!(s.closing.as_ref().unwrap().minor, 108_750);
        assert_eq!(s.lines[0].minor, -1_250);
        assert_eq!(s.lines[0].booking_date, "2026-03-02");
        assert_eq!(s.lines[0].bank_reference, None);
        assert_eq!(s.lines[1].minor, 10_000);
        assert_eq!(s.lines[1].bank_reference.as_deref(), Some("BANK1"));
        assert_eq!(s.lines[1].remittance, "Refund");
        assert_eq!(
            crate::statement::check_balances(s).unwrap(),
            crate::statement::BalanceCheck::Checked
        );
    }

    #[test]
    fn reversals_flip_the_sign_and_structured_information_is_read() {
        let text = ":20:X\n:60F:C260301EUR0,00\n\
:61:2603100310RD5,00NTRF//R1\n\
:86:?00NTRF?10123?20Invoice 4711?21Rechnung?31DE89370400440532013000?32Halden Supplies\
?33GmbH\n:62F:C260310EUR5,00\n";
        let s = &parse(text).expect("parses")[0];
        let line = &s.lines[0];
        assert_eq!(line.minor, 500);
        assert_eq!(line.remittance, "Invoice 4711 Rechnung");
        assert_eq!(
            line.counterparty_name.as_deref(),
            Some("Halden Supplies GmbH")
        );
        assert_eq!(
            line.counterparty_iban.as_deref(),
            Some("DE89370400440532013000")
        );
        assert_eq!(line.bank_reference.as_deref(), Some("R1"));
    }

    #[test]
    fn several_statements_in_one_message_are_all_read() {
        let text = ":20:A\n:60F:C260301EUR1,00\n:62F:C260301EUR1,00\n:20:B\n:60F:C260302EUR1,00\n:62F:C260302EUR1,00\n";
        assert_eq!(parse(text).expect("parses").len(), 2);
    }

    #[test]
    fn a_header_block_is_skipped() {
        let text =
            "{1:F01DEUTDEFFAXXX0000000000}{4:\n:20:H\n:60F:C260301EUR1,00\n:62F:C260301EUR1,00\n-}";
        assert_eq!(
            parse(text).expect("parses")[0].reference.as_deref(),
            Some("H")
        );
    }

    #[test]
    fn a_bad_date_or_mark_is_malformed() {
        assert!(parse(":20:X\n:60F:C261301EUR1,00\n:62F:C261301EUR1,00\n").is_err());
        assert!(parse(":20:X\n:60F:Q260301EUR1,00\n:62F:Q260301EUR1,00\n").is_err());
        assert!(parse(":60F:C260301EUR1,00\n").is_err());
    }
}
