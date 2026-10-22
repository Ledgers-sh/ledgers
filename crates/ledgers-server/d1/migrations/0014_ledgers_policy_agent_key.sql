-- Agent keys and policy audit trail.
-- Conventions as in all Ledgers migrations: no ULID surrogate where a natural
-- key exists, timestamps are ISO 8601 TEXT in UTC, and the SQL stays inside the
-- harness's portable subset (no AUTOINCREMENT, NOW(), json_extract, FTS5).
--
-- agent_key: the full key policy (tier, limits, expiry, scope, revocation).
-- Natural key is (company_id, id).
--
-- policy: JSON text of AgentKey (serde RFC3339 timestamps).
-- updated_at: ISO 8601 timestamp of last change.
--
CREATE TABLE agent_key (
    company_id       TEXT NOT NULL,
    id               TEXT NOT NULL,
    policy           TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    PRIMARY KEY (company_id, id)
);

-- Audit log: before/after snapshots of policy changes.
-- Append-only; never deleted. Natural key is (company_id, key_id, changed_at)
-- since changed_at is guaranteed unique per key by the application (timestamps are precise).
--
-- key_id: the agent key being changed.
-- before: JSON of AgentKey before the change (NULL if key was newly created).
-- after: JSON of AgentKey after the change (NULL if key was deleted, but deletion is revocation).
-- changed_by: person_id who made the change.
-- changed_at: ISO 8601 timestamp of the change.
--
CREATE TABLE policy_change (
    company_id       TEXT NOT NULL,
    key_id           TEXT NOT NULL,
    before           TEXT,
    after            TEXT NOT NULL,
    changed_by       TEXT NOT NULL,
    changed_at       TEXT NOT NULL,
    PRIMARY KEY (company_id, key_id, changed_at),
    FOREIGN KEY (company_id, key_id) REFERENCES agent_key(company_id, id)
);
