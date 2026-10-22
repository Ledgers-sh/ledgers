//! ISO 20022 `camt.053` (bank-to-customer statement) reader.
//!
//! Elements are matched by their path from the root, so the same local name
//! inside a balance (`Bal/Amt`) and inside an entry (`Ntry/Amt`) is never
//! confused. Entries whose status is pending (`PDNG`) are skipped: only
//! booked lines are imported. Counterparty fields follow the sign: an
//! outflow names the creditor, an inflow the debtor.

use super::{finish_currency, indicator_sign};
use crate::money::{Decimal, parse_minor};
use crate::statement::{BalanceRow, Format, ParseError, ParsedLine, ParsedStatement};
use quick_xml::Reader;
use quick_xml::events::Event;

/// Reads every `Stmt` of a camt.053 document.
///
/// # Errors
/// [`ParseError::Malformed`] for broken XML or a missing amount, currency,
/// sign or date; [`ParseError::Unsupported`] for other camt message types.
pub fn parse(text: &str) -> Result<Vec<ParsedStatement>, ParseError> {
    if !text.contains("camt.053") && !text.contains("BkToCstmrStmt") {
        if text.contains("BkToCstmrAcctRpt") {
            return Err(ParseError::Unsupported(
                "camt.052 intraday reports are not imported yet".to_owned(),
            ));
        }
        return Err(ParseError::Malformed("not a camt.053 document".to_owned()));
    }
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut walker = Walker::default();
    let mut path: Vec<String> = Vec::new();
    let mut pending_ccy: Option<String> = None;
    // Text can arrive in pieces (around entity references such as `&amp;`),
    // so it is gathered per element and trimmed once, when the element changes.
    let mut buffer = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                flush(&mut walker, &path, &mut buffer, pending_ccy.as_deref());
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                match name.as_str() {
                    "Stmt" => walker.begin_statement(),
                    "Ntry" => walker.begin_entry(),
                    "Bal" => walker.begin_balance(),
                    "Amt" => pending_ccy = attribute(&start, "Ccy"),
                    _ => {}
                }
                path.push(name);
            }
            Ok(Event::End(end)) => {
                flush(&mut walker, &path, &mut buffer, pending_ccy.as_deref());
                path.pop();
                match String::from_utf8_lossy(end.local_name().as_ref()).as_ref() {
                    "Ntry" => walker.end_entry()?,
                    "Bal" => walker.end_balance()?,
                    "Stmt" => walker.end_statement()?,
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                let raw = text
                    .decode()
                    .map_err(|err| ParseError::Malformed(err.to_string()))?;
                let value = quick_xml::escape::unescape(&raw)
                    .map_err(|err| ParseError::Malformed(err.to_string()))?;
                buffer.push_str(&value);
            }
            Ok(Event::GeneralRef(reference)) => {
                let name = reference
                    .decode()
                    .map_err(|err| ParseError::Malformed(err.to_string()))?;
                let resolved = match name.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    other => {
                        return Err(ParseError::Malformed(format!(
                            "entity &{other}; is not allowed"
                        )));
                    }
                };
                buffer.push_str(resolved);
            }
            Ok(Event::Eof) => {
                flush(&mut walker, &path, &mut buffer, pending_ccy.as_deref());
                break;
            }
            Ok(_) => {}
            Err(err) => return Err(ParseError::Malformed(format!("xml: {err}"))),
        }
    }
    if walker.statements.is_empty() {
        return Err(ParseError::Malformed(
            "no statement in the document".to_owned(),
        ));
    }
    Ok(walker.statements)
}

/// Hands the text gathered for the current element to the walker, trimmed.
fn flush(walker: &mut Walker, path: &[String], buffer: &mut String, ccy: Option<&str>) {
    let value = buffer.trim().to_owned();
    buffer.clear();
    if !value.is_empty() {
        walker.text(path, &value, ccy);
    }
}

fn attribute(start: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    start.attributes().flatten().find_map(|attr| {
        (attr.key.local_name().as_ref() == key.as_bytes())
            .then(|| String::from_utf8_lossy(&attr.value).into_owned())
    })
}

fn ends(path: &[String], suffix: &[&str]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .map(String::as_str)
            .eq(suffix.iter().copied())
}

fn inside(path: &[String], name: &str) -> bool {
    path.iter().any(|segment| segment == name)
}

