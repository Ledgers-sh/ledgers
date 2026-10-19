-- Fiscal years and year-end close (docs/PLAN.md §6, `ledgers-ledger`
-- row: "... entries, periods ..."): the fiscal_year row a company's
-- periods hang from, and the opening balances a year-end close carries
-- into the next year. Conventions as in every Ledgers migration: no
-- ULID surrogate where a natural key exists ((company_id, label) is the
-- year's own identity), timestamps are ISO 8601 TEXT in UTC bound from
-- code, counters and money are BIGINT minor units, and the SQL stays
-- inside the harness's portable subset (no AUTOINCREMENT, NOW(),
-- json_extract, FTS5 or backticks).
--
-- starts_on / ends_on are inclusive "YYYY-MM-DD" dates: the fiscal year
-- covers [starts_on, ends_on], computed by crate::fiscal so every
-- engine agrees on the calendar (leap years included). The
-- fiscal_year_span_idx backs the overlap guard
-- (crate::period::open_fiscal_year), which refuses a new year whose
-- span touches an existing one at write time, inside the same
-- batch_atomic that inserts it.
--
-- year_end_closed_at stays NULL until the year-end close commits; the
-- close's final batch flips it in the same batch that writes the
-- opening balances and closes the last period, and the IS NULL guard
-- makes a concurrent second close fail the batch instead of doubling
-- the carry-forward.
--
-- year_end_report is the serialized close report (single-line
-- key=value text, crate::period::YearEndReport::serialize) powering the
-- idempotent rerun: once year_end_closed_at is set, closing again
-- parses the stored report back and writes nothing.
--
-- As with the account migration: NO cross-module foreign keys — module
-- application order is undefined, so company_id carries no FK to
-- company(id) and account_number carries none to account(number); the
-- command layer owns those references.
--
-- The accounting_period table this crate's periods live in belongs to
-- ledgers-db's migration (0001_guards.sql); its status domain grows the
-- value 'soft-closed' with this issue, documented beside the PeriodState
-- enum in crate::period rather than re-declared here, so this migration
-- stays self-contained.
CREATE TABLE fiscal_year (
    company_id            TEXT NOT NULL,
    label                 INTEGER NOT NULL,
    start_month           INTEGER NOT NULL,
    has_adjustment_period INTEGER NOT NULL DEFAULT 0,
    starts_on             TEXT NOT NULL,
    ends_on               TEXT NOT NULL,
    year_end_closed_at    TEXT,
    closing_entry_id      TEXT,
    year_end_report       TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    PRIMARY KEY (company_id, label)
);
CREATE INDEX fiscal_year_span_idx ON fiscal_year (company_id, starts_on, ends_on);
-- Opening balances for the labelled year, written by the year-end close
-- of the year before: one row per balance-sheet account with a non-zero
-- post-closing balance, amount_minor signed debit-positive in the
-- company base currency. Plain INSERT in the close batch; the primary
-- key is the unique backstop a crashed or doubled close trips.
CREATE TABLE fiscal_year_opening_balance (
    company_id     TEXT NOT NULL,
    fiscal_year    INTEGER NOT NULL,
    account_number TEXT NOT NULL,
    amount_minor   BIGINT NOT NULL,
    created_at     TEXT NOT NULL,
    PRIMARY KEY (company_id, fiscal_year, account_number)
);
