//! CSV import for purchase orders and goods receipts.

use crate::MatchError;
use crate::model::{GoodsReceipt, PoLine, PurchaseOrder, Quantity, ReceiptLine};
use ledgers_money::{Amount, Currency};
use std::collections::BTreeMap;

/// Parse price string in major units (e.g., "12.50") to Amount.
///
/// # Errors
///
/// Returns error on invalid format, overflow, or parse failure.
fn parse_price_string(s: &str, currency: Currency) -> Result<Amount, MatchError> {
    // Try to parse as integer first (no decimal point)
    if !s.contains('.') && !s.contains(',') {
        let major: i128 = s
            .parse()
            .map_err(|_| MatchError::InvalidPrice(format!("not a valid number: {s}")))?;
        return Amount::from_major(major, currency)
            .map_err(|e| MatchError::InvalidPrice(e.to_string()));
    }

    // Parse decimal: split on . or , and reassemble
    let normalized = s.replace(',', ".");
    let parts: Vec<&str> = normalized.split('.').collect();

    if parts.len() != 2 {
        return Err(MatchError::InvalidPrice(format!(
            "invalid decimal format: {s}"
        )));
    }

    let major: i128 = parts[0]
        .parse()
        .map_err(|_| MatchError::InvalidPrice(format!("invalid major part: {}", parts[0])))?;

    let frac_str = parts[1];
    // The fractional part determines the minor units
    // For EUR (exponent 2): ".50" = 50 minor units
    // We pad or truncate to the currency's exponent
    let exponent = usize::try_from(currency.exponent()).unwrap_or(usize::MAX);

    let mut frac_padded = frac_str.to_string();
    if frac_padded.len() < exponent {
        frac_padded.push_str(&"0".repeat(exponent - frac_padded.len()));
    } else if frac_padded.len() > exponent {
        frac_padded.truncate(exponent);
    }

    let frac: i128 = frac_padded
        .parse()
        .map_err(|_| MatchError::InvalidPrice(format!("invalid fractional part: {frac_str}")))?;

    let total_minor = major
        .checked_mul(currency.minor_per_major())
        .ok_or(MatchError::InvalidPrice(
            "overflow in major part".to_string(),
        ))?
        .checked_add(frac)
        .ok_or(MatchError::InvalidPrice(
            "overflow in fractional part".to_string(),
        ))?;

    Ok(Amount::new(total_minor, currency))
}

