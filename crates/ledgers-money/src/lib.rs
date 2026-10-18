#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
//! Exact money for Ledgers — docs/PLAN.md §3 rule 1: **money is never a
//! float.**
//!
//! # The money rules
//!
//! - An [`Amount`] is integer minor units in its currency's own exponent
//!   (JPY 0, EUR 2, BHD 3, CLF 4 — see [`Currency::exponent`]), held as
//!   an [`i128`] and computed with checked integer arithmetic only. There
//!   is no `f64`/`f32` code path anywhere in this crate, and no cast from
//!   a float type.
//! - The storage boundary is [`i64`] (the database column is `INTEGER`):
//!   [`Amount::to_i64`] checks the range and returns
//!   [`MoneyError::Overflow`] rather than truncating;
//!   [`Amount::from_i64`] is infallible.
//! - Every fallible operation returns [`Result`]. Mixing currencies
//!   ([`MoneyError::CurrencyMismatch`]), overflow
//!   ([`MoneyError::Overflow`]) and malformed input
//!   ([`MoneyError::ParseError`]) are errors — nothing panics, nothing
//!   wraps, and there are no `std::ops` impls to bypass the checks.
//! - Rounding is explicit and named ([`Rounding`]): multiplying an amount
//!   by a [`Rate`] always states its mode.
//!
//! # Refreshing the currency table
//!
//! The currency table in [`currency`] was built from live ISO 4217 data
//! on **2026-10-18**:
//!
//! - Primary (active codes): Wikipedia, <https://en.wikipedia.org/wiki/ISO_4217>
//!   (the article's active-codes table; List One circulating currencies
//!   plus the funds/other codes, minus the four precious metals XAU, XAG,
//!   XPD, XPT).
//! - Secondary (cross-check of numerics and exponents, and the historic
//!   withdrawal record): the ISO 4217 maintenance-agency data mirrored at
//!   <https://raw.githubusercontent.com/datasets/currency-codes/master/data/codes-all.csv>.
//!
//! To refresh: fetch a new ISO List One (and the maintenance agency's
//! change history), diff it against the fixture in
//! `tests/iso4217_table.rs`, then update the table in `src/currency.rs`,
//! the fixture, `EXPECTED_ACTIVE` and both retrieval dates in the same
//! change. **Never delete a code**: a retired code moves from active to
//! historic so journal entries denominated in it keep parsing. The most
//! recent example is ZWG replacing ZWL in 2024 — ZWL is in the table with
//! `active = false`.
//!
//! # The `crypto` Cargo feature
//!
//! `crypto` is a reserved, empty slot for crypto assets with their own
//! minor-unit exponents (e.g. USDC = 6), planned for a later milestone
//! (docs/PLAN.md §6). Nothing is gated on it yet; it exists so downstream
//! manifests can name the capability early.
//!
//! # Examples
//!
//! Allocating a total without losing a minor unit — the parts always sum
//! to the input:
//!
//! ```
//! use ledgers_money::{Amount, Currency};
//!
//! let eur = Currency::from_code("EUR")?;
//! let total = Amount::from_major(100, eur)?;
//! let parts = total.allocate(&[1, 1, 1])?;
//! let sum = parts
//!     .iter()
//!     .try_fold(Amount::new(0, eur), |acc, p| acc.checked_add(*p))?;
//! assert_eq!(sum, total);
//! assert_eq!(parts[0].minor(), 3334); // the earliest part takes the remainder
//! # Ok::<(), ledgers_money::MoneyError>(())
//! ```
//!
//! Multiplying by an exact rate with a named rounding mode:
//!
//! ```
//! use ledgers_money::{Amount, Currency, Rate, Rounding};
//!
//! let jpy = Currency::from_code("JPY")?;
//! let net = Amount::new(1234, jpy); // ¥1,234
//! let tax = "0.1".parse::<Rate>()?;
//! let owed = net.mul_rate(tax, Rounding::HalfEven)?;
//! assert_eq!(owed.minor(), 123); // 123.4 rounds to 123
//! # Ok::<(), ledgers_money::MoneyError>(())
//! ```
//!
//! Parsing human input in a locale style back into exact minor units:
//!
//! ```
//! use ledgers_money::{Amount, Currency, DecimalStyle};
//!
//! let eur = Currency::from_code("EUR")?;
//! let a = Amount::parse_locale("1.234,56", eur, DecimalStyle::DE_DE)?;
//! assert_eq!(a.to_canonical_string(), "1234.56");
//! assert_eq!(a.to_i64()?, 123_456);
//! # Ok::<(), ledgers_money::MoneyError>(())
//! ```

pub mod allocation;
pub mod amount;
pub mod currency;
pub mod error;
pub mod rate;
pub mod style;

#[cfg(test)]
mod props;

pub use amount::Amount;
pub use currency::Currency;
pub use error::MoneyError;
pub use rate::{Rate, Rounding};
pub use style::DecimalStyle;
