-- The outbox table: exactly cratefield_core::Outbox::new("outbox")
-- .create_table_sql() (crate::outbox pins the equality in a test), the
-- same delegation 0001_guards.sql makes for agent_usage. A posting
-- batch enqueues its event here inside the same batch_atomic, so the
-- event is durable exactly when the posting is (docs/PLAN.md §2,
-- "Durable side effects").
CREATE TABLE IF NOT EXISTS outbox (
    id TEXT PRIMARY KEY,
    topic TEXT NOT NULL,
    payload TEXT NOT NULL,
    subject TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    locked_until TEXT,
    created_at TEXT NOT NULL
);
