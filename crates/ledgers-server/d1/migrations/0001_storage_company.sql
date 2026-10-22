-- The single-company anchor: every domain table hangs off one company row
-- (docs/PLAN.md §1 — the open-source core is complete for one company).
-- Conventions for every Ledgers migration: ids are ULID TEXT, timestamps are
-- ISO 8601 TEXT in UTC, amounts are BIGINT minor units (never INTEGER:
-- Postgres INTEGER is 32-bit, while SQLite's type affinity maps BIGINT to a
-- 64-bit integer), and SQL stays portable — no AUTOINCREMENT, NOW(),
-- json_extract, FTS5 or backticks.
CREATE TABLE company (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    base_currency TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
