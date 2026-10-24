-- Widens connector_connection.connector to the cloud connectors (stripe).
ALTER TABLE connector_connection DROP CONSTRAINT connector_connection_connector_check;
ALTER TABLE connector_connection ADD CONSTRAINT connector_connection_connector_check
    CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale', 'aws', 'vercel', 'gcp', 'azure', 'supabase', 'openai', 'openrouter', 'stripe'));

-- connector_ledger_line: one row per provider transaction a ledger connector posted
-- as a journal entry (see the SQLite migration).
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
