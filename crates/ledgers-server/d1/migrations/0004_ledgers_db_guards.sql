-- The guard-statement foundation (docs/PLAN.md §2, "Atomic writes":
-- `batch_atomic` only, no interactive transactions): the small tables
-- every write-time invariant needs, so gapless numbering, period
-- closure, agent limits and the audit chain all fail INSIDE the batch
-- that would violate them. The pattern is the one the harness's own
-- usage metering set (cratefield_core::Usage::consume_statement): a
-- guarded write resolves a column to NULL when the invariant is broken,
-- and the column's NOT NULL constraint aborts the statement — and with
-- it the whole batch — instead of silently doing nothing
-- (batch_atomic reports no row counts, so a quiet no-op is invisible).
--
-- Conventions as in every Ledgers migration: ids and timestamps are
-- ISO 8601 TEXT bound from code (no NOW() defaults), counters are
-- BIGINT, and the SQL stays inside the harness's portable subset (no
-- AUTOINCREMENT, NOW(), json_extract, FTS5 or backticks). No
-- cross-module foreign keys: module application order is undefined
-- (same reasoning as ledgers-ledger's account migration).
CREATE TABLE numbering_counter (
    journal_id  TEXT NOT NULL,
    fiscal_year INTEGER NOT NULL,
    next_number BIGINT NOT NULL,
    PRIMARY KEY (journal_id, fiscal_year)
);
-- next_number is the number the NEXT bump assigns: provision starts at
-- 0, the bump inside a posting batch advances it by one, and the entry
-- insert in the same batch reads the advanced value. Gapless by
-- construction — a rolled-back batch rolls its bump back with it.
CREATE TABLE ledger_guard (
    guard_key TEXT PRIMARY KEY,
    ok BIGINT NOT NULL
);
-- Transient probe rows: assert_statement inserts (key, CASE WHEN
-- <condition> THEN 1 ELSE NULL END) and the release later in the same
-- batch deletes the row again. ok carries no meaning beyond its NOT
-- NULL: a NULL there (condition false or unknown) is what aborts the
-- batch.
CREATE TABLE accounting_period (
    company_id TEXT NOT NULL,
    period     TEXT NOT NULL,   -- "YYYY-MM"
    status     TEXT NOT NULL,   -- 'open' | 'closed'
    PRIMARY KEY (company_id, period)
);
-- Period administration (the operator-only close/reopen flow) lives in
-- a later crate; this table is the write-time check itself, read by the
-- period-open guard. A MISSING row is treated as closed: the guard's
-- scalar subquery yields NULL, and NULL = 'open' is not true.
CREATE TABLE audit_event (
    company_id TEXT NOT NULL,
    seq        BIGINT NOT NULL,
    prev_hash  TEXT NOT NULL,
    hash       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (company_id, seq)
);
-- The append-only audit chain's sequencing spine. Hashing and
-- verification belong to ledgers-audit later; here hashes are opaque
-- TEXT. The composite PRIMARY KEY is what makes a fork impossible: two
-- batches that read the same head both append the same next seq, one
-- commits, the other takes a unique violation and retries the whole
-- batch against a fresh head (crate::retry).
CREATE TABLE IF NOT EXISTS agent_usage (
    subject TEXT NOT NULL,
    meter TEXT NOT NULL,
    period_start TEXT NOT NULL,
    used BIGINT NOT NULL,
    PRIMARY KEY (subject, meter, period_start)
);
-- The harness's own usage metering table, spelled EXACTLY as
-- cratefield_core::Usage::new("agent_usage").create_table_sql() returns
-- (crate::limit pins the equality in a test, so the two cannot drift):
-- agent limits delegate to that implementation rather than
-- reimplementing it. The guarded consume upsert fails on NOT NULL when
-- the allowance is spent — the daily-limit guard.
