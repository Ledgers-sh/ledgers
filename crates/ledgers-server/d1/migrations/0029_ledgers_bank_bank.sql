-- Bank accounts, statements and their lines, and bank rules (docs/PLAN.md
-- section 6, issues #26 and #48).
--
-- Conventions as in every Ledgers migration: ULID TEXT ids, ISO 8601 TEXT
-- dates bound from code, BIGINT minor units signed (inflow +, outflow -)
-- from the bank account's side, JSON in TEXT parsed in code, portable SQL
-- (no AUTOINCREMENT, NOW(), json_extract, FTS5). company_id carries no
-- cross-module FOREIGN KEY; the keys below are same-module only.
--
-- A bank account is linked to exactly one ledger account of the chart with
-- the `bank` tag: UNIQUE (company_id, ledger_account_number). A statement
-- line is one row, UNIQUE (company_id, fingerprint): re-importing the same
-- statement, or an overlapping one, cannot store a line twice. The status
-- is one of unmatched, matched, ignored; "suggested" is derived on read from
-- the suggestions, never stored. A line is matched to at most one posted
-- entry and at most one bill (the UNIQUEs below), as a backstop to the guard
-- statements the commands run.

CREATE TABLE bank_account (
    id                     TEXT NOT NULL PRIMARY KEY,
    company_id             TEXT NOT NULL,
    name                   TEXT NOT NULL,
    iban                   TEXT NOT NULL,
    bic                    TEXT,
    currency               TEXT NOT NULL,
    ledger_account_number  TEXT NOT NULL,
    opening_minor          BIGINT NOT NULL,
    opening_date           TEXT NOT NULL,
    csv_mapping            TEXT,
    archived_at            TEXT,
    created_by             TEXT NOT NULL,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL,
    UNIQUE (company_id, iban),
    UNIQUE (company_id, ledger_account_number)
);

CREATE TABLE bank_statement (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    account_id      TEXT NOT NULL REFERENCES bank_account (id),
    document_id     TEXT NOT NULL,
    ordinal         INTEGER NOT NULL,
    format          TEXT NOT NULL CHECK (format IN ('camt053', 'mt940', 'csv')),
    reference       TEXT,
    currency        TEXT NOT NULL,
    opening_date    TEXT,
    opening_minor   BIGINT,
    closing_date    TEXT,
    closing_minor   BIGINT,
    balance_checked INTEGER NOT NULL CHECK (balance_checked IN (0, 1)),
    line_count      INTEGER NOT NULL,
    created_by      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (company_id, account_id, document_id, ordinal)
);

CREATE TABLE bank_transaction (
    id                 TEXT NOT NULL PRIMARY KEY,
    company_id         TEXT NOT NULL,
    account_id         TEXT NOT NULL REFERENCES bank_account (id),
    statement_id       TEXT NOT NULL REFERENCES bank_statement (id),
    fingerprint        TEXT NOT NULL,
    group_key          TEXT NOT NULL,
    booking_date       TEXT NOT NULL,
    value_date         TEXT,
    minor              BIGINT NOT NULL,
    currency           TEXT NOT NULL,
    counterparty_name  TEXT,
    counterparty_iban  TEXT,
    remittance         TEXT NOT NULL,
    bank_reference     TEXT,
    status             TEXT NOT NULL CHECK (status IN ('unmatched', 'matched', 'ignored')),
    matched_entry_id   TEXT,
    matched_bill_id    TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    UNIQUE (company_id, fingerprint),
    UNIQUE (company_id, matched_entry_id),
    UNIQUE (company_id, matched_bill_id)
);
CREATE INDEX bank_transaction_account_idx ON bank_transaction (company_id, account_id, booking_date);
CREATE INDEX bank_transaction_group_idx ON bank_transaction (company_id, account_id, group_key);

CREATE TABLE bank_rule (
    id                  TEXT NOT NULL PRIMARY KEY,
    company_id          TEXT NOT NULL,
    name                TEXT NOT NULL,
    contains_text       TEXT,
    counterparty_iban   TEXT,
    amount_min          BIGINT,
    amount_max          BIGINT,
    account_number      TEXT NOT NULL,
    tax_code            TEXT,
    unit                TEXT,
    enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    proposed_by         TEXT NOT NULL,
    enabled_by          TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX bank_rule_company_idx ON bank_rule (company_id, created_at, id);
