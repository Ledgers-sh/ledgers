-- The Postgres override of the journal migration: the same three
-- tables, byte-for-byte the same DDL as the portable set in
-- migrations/sqlite/0002_journal.sql (that DDL is portable — TEXT,
-- INTEGER, BIGINT, UNIQUE, REFERENCES all run on both), and the same
-- four immutability triggers in Postgres's own trigger dialect:
-- SQLite's `CREATE TRIGGER … BEGIN SELECT RAISE(ABORT, …); END;` is
-- not valid Postgres, so the refusal lives in a plpgsql function the
-- triggers call. Same rule, same error text, second line of defence
-- against every path that is not the posting batch.
--
-- Why an override set at all: the Postgres migration runner prefers a
-- module's postgres set when one exists (cratefield-adapter-postgres
-- select_set) and applies the sqlite set to Postgres only when it
-- passes the portable lint — which the sqlite file does, but its
-- trigger syntax would still fail on arrival. The runner cannot know
-- that, so this file says it explicitly.
--
-- 0001_accounts.sql in this directory is the portable accounts DDL,
-- verbatim: the set must be complete and contiguous from 0001 on its
-- own (assert_migration_set).
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

-- The refusals. RAISE EXCEPTION aborts the statement (and the
-- transaction the batch runner opened), exactly as RAISE(ABORT, …)
-- does on SQLite.
CREATE FUNCTION ledgers_journal_line_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'journal_line is append-only (docs/PLAN.md §3 rule 3)';
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION ledgers_journal_entry_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'posted journal_entry is immutable (docs/PLAN.md §3 rule 3)';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER journal_line_no_update
BEFORE UPDATE ON journal_line
FOR EACH ROW EXECUTE FUNCTION ledgers_journal_line_immutable();

CREATE TRIGGER journal_line_no_delete
BEFORE DELETE ON journal_line
FOR EACH ROW EXECUTE FUNCTION ledgers_journal_line_immutable();

CREATE TRIGGER journal_entry_no_update_when_posted
BEFORE UPDATE ON journal_entry
FOR EACH ROW WHEN (OLD.status = 'posted')
EXECUTE FUNCTION ledgers_journal_entry_immutable();

CREATE TRIGGER journal_entry_no_delete_when_posted
BEFORE DELETE ON journal_entry
FOR EACH ROW WHEN (OLD.status = 'posted')
EXECUTE FUNCTION ledgers_journal_entry_immutable();
