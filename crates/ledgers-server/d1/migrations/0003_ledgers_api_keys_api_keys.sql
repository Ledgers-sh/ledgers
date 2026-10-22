-- The harness API-key table (cratefield-core `ApiKeys`): key prefixes and
-- SHA-256 hashes of the secrets only, never the secrets. Ledgers agent keys
-- are rows of this table, keyed by the key subject (the agent key id).
CREATE TABLE IF NOT EXISTS api_keys (
    prefix TEXT PRIMARY KEY,
    secret_hash TEXT NOT NULL,
    namespace TEXT NOT NULL,
    subject TEXT NOT NULL,
    scopes TEXT NOT NULL,
    mode TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT
);
