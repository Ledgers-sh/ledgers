-- The journal tables (docs/PLAN.md §6, `ledgers-ledger` row): the
-- journals themselves, their entries and the entries' lines. Numbering
-- is gapless per (journal_id, fiscal_year) through the numbering
-- counter in ledgers-db (a rolled-back post never leaves a hole — the
-- bump and the insert share one batch); the balance invariant is
-- re-proved at write time by SQL guards over journal_line (ADR 0004);
-- immutability is the schema's own second line of defence, below.
--
-- Conventions as in 0001_accounts.sql: ULID TEXT ids, ISO 8601 TEXT
-- dates bound from code (no NOW()), BIGINT minor units signed debit +
-- and credit −, JSON in TEXT columns parsed in code (no json_extract),
-- portable SQL. company_id again carries NO cross-module FOREIGN KEY
-- (module application order is undefined); journal_entry → journal and
-- journal_line → journal_entry are same-module and do carry keys.
CREATE TABLE journal (
    id         TEXT PRIMARY KEY,
    company_id TEXT NOT NULL,
    code       TEXT NOT NULL,   -- 1..=16 of [A-Z0-9-], validated in code
    kind       TEXT NOT NULL,   -- general|sales|purchases|bank|cash|opening|closing
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (company_id, code)
);

CREATE TABLE journal_entry (
    id               TEXT PRIMARY KEY,
    company_id       TEXT NOT NULL,
    journal_id       TEXT NOT NULL REFERENCES journal(id),
    fiscal_year      INTEGER NOT NULL,
    number           BIGINT NOT NULL,
    status           TEXT NOT NULL,   -- 'posted'; there is no draft state
    document_date    TEXT NOT NULL,   -- the business event's date
    posting_date     TEXT NOT NULL,   -- the books' date; names the period
    tax_point_date   TEXT NOT NULL,   -- VAT becomes chargeable here
    vat_period       TEXT NOT NULL,   -- 'YYYY-MM' of the tax point date
    recorded_at      TEXT NOT NULL,   -- set once, at posting
    description      TEXT NOT NULL,
    source_documents TEXT NOT NULL,   -- JSON array of link strings
    reverses         TEXT REFERENCES journal_entry(id),
    base_currency    TEXT NOT NULL,
    UNIQUE (journal_id, fiscal_year, number)
);
-- The UNIQUE above is the numbering backstop: the counter's UPDATE is
-- the serialisation point (ledgers-db numbering), and any path that
-- bypasses it lands here as a unique violation — the one failure class
-- the retry policy retries. `reverses` points at the entry this one
-- corrects; the reverse relation is read back with a query
-- (SELECT id FROM journal_entry WHERE reverses = ?), never a mutable
-- reversed_by column — no UPDATE path may exist for a posted entry.

CREATE TABLE journal_line (
    entry_id       TEXT NOT NULL REFERENCES journal_entry(id),
    line_no        INTEGER NOT NULL,  -- 1-based, in order
    account_number TEXT NOT NULL,     -- references account(company_id, number) in-code
    base_currency  TEXT NOT NULL,
    base_minor     BIGINT NOT NULL,   -- signed: debit +, credit −
    tx_currency    TEXT NOT NULL,     -- the base code when the line has no FX leg
    tx_minor       BIGINT NOT NULL,   -- signed, same convention
    rate_scaled    TEXT,              -- fx rate × 10^12, as decimal text; NULL when no FX
    tax_code       TEXT,
    dimensions     TEXT NOT NULL,     -- JSON object of string → string
    memo           TEXT,
    PRIMARY KEY (entry_id, line_no)
);
CREATE INDEX journal_line_entry_idx ON journal_line (entry_id);
CREATE INDEX journal_entry_journal_idx ON journal_entry (journal_id, fiscal_year, number);

-- Immutability, the second line of defence. The first is the API: no
-- function in this crate updates or deletes a posted entry or any
-- line, and corrections are reversals (docs/PLAN.md §3 rule 3). These
-- triggers are what enforces that against every OTHER path — a stray
-- db.execute, a future crate, a hand-run SQL console:
--
--   journal_line is append-only, unconditionally: lines are never
--   updated and never deleted, full stop.
--
--   journal_entry rows are immutable once posted; the WHEN clause (not
--   a bare trigger) is deliberate — it documents that it is the POSTED
--   state that is protected, so a future non-posted lifecycle (a
--   period-close working state, say) is not silently locked out of its
--   own machinery by this file.
--
-- SQLite spelling: RAISE(ABORT, …) inside a BEFORE trigger, which the
-- batch runner reports as a failed statement and the whole batch rolls
-- back. This file is the portable set; migrations/postgres/0002_journal.sql
-- carries the plpgsql equivalent for the Postgres override set (SQLite
-- trigger syntax is a dialect of its own).
CREATE TRIGGER journal_line_no_update
BEFORE UPDATE ON journal_line
BEGIN
    SELECT RAISE(ABORT, 'journal_line is append-only (docs/PLAN.md §3 rule 3)');
END;

CREATE TRIGGER journal_line_no_delete
BEFORE DELETE ON journal_line
BEGIN
    SELECT RAISE(ABORT, 'journal_line is append-only (docs/PLAN.md §3 rule 3)');
END;

CREATE TRIGGER journal_entry_no_update_when_posted
BEFORE UPDATE ON journal_entry
FOR EACH ROW WHEN OLD.status = 'posted'
BEGIN
    SELECT RAISE(ABORT, 'posted journal_entry is immutable (docs/PLAN.md §3 rule 3)');
END;

CREATE TRIGGER journal_entry_no_delete_when_posted
BEFORE DELETE ON journal_entry
FOR EACH ROW WHEN OLD.status = 'posted'
BEGIN
    SELECT RAISE(ABORT, 'posted journal_entry is immutable (docs/PLAN.md §3 rule 3)');
END;
