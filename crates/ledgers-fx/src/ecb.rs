//! ECB parser (behind `ecb` feature).

#[cfg(feature = "ecb")]
pub mod parser {
    use crate::error::FxError;
    use ledgers_money::{Currency, Rate};
    use time::Date;

    /// One ECB rate.
    #[derive(Clone, Debug)]
    pub struct EcbRate {
        pub date: Date,
        pub quote: String,
        pub rate: Rate,
    }

    /// Parse ECB XML: `<Cube time='YYYY-MM-DD'><Cube currency='...' rate='...'/></Cube>`
    ///
    /// # Errors
    /// [`FxError::UnknownCurrency`] for unknown currency codes, or [`FxError::InvalidRate`] for invalid rates.
    pub fn parse_ecb_xml(xml: &str) -> Result<Vec<EcbRate>, FxError> {
        let mut rates = Vec::new();
        let mut date: Option<Date> = None;

        for part in xml.split("<Cube ") {
            if let Some(time_part) = part.strip_prefix("time='")
                && let Some(end) = time_part.find('\'')
            {
                let date_str = &time_part[..end];
                if let Ok(d) = Date::parse(
                    date_str,
                    &time::format_description::well_known::Iso8601::DEFAULT,
                ) {
                    date = Some(d);
                }
            } else if let Some(curr_part) = part.strip_prefix("currency='")
                && let Some(end) = curr_part.find('\'')
            {
                let currency = &curr_part[..end];

                if Currency::from_code(currency).is_err() {
                    return Err(FxError::UnknownCurrency {
                        code: currency.to_owned(),
                    });
                }

                if let Some(rate_start) = curr_part.find("rate='")
                    && let Some(rate_end) = curr_part[rate_start + 6..].find('\'')
                {
                    let rate_str = &curr_part[rate_start + 6..][..rate_end];
                    let rate = Rate::parse(rate_str).map_err(|_| FxError::InvalidRate {
                        reason: format!("bad rate: {rate_str}"),
                    })?;

                    if let Some(d) = date {
                        rates.push(EcbRate {
                            date: d,
                            quote: currency.to_owned(),
                            rate,
                        });
                    }
                }
            }
        }

        Ok(rates)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_valid() {
            let xml = "<Cube time='2026-10-20'><Cube currency='USD' rate='1.0812'/><Cube currency='GBP' rate='0.8345'/></Cube>";
            let rates = parse_ecb_xml(xml).unwrap();
            assert_eq!(rates.len(), 2);
        }

        #[test]
        fn test_bad_currency() {
            let xml = "<Cube time='2026-10-20'><Cube currency='ZZZ' rate='1.0'/></Cube>";
            assert!(matches!(
                parse_ecb_xml(xml),
                Err(FxError::UnknownCurrency { .. })
            ));
        }

        #[test]
        fn test_bad_rate() {
            let xml = "<Cube time='2026-10-20'><Cube currency='USD' rate='xx'/></Cube>";
            assert!(matches!(
                parse_ecb_xml(xml),
                Err(FxError::InvalidRate { .. })
            ));
        }
    }
}
