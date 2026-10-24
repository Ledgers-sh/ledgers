-- Widens connector_connection.connector to Stripe, and adds the ledger idempotency table.
--
-- SQLite cannot alter a CHECK, so the table is rebuilt: copy the rows into a
-- new table, drop the old one, rename the new one into its name. The child
-- tables (connector_run, connector_invoice, connector_split, ...) refer to
-- connector_connection by name, so they stay linked to the rebuilt table.
--
-- The rebuild needs foreign keys deferred, not switched off: D1 ignores
-- `PRAGMA foreign_keys = OFF`, and an immediate check fails on DROP TABLE while
-- child rows still refer to the parent. `defer_foreign_keys` holds the check
-- to the end of the transaction, when every child refers to a row again.
-- This file runs as one transaction, natively (the migration runner) and on
-- D1 (the batch), so both apply the same statements.
PRAGMA defer_foreign_keys = true;

CREATE TABLE connector_connection_v2 (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connector       TEXT NOT NULL CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale', 'aws', 'vercel', 'gcp', 'azure', 'supabase', 'openai', 'openrouter', 'stripe')),
    name            TEXT NOT NULL,
    config_json     TEXT NOT NULL,
    default_unit    TEXT NOT NULL,
    is_demo         INTEGER NOT NULL DEFAULT 0 CHECK (is_demo IN (0, 1)),
    last_sync_at    TEXT,
    deleted_at      TEXT,
    created_by      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (company_id, name)
);

INSERT INTO connector_connection_v2 (id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, deleted_at, created_by, created_at, updated_at)
SELECT id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, deleted_at, created_by, created_at, updated_at
FROM connector_connection;

DROP TABLE connector_connection;

ALTER TABLE connector_connection_v2 RENAME TO connector_connection;

PRAGMA defer_foreign_keys = false;

-- connector_ledger_line: one row per provider transaction a ledger connector posted
-- as a journal entry. Keyed by the provider's id per company: a repeat sync finds the
-- row and posts nothing. entry_id names the posted journal entry.
CREATE TABLE connector_ledger_line (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    provider_id     TEXT NOT NULL,
    entry_id        TEXT NOT NULL,
    posting_date    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (company_id, provider_id)
);
