-- Approval notification targets: opt-in per company, person, channel.
CREATE TABLE IF NOT EXISTS approval_notify_target (
    company_id TEXT NOT NULL REFERENCES company(id) ON DELETE CASCADE,
    person_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    target TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (company_id, person_id, channel)
);

CREATE INDEX IF NOT EXISTS idx_approval_notify_target_company_person
    ON approval_notify_target(company_id, person_id);

-- Dedicated outbox for approval notifications (no topic filter in Outbox::claim_due).
-- Same schema as outbox but isolated so drain never claims audit/ledger records.
CREATE TABLE IF NOT EXISTS approval_outbox (
    id TEXT PRIMARY KEY,
    subject TEXT,
    topic TEXT NOT NULL,
    payload TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    locked_until TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_approval_outbox_next_attempt
    ON approval_outbox(next_attempt_at);