/// Parse a CSV line with support for double-quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // Toggle quote state
                if in_quotes && chars.peek() == Some(&'"') {
                    // Escaped quote
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

/// Import POs from CSV: `po_number`, `vendor`, `currency`, `line`, `item`, `description`, `quantity`, `unit_price`[, `billed`].
/// One row per line item; rows grouped by `po_number`.
///
/// # Errors
///
/// Returns error on invalid CSV format, missing columns, invalid currency, or malformed quantities/prices.
pub fn import_purchase_orders(csv: &str) -> Result<Vec<PurchaseOrder>, MatchError> {
    let lines: Vec<&str> = csv.lines().collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    // Parse header
    let header_fields = parse_csv_line(lines[0]);
    if header_fields.len() < 8 {
        return Err(MatchError::Csv {
            row: 1,
            message: "expected at least 8 columns (po_number, vendor, currency, line, item, description, quantity, unit_price)".to_string(),
        });
    }

    let mut pos_map: BTreeMap<String, (String, Currency, Vec<PoLine>)> = BTreeMap::new();

    for (idx, line_str) in lines[1..].iter().enumerate() {
        let row_num = idx + 2; // 1-indexed, skip header
        let fields = parse_csv_line(line_str);

        if fields.is_empty() || fields.iter().all(String::is_empty) {
            continue; // Skip empty lines
        }

        if fields.len() < 8 {
            return Err(MatchError::Csv {
                row: row_num,
                message: format!("expected at least 8 fields, got {}", fields.len()),
            });
        }

        let po_number = fields[0].clone();
        let vendor = fields[1].clone();
        let currency_code = &fields[2];
        let currency = Currency::from_code(currency_code).map_err(|_| MatchError::Csv {
            row: row_num,
            message: format!("invalid currency code: {currency_code}"),
        })?;

        let line_num: u32 = fields[3].parse().map_err(|_| MatchError::Csv {
            row: row_num,
            message: format!("line number must be integer: {}", fields[3]),
        })?;

        let item = fields[4].clone();
        let description = fields[5].clone();

        let quantity = Quantity::parse(&fields[6]).map_err(|_| MatchError::Csv {
            row: row_num,
            message: format!("invalid quantity: {}", fields[6]),
        })?;

        // Parse unit price: it may be like "12.00" (major units with decimals)
        // We need to parse it as a string with the amount library
        let price_str = &fields[7];
        let unit_price = parse_price_string(price_str, currency).map_err(|e| MatchError::Csv {
            row: row_num,
            message: format!("invalid unit price: {} ({})", fields[7], e),
        })?;

        let billed = if fields.len() > 8 {
            Quantity::parse(&fields[8]).map_err(|_| MatchError::Csv {
                row: row_num,
                message: format!("invalid billed quantity: {}", fields[8]),
            })?
        } else {
            Quantity::from_thousandths(0)
        };

        let entry = pos_map
            .entry(po_number.clone())
            .or_insert_with(|| (vendor.clone(), currency, Vec::new()));

        if entry.1 != currency {
            return Err(MatchError::Csv {
                row: row_num,
                message: format!(
                    "vendor/currency mismatch in document {}: {} vs {}",
                    po_number,
                    entry.1.code(),
                    currency.code()
                ),
            });
        }

        entry.2.push(PoLine {
            line: line_num,
            item,
            description,
            quantity,
            unit_price,
            billed,
        });
    }

    let mut pos = Vec::new();
    for (number, (vendor, currency, lines)) in pos_map {
        pos.push(PurchaseOrder {
            number,
            vendor,
            currency,
            lines,
        });
    }

    Ok(pos)
}

/// Import receipts from CSV: `receipt_number`, `vendor`, `po_number`, `po_line`, `quantity`.
///
/// # Errors
///
/// Returns error on invalid CSV format or missing columns.
pub fn import_goods_receipts(csv: &str) -> Result<Vec<GoodsReceipt>, MatchError> {
    let lines: Vec<&str> = csv.lines().collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let header_fields = parse_csv_line(lines[0]);
    if header_fields.len() < 5 {
        return Err(MatchError::Csv {
            row: 1,
            message:
                "expected at least 5 columns (receipt_number, vendor, po_number, po_line, quantity)"
                    .to_string(),
        });
    }

    let mut receipts_map: BTreeMap<String, (String, String, Vec<ReceiptLine>)> = BTreeMap::new();

    for (idx, line_str) in lines[1..].iter().enumerate() {
        let row_num = idx + 2;
        let fields = parse_csv_line(line_str);

        if fields.is_empty() || fields.iter().all(String::is_empty) {
            continue;
        }

        if fields.len() < 5 {
            return Err(MatchError::Csv {
                row: row_num,
                message: format!("expected at least 5 fields, got {}", fields.len()),
            });
        }

        let receipt_number = fields[0].clone();
        let vendor = fields[1].clone();
        let po_number = fields[2].clone();

        let po_line: u32 = fields[3].parse().map_err(|_| MatchError::Csv {
            row: row_num,
            message: format!("po_line must be integer: {}", fields[3]),
        })?;

        let quantity = Quantity::parse(&fields[4]).map_err(|_| MatchError::Csv {
            row: row_num,
            message: format!("invalid quantity: {}", fields[4]),
        })?;

        let entry = receipts_map
            .entry(receipt_number.clone())
            .or_insert_with(|| (vendor.clone(), po_number.clone(), Vec::new()));

        entry.2.push(ReceiptLine { po_line, quantity });
    }

    let mut receipts = Vec::new();
    for (number, (vendor, po_number, lines)) in receipts_map {
        receipts.push(GoodsReceipt {
            number,
            vendor,
            po_number,
            lines,
        });
    }

    Ok(receipts)
}
