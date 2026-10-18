# 0003: Rates are fixed-point i128 with 12 fractional digits

Date: 2026-10-18. Status: accepted.

## Context

Rates in Ledgers are only ever ratios applied to amounts: FX conversion
and tax percentages. They are never user-entered general-purpose
decimals, so they need no full arbitrary-precision decimal type. The
repository's correctness rules (docs/PLAN.md §3) demand checked
arithmetic everywhere — an operation that cannot be represented is an
`Err`, never a panic or a silent wrap — and the money crate carries
`#![deny(clippy::float_arithmetic)]`, so a binary floating-point rate is
out of the question. The obvious off-the-shelf candidate, `rust_decimal`,
multiplies and divides through operator overloads that panic on overflow,
would be this crate's first non-dev runtime dependency, and adds wasm
payload the worker does not need.

## Decision

`Rate` is an `i128` scaled by 10¹² (`SCALE = 1_000_000_000_000`): range
±1.7 × 10²⁶, precision 10⁻¹² — ample for FX and tax rates. Multiplying
an `Amount` by a `Rate` goes through `Amount::mul_rate(amount, rate,
rounding)` with an explicit named rounding mode; the intermediate product
`minor × scaled` is `checked_mul`'d (documented bound: minor units up to
`i64::MAX` in magnitude with rates up to 10⁶ fit `i128` comfortably;
beyond that the result is `Err(MoneyError::Overflow)`).

## Consequences

- No external decimal dependency: the crate stays runtime-dependency-free
  apart from `thiserror`, deterministic, and small for wasm.
- All rate arithmetic is checked and returns `Result`; nothing panics.
- Converting a rate with more than 12 fractional digits is a parse error,
  not a silent rounding.
- Rounding acts on the magnitude with the sign reapplied, so every mode
  is symmetric around zero and easy to reason about for credit notes.
