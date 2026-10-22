-- People sessions (issue #12). One row per signed-in browser session. The
-- cookie carries an opaque random id; only its SHA-256 (id_hash) is stored, so
-- a copy of this table cannot be replayed as cookies. Times are epoch seconds.
-- idle_expires_at slides on use and never passes absolute_expires_at.
CREATE TABLE IF NOT EXISTS people_session (
    id_hash TEXT PRIMARY KEY NOT NULL,
    person_id TEXT NOT NULL,
    email TEXT,
    roles TEXT NOT NULL,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    idle_expires_at INTEGER NOT NULL,
    absolute_expires_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS people_session_absolute_expiry
    ON people_session (absolute_expires_at);