#[derive(Default)]
struct BalanceBuilder {
    kind: Option<String>,
    amount: Option<String>,
    sign: Option<String>,
    date: Option<String>,
    currency: Option<String>,
}

#[derive(Default)]
struct EntryBuilder {
    amount: Option<String>,
    sign: Option<String>,
    status: Option<String>,
    booking_date: Option<String>,
    value_date: Option<String>,
    account_servicer_ref: Option<String>,
    entry_ref: Option<String>,
    remittance: String,
    creditor_name: Option<String>,
    debtor_name: Option<String>,
    creditor_iban: Option<String>,
    debtor_iban: Option<String>,
    currency: Option<String>,
}

#[derive(Default)]
struct Walker {
    statements: Vec<ParsedStatement>,
    statement_iban: Option<String>,
    statement_id: Option<String>,
    balance: Option<BalanceBuilder>,
    entry: Option<EntryBuilder>,
    opening: Option<BalanceRow>,
    closing: Option<BalanceRow>,
    currency: Option<String>,
    lines: Vec<ParsedLine>,
}

impl Walker {
    fn begin_statement(&mut self) {
        self.statement_iban = None;
        self.statement_id = None;
        self.opening = None;
        self.closing = None;
        self.currency = None;
        self.lines = Vec::new();
    }

    fn begin_entry(&mut self) {
        self.entry = Some(EntryBuilder::default());
    }

    fn begin_balance(&mut self) {
        self.balance = Some(BalanceBuilder::default());
    }

    fn text(&mut self, path: &[String], value: &str, ccy: Option<&str>) {
        if let Some(entry) = self.entry.as_mut() {
            // Only the entry's own elements: a balance never lands here.
            let suffix = |names: &[&str]| ends(path, names);
            if suffix(&["Ntry", "Amt"]) {
                entry.amount = Some(value.to_owned());
                entry.currency = ccy.map(str::to_owned).or(entry.currency.take());
            } else if suffix(&["Ntry", "CdtDbtInd"]) {
                entry.sign = Some(value.to_owned());
            } else if suffix(&["Ntry", "Sts"]) || suffix(&["Ntry", "Sts", "Cd"]) {
                entry.status = Some(value.to_owned());
            } else if suffix(&["Ntry", "BookgDt", "Dt"]) {
                entry.booking_date = Some(value.to_owned());
            } else if suffix(&["Ntry", "BookgDt", "DtTm"]) {
                entry.booking_date = value.get(..10).map(str::to_owned);
            } else if suffix(&["Ntry", "ValDt", "Dt"]) {
                entry.value_date = Some(value.to_owned());
            } else if suffix(&["Ntry", "AcctSvcrRef"]) {
                entry.account_servicer_ref = Some(value.to_owned());
            } else if suffix(&["Ntry", "NtryRef"]) {
                entry.entry_ref = Some(value.to_owned());
            } else if suffix(&["RmtInf", "Ustrd"]) {
                if !entry.remittance.is_empty() {
                    entry.remittance.push(' ');
                }
                entry.remittance.push_str(value);
            } else if suffix(&["Cdtr", "Nm"]) {
                entry.creditor_name.get_or_insert_with(|| value.to_owned());
            } else if suffix(&["Dbtr", "Nm"]) {
                entry.debtor_name.get_or_insert_with(|| value.to_owned());
            } else if suffix(&["CdtrAcct", "Id", "IBAN"]) {
                entry.creditor_iban.get_or_insert_with(|| value.to_owned());
            } else if suffix(&["DbtrAcct", "Id", "IBAN"]) {
                entry.debtor_iban.get_or_insert_with(|| value.to_owned());
            }
            return;
        }
        if let Some(balance) = self.balance.as_mut() {
            if ends(path, &["Bal", "Tp", "CdOrPrtry", "Cd"]) {
                balance.kind = Some(value.to_owned());
            } else if ends(path, &["Bal", "Amt"]) {
                balance.amount = Some(value.to_owned());
                balance.currency = ccy.map(str::to_owned);
            } else if ends(path, &["Bal", "CdtDbtInd"]) {
                balance.sign = Some(value.to_owned());
            } else if ends(path, &["Bal", "Dt", "Dt"]) {
                balance.date = Some(value.to_owned());
            } else if ends(path, &["Bal", "Dt", "DtTm"]) {
                balance.date = value.get(..10).map(str::to_owned);
            }
            return;
        }
        if ends(path, &["Stmt", "Id"]) {
            self.statement_id = Some(value.to_owned());
        } else if ends(path, &["Acct", "Id", "IBAN"]) && !inside(path, "Ntry") {
            self.statement_iban = Some(value.to_owned());
        }
    }

