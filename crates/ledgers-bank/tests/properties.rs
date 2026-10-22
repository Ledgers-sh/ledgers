//! Property tests: parsers never panic on arbitrary bytes, decimal text
//! round-trips through the currency exponent, and IBANs with generated check
//! digits always validate.

use ledgers_bank::iban;
use ledgers_bank::money::{Decimal, parse_minor};
use ledgers_bank::parse::{self, CsvMapping};
use ledgers_bank::statement::Format;
use proptest::prelude::*;

fn mapping() -> CsvMapping {
    CsvMapping {
        delimiter: ";".into(),
        skip_rows: 0,
        has_header: true,
        decimal: ".".into(),
        date_format: "YYYY-MM-DD".into(),
        booking_date: "date".into(),
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic_any_reader(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse::parse(Format::Camt053, &bytes, None, "EUR");
        let _ = parse::parse(Format::Mt940, &bytes, None, "EUR");
        let _ = parse::parse(Format::Csv, &bytes, Some(&mapping()), "EUR");
        let _ = parse::detect(&bytes);
    }

    #[test]
    fn arbitrary_text_never_panics_the_readers(text in "\\PC{0,400}") {
        let _ = parse::parse(Format::Mt940, text.as_bytes(), None, "EUR");
        let _ = parse::parse(Format::Camt053, text.as_bytes(), None, "EUR");
        let _ = parse::parse(Format::Csv, text.as_bytes(), Some(&mapping()), "EUR");
    }

    #[test]
    fn decimal_text_round_trips_at_every_exponent(minor in -10_000_000_000i64..10_000_000_000i64, exp in 0u32..=3) {
        let scale = 10i64.pow(exp);
        let major = minor / scale;
        let frac = (minor % scale).unsigned_abs();
        let text = if exp == 0 {
            format!("{major}")
        } else {
            format!("{major}.{frac:0width$}", width = exp as usize)
        };
        // A negative amount below one major unit keeps its sign on the text.
        let text = if minor < 0 && major == 0 && exp > 0 { format!("-0.{frac:0width$}", width = exp as usize) } else { text };
        prop_assert_eq!(parse_minor(&text, Decimal::Point, exp), Ok(minor));
    }

    #[test]
    fn generated_ibans_validate_and_one_changed_digit_never_does(bban in "[0-9]{18}", country in prop::sample::select(vec!["DE", "NL", "FR", "AT", "CH"])) {
        // The generated IBAN is validated for every country; the length check
        // inside `validate` decides which of them are well-formed.
        let generated = iban::with_check_digits(country, &bban);
        // The generated IBAN has the right length only for its country; the
        // property is checked for the countries whose length matches the BBAN.
        if let Ok(valid) = iban::validate(&generated) {
            prop_assert_eq!(valid, generated.clone());
            let index = generated.len() - 1;
            let original = generated.as_bytes()[index];
            let replacement = if original == b'0' { b'1' } else { b'0' };
            let mut bytes = generated.clone().into_bytes();
            bytes[index] = replacement;
            let changed = String::from_utf8(bytes).unwrap();
            prop_assert!(iban::validate(&changed).is_err());
        }
    }
}
