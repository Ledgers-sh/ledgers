-- Dimensions: internal units, cost centres, and projects (issue #8).
-- This migration adds three new tables:
--
-- - dimension_value: a registry of valid dimension values (unit codes,
--   cost centre codes, project codes). Each company maintains its own set.
--
-- - account_dimension_rule: per-account rules for each dimension kind
--   (required, forbidden, or no rule = optional). Accounts and dimensions
--   without a rule row allow and do not require the dimension.
--
-- - journal_line_dimension: a normalized flat table paralleling
--   journal_line, with one row per (entry, line, dimension kind, value).
--   Written in the same batch as journal_line rows; guards use this
--   table so they stay portable SQL (no dialect-specific json_extract).
--
-- Conventions: company_id carries no cross-module foreign key; the
-- normalized dimension table's PK is (company_id, entry_id, line_no,
-- kind); portable SQL, no dialect functions.

CREATE TABLE dimension_value (
    company_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,   -- 'unit' | 'cost_centre' | 'project'
    code         TEXT NOT NULL,   -- 2..=16 ASCII alphanumerics, validated in code
    name         TEXT NOT NULL,   -- Human name
    own_books    INTEGER NOT NULL DEFAULT 0,  -- 1 = unit keeps own books (only for units)
    created_at   TEXT NOT NULL,
    PRIMARY KEY (company_id, kind, code)
);

CREATE INDEX dimension_value_company_idx ON dimension_value (company_id);

CREATE TABLE account_dimension_rule (
    company_id     TEXT NOT NULL,
    account_number TEXT NOT NULL,
    kind           TEXT NOT NULL,   -- 'unit' | 'cost_centre' | 'project'
    rule           TEXT NOT NULL,   -- 'required' | 'forbidden'
    PRIMARY KEY (company_id, account_number, kind)
);

CREATE INDEX account_dimension_rule_account_idx ON account_dimension_rule (account_number);

CREATE TABLE journal_line_dimension (
    company_id TEXT NOT NULL,
    entry_id   TEXT NOT NULL,
    line_no    INTEGER NOT NULL,
    kind       TEXT NOT NULL,   -- 'unit' | 'cost_centre' | 'project'
    value      TEXT NOT NULL,   -- The dimension value
    PRIMARY KEY (company_id, entry_id, line_no, kind)
);

CREATE INDEX journal_line_dimension_entry_idx ON journal_line_dimension (entry_id);
CREATE INDEX journal_line_dimension_value_idx ON journal_line_dimension (kind, value);
