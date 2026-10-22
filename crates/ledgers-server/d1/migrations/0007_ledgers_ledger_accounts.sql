-- The chart of accounts (docs/PLAN.md §6, `ledgers-ledger` row): one
-- `account` table per company, holding the tree the journals post against.
-- Conventions as in every Ledgers migration: ULID TEXT ids elsewhere, but
-- the account's own natural key is its number (TEXT, fixed width per
-- chart so lexical order is numeric order); timestamps are ISO 8601 TEXT
-- in UTC; the SQL stays inside the harness's portable subset (no
-- AUTOINCREMENT, NOW(), json_extract, FTS5 or backticks).
--
-- company_id follows the company-scoped pattern of ledgers-storage's
-- migrations but deliberately carries NO cross-module FOREIGN KEY to
-- company(id): module application order is undefined, so this table may
-- be migrated before the storage foundation exists. Integrity — that the
-- company exists, and that parent_number points at an account of the
-- same company — is enforced in the command layer (crate::commands),
-- which refuses to create an account under a parent it cannot find.
CREATE TABLE account (
    company_id     TEXT NOT NULL,
    number         TEXT NOT NULL,
    name_key       TEXT NOT NULL,
    account_type   TEXT NOT NULL,
    normal_balance TEXT NOT NULL,
    currency       TEXT,
    parent_number  TEXT,
    tags           TEXT NOT NULL,   -- space-separated kebab-case tags
    archived       INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (company_id, number)
);
-- There is deliberately no `postable` column: postable-vs-heading is
-- DERIVED from the tree (an account with children is a heading, a leaf is
-- postable), recomputed on read via the parent_number index below. A
-- stored flag could go stale the moment a child is added under a former
-- leaf, and an agent reading a stale heading as postable is unsafe — so
-- the flag is never stored, only derived (crate::account::AccountEntry).
CREATE INDEX account_parent_idx ON account (company_id, parent_number);
