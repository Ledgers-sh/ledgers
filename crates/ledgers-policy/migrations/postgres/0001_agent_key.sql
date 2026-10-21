-- Agent keys and policy audit trail (Postgres dialect).
-- See sqlite/0001_agent_key.sql for semantics.

CREATE TABLE IF NOT EXISTS agent_key (
    company_id       TEXT NOT NULL,
    id               TEXT NOT NULL,
    policy           TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    PRIMARY KEY (company_id, id)
);

CREATE TABLE IF NOT EXISTS policy_change (
    company_id       TEXT NOT NULL,
    key_id           TEXT NOT NULL,
    before           TEXT,
    after            TEXT NOT NULL,
    changed_by       TEXT NOT NULL,
    changed_at       TEXT NOT NULL,
    PRIMARY KEY (company_id, key_id, changed_at),
    FOREIGN KEY (company_id, key_id) REFERENCES agent_key(company_id, id)
);
