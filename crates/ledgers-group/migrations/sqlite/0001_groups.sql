-- Company groups and intercompany agreements (issue #128, priorities 3 to 5
-- and 7). Conventions as every Ledgers migration: ids are ULID TEXT, timestamps
-- ISO 8601 TEXT in UTC, amounts are BIGINT minor units, and the SQL is portable.
--
-- A group is not owned by one company, so company_group carries no company_id.
-- Each member's row lives in the member's own partition (company_id = member),
-- so a company's reads see only its own memberships.
CREATE TABLE company_group (
    id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE company_group_member (
    company_id TEXT NOT NULL REFERENCES company(id),
    group_id TEXT NOT NULL REFERENCES company_group(id),
    kind TEXT NOT NULL CHECK (kind IN ('holding', 'ip_holding', 'operating')),
    ownership_bp INTEGER NOT NULL CHECK (ownership_bp BETWEEN 0 AND 10000),
    created_at TEXT NOT NULL,
    PRIMARY KEY (company_id, group_id)
);

CREATE INDEX company_group_member_group_idx ON company_group_member (group_id);

-- One row per party side: the licensor's copy and the licensee's copy share
-- the id and the terms, and carry the accounts that side posts to.
CREATE TABLE intercompany_agreement (
    company_id TEXT NOT NULL REFERENCES company(id),
    id TEXT NOT NULL,
    group_id TEXT NOT NULL REFERENCES company_group(id),
    side TEXT NOT NULL CHECK (side IN ('licensor', 'licensee')),
    counterparty_id TEXT NOT NULL REFERENCES company(id),
    kind TEXT NOT NULL CHECK (kind IN ('licence', 'royalty', 'management_fee', 'loan')),
    basis TEXT NOT NULL CHECK (basis IN ('pct_of_revenue_bp', 'fixed_monthly', 'cost_plus_bp')),
    rate_bp INTEGER CHECK (rate_bp IS NULL OR rate_bp BETWEEN 0 AND 100000),
    fixed_minor BIGINT CHECK (fixed_minor IS NULL OR fixed_minor >= 0),
    currency TEXT NOT NULL,
    wht_bp INTEGER NOT NULL CHECK (wht_bp BETWEEN 0 AND 10000),
    wht_basis TEXT NOT NULL CHECK (wht_basis IN ('pack', 'treaty_override', 'resident', 'no_rate_on_file', 'not_applicable')),
    treaty_override_bp INTEGER CHECK (treaty_override_bp IS NULL OR treaty_override_bp BETWEEN 0 AND 10000),
    account_royalty TEXT NOT NULL,
    account_ic TEXT NOT NULL,
    account_wht TEXT,
    account_bank TEXT NOT NULL,
    account_unit TEXT,
    notes TEXT,
    terminated_at TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (company_id, id)
);

CREATE INDEX intercompany_agreement_group_idx ON intercompany_agreement (group_id);

-- One row per agreement, period and side. The id is the intercompany id
-- IC-<agreement>-<period>, shared by both sides, so run_period is idempotent:
-- the primary key refuses a second draft. status moves draft -> posted ->
-- settled; failed is a post attempt that was undone (both sides), and a
-- later post starts again from it.
CREATE TABLE intercompany_document (
    company_id TEXT NOT NULL REFERENCES company(id),
    id TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('licensor', 'licensee')),
    agreement_id TEXT NOT NULL,
    period TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('draft', 'posted', 'settled', 'failed')),
    currency TEXT NOT NULL,
    basis_minor BIGINT NOT NULL,
    gross_minor BIGINT NOT NULL,
    wht_minor BIGINT NOT NULL,
    net_minor BIGINT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0,
    accrual_entry_id TEXT,
    settle_entry_id TEXT,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (company_id, id, side)
);

CREATE INDEX intercompany_document_agreement_idx ON intercompany_document (company_id, agreement_id, period);