    fn end_entry(&mut self) -> Result<(), ParseError> {
        let Some(entry) = self.entry.take() else {
            return Ok(());
        };
        // Pending entries are not booked yet: they are skipped, not refused.
        if entry.status.as_deref() == Some("PDNG") {
            return Ok(());
        }
        let currency = entry
            .currency
            .clone()
            .ok_or_else(|| ParseError::Malformed("an entry has no currency".to_owned()))?;
        let exponent = finish_currency(&currency)?;
        let amount = entry
            .amount
            .as_deref()
            .ok_or_else(|| ParseError::Malformed("an entry has no amount".to_owned()))?;
        let magnitude =
            parse_minor(amount, Decimal::Point, exponent).map_err(ParseError::Malformed)?;
        let sign = indicator_sign(entry.sign.as_deref(), "CRDT", "DBIT")?;
        let outflow = sign < 0;
        let booking_date = entry
            .booking_date
            .clone()
            .or_else(|| entry.value_date.clone())
            .ok_or_else(|| ParseError::Malformed("an entry has no booking date".to_owned()))?;
        let (name, iban) = if outflow {
            (entry.creditor_name, entry.creditor_iban)
        } else {
            (entry.debtor_name, entry.debtor_iban)
        };
        let bank_reference = entry.account_servicer_ref.or(entry.entry_ref);
        self.lines.push(ParsedLine {
            booking_date,
            value_date: entry.value_date,
            minor: sign * magnitude,
            currency: currency.clone(),
            counterparty_name: name,
            counterparty_iban: iban.map(|iban| crate::iban::normalise(&iban)),
            remittance: entry.remittance,
            bank_reference,
        });
        self.note_currency(&currency)
    }

    fn end_balance(&mut self) -> Result<(), ParseError> {
        let Some(balance) = self.balance.take() else {
            return Ok(());
        };
        let kind = balance.kind.clone().unwrap_or_default();
        let currency = balance
            .currency
            .clone()
            .ok_or_else(|| ParseError::Malformed("a balance has no currency".to_owned()))?;
        let exponent = finish_currency(&currency)?;
        let amount = balance
            .amount
            .as_deref()
            .ok_or_else(|| ParseError::Malformed("a balance has no amount".to_owned()))?;
        let magnitude =
            parse_minor(amount, Decimal::Point, exponent).map_err(ParseError::Malformed)?;
        let sign = indicator_sign(balance.sign.as_deref(), "CRDT", "DBIT")?;
        let date = balance
            .date
            .ok_or_else(|| ParseError::Malformed("a balance has no date".to_owned()))?;
        let row = BalanceRow {
            date,
            minor: sign * magnitude,
        };
        match kind.as_str() {
            "OPBD" | "PRCD" => self.opening = Some(row),
            "CLBD" => self.closing = Some(row),
            // Available and intraday balances do not bound the statement.
            _ => {}
        }
        self.note_currency(&currency)
    }

    fn end_statement(&mut self) -> Result<(), ParseError> {
        let currency = self
            .currency
            .clone()
            .ok_or_else(|| ParseError::Malformed("the statement has no currency".to_owned()))?;
        let statement = ParsedStatement {
            format: Format::Camt053,
            iban: self
                .statement_iban
                .take()
                .map(|iban| crate::iban::normalise(&iban)),
            reference: self.statement_id.take(),
            currency,
            opening: self.opening.take(),
            closing: self.closing.take(),
            lines: std::mem::take(&mut self.lines),
        };
        self.statements.push(statement);
        Ok(())
    }

    fn note_currency(&mut self, currency: &str) -> Result<(), ParseError> {
        match &self.currency {
            None => {
                self.currency = Some(currency.to_owned());
                Ok(())
            }
            Some(known) if known == currency => Ok(()),
            Some(known) => Err(ParseError::Malformed(format!(
                "the statement mixes {known} and {currency}"
            ))),
        }
    }
}
