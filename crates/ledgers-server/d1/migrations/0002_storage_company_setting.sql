-- Company-scoped settings, and the shape every company-scoped table
-- follows: a company_id referencing company(id) plus the company-local
-- natural key (here a composite PRIMARY KEY, so a setting is one row per
-- company per key, never a global key namespace).
CREATE TABLE company_setting (
    company_id TEXT NOT NULL REFERENCES company(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (company_id, key)
);
