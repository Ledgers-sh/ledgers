//! Regression tests for uploaded statements that are hostile or truncated.
//!
//! Bank files are untrusted input. Each reader must return an error for these
//! inputs, never panic (a panic in the Worker is a denial of service). The
//! inputs are the ones that used to slice a `&str` inside a multibyte
//! character, plus truncated lines and a byte-order mark.

use ledgers_bank::parse::{self, ColumnRef, CsvMapping};
use ledgers_bank::statement::Format;

fn csv_mapping() -> CsvMapping {
    CsvMapping {
        delimiter: ";".into(),
        skip_rows: 0,
        has_header: true,
        decimal: ".".into(),
        date_format: "YYYY-MM-DD".into(),
        booking_date: ColumnRef::from("date"),
        value_date: None,
        amount: Some("amount".into()),
        debit: None,
        credit: None,
        currency: None,
        counterparty_name: None,
        counterparty_iban: None,
        remittance: vec!["text".into()],
        bank_reference: None,
    }
}

fn mt940(text: &str) -> Result<usize, ledgers_bank::statement::ParseError> {
    parse::parse(Format::Mt940, text.as_bytes(), None, "EUR").map(|s| s.len())
}

/// Multibyte characters where a tag, a balance mark, a transaction code or a
/// subfield key used to be sliced by byte index. Each must be an error.
#[test]
fn mt940_multibyte_at_structural_positions_is_an_error_not_a_panic() {
    let hostile = [
        // `:61:` after the value date: `NTRF` code ends inside `é`.
        ":20:X\n:60F:C260301EUR0,00\n:61:2603020302D12,50NTRé\n:62F:C260301EUR0,00\n",
        // `:61:` with a multibyte amount or mark.
        ":20:X\n:60F:C260301EUR0,00\n:61:2603020302Dé12,50NTRF\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:260302é302D12,50NTRF\n:62F:C260301EUR0,00\n",
        // Balances: the mark and the date/currency split inside `é`.
        ":20:X\n:60F:Cé260301EUR0,00\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C26030é1EUR0,00\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EURé,00\n:62F:C260301EUR0,00\n",
    ];
    for text in hostile {
        assert!(mt940(text).is_err(), "expected an error for {text:?}");
    }
}

/// A line whose tag is not a tag (a multibyte character where a `:NN:` digit
/// belongs) is not a field: it continues the field before it. It must not
/// panic, whichever way it is read.
#[test]
fn mt940_multibyte_tag_lines_do_not_panic() {
    for line in [":1é:x", ":é1:x", ":Aü:", ":1€:x", ":€€:x", ":12É:x", ":é"] {
        let text = format!(":20:X\n:60F:C260301EUR0,00\n{line}\n:62F:C260301EUR0,00\n");
        let _ = mt940(&text);
    }
}

/// Lines cut off part-way through a field must be errors.
#[test]
fn mt940_truncated_lines_are_errors() {
    let hostile = [
        ":20:X\n:60F:\n",
        ":20:X\n:60F:C\n",
        ":20:X\n:60F:C2603\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:26030203\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:2603020302D\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:2603020302D12,50N\n:62F:C260301EUR0,00\n",
        ":20:X\n:60F:C260301EUR0,00\n:61:2603020302D12,50NTR\n:62F:C260301EUR0,00\n",
        ":2",
        ":",
        "{4:",
        ":20",
    ];
    for text in hostile {
        assert!(mt940(text).is_err(), "expected an error for {text:?}");
    }
}

/// A byte-order mark is not content: the statement must still read.
#[test]
fn mt940_with_a_byte_order_mark_reads() {
    let text = "\u{feff}:20:X\n:60F:C260301EUR1,00\n:62F:C260301EUR1,00\n";
    assert_eq!(mt940(text), Ok(1));
    let block =
        "\u{feff}{1:F01DEUTDEFFAXXX}{4:\n:20:X\n:60F:C260301EUR1,00\n:62F:C260301EUR1,00\n-}";
    assert_eq!(mt940(block), Ok(1));
}

#[test]
fn csv_with_a_byte_order_mark_reads_its_header() {
    let text = "\u{feff}date;amount;text\n2026-03-02;-12.50;Coffee\n";
    let statement = parse::parse(Format::Csv, text.as_bytes(), Some(&csv_mapping()), "EUR")
        .expect("a BOM before the header is not part of the column name");
    assert_eq!(statement[0].lines.len(), 1);
    assert_eq!(statement[0].lines[0].minor, -1_250);
}

#[test]
fn csv_multibyte_and_truncated_rows_are_errors() {
    let mapping = csv_mapping();
    let hostile = [
        "date;amount;text\n2026-03-0é;-1.00;x\n",
        "date;amount;text\n2026-03-02;-1.0é;x\n",
        "date;amount;text\n2026-03-02\n",
        "date;amount;text\n2026-03-02;",
        "date;amount;text\n20é6-03-02;1.00;x\n",
        "date;amount;text\né",
        "date;am",
    ];
    for text in hostile {
        let result = parse::parse(Format::Csv, text.as_bytes(), Some(&mapping), "EUR");
        assert!(result.is_err(), "expected an error for {text:?}");
    }
}

#[test]
fn camt053_truncated_and_multibyte_input_is_an_error_not_a_panic() {
    let hostile = [
        "<BkToCstmrStmt><Stmt><Id>é",
        "<BkToCstmrStmt><Stmt><Ntry><Amt Ccy=\"EU",
        "<BkToCstmrStmt><Stmt><Ntry><Amt Ccy=\"EUR\">1é",
        "\u{feff}<BkToCstmrStmt>",
        "<BkToCstmrStmt>é",
    ];
    for text in hostile {
        let result = parse::parse(Format::Camt053, text.as_bytes(), None, "EUR");
        assert!(result.is_err(), "expected an error for {text:?}");
    }
}

/// The format sniffer reads at most 4096 bytes. A window that ends inside a
/// multibyte character must still detect the format from the whole characters
/// before it.
#[test]
fn detect_reads_a_window_that_ends_inside_a_multibyte_character() {
    let mut text = String::from("<BkToCstmrStmt><!--");
    while text.len() < 8192 {
        text.push('é');
    }
    text.push_str("--></BkToCstmrStmt>");
    let bytes = text.as_bytes();
    assert!(
        std::str::from_utf8(&bytes[..4096]).is_err(),
        "the window must cut a character for this test to mean anything"
    );
    assert_eq!(parse::detect(bytes), Some(Format::Camt053));
}

#[test]
fn detect_refuses_bytes_that_are_not_utf8() {
    let bytes = [0xff_u8, 0xfe, b':', b'2', b'0', b':'];
    assert_eq!(parse::detect(&bytes), None);
}
