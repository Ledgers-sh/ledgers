-- Widens connector_connection.connector to the cloud connectors (aws, vercel).
--
-- SQLite cannot alter a CHECK, so the table is rebuilt. This migration runs
-- outside a transaction, because the foreign-key pragma can only be changed
-- outside one; the statements are guarded so a repeat is harmless:
-- CREATE ... IF NOT EXISTS, and INSERT OR IGNORE. The child tables refer to
-- connector_connection by name, so they stay linked to the rebuilt table.
-- Known limit: a crash between the DROP and the RENAME leaves the rows in
-- connector_connection_v2 and needs the RENAME run by hand.
PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS connector_connection_v2 (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connector       TEXT NOT NULL CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale', 'aws', 'vercel')),
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

INSERT OR IGNORE INTO connector_connection_v2 (id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, deleted_at, created_by, created_at, updated_at)
SELECT id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, deleted_at, created_by, created_at, updated_at
FROM connector_connection;

DROP TABLE IF EXISTS connector_connection;

ALTER TABLE connector_connection_v2 RENAME TO connector_connection;

PRAGMA foreign_keys = ON;
