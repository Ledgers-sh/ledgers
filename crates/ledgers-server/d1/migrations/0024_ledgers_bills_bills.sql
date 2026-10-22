-- Vendor bills (docs/PLAN.md section 6, issue #22) and the minimal vendor
-- table they need (the parties issue #44 will own vendors later).
--
-- A vendor is matched by VAT id, then by normalised name. A vendor created
-- from an extraction needs a person (needs_review = 1) until bills.confirm.
-- A bill is one document: UNIQUE (company_id, document_id) refuses a second
-- bill for the same file, and UNIQUE (company_id, vendor_id, invoice_number)
-- refuses the same invoice twice from one vendor. Both are guards, not
-- read-then-write checks; the command reads them first only to name the
-- existing bill in its refusal.
--
-- The bill's status is not stored while it is open: it is derived from the
-- checks, the flags and the vendor on every read. Only posted_entry_id is
-- written at posting.
CREATE TABLE vendor (
    id           TEXT NOT NULL PRIMARY KEY,
    company_id   TEXT NOT NULL,
    name         TEXT NOT NULL,
    vat_id       TEXT,
    iban         TEXT,
    source       TEXT NOT NULL CHECK (source IN ('extraction', 'manual')),
    needs_review INTEGER NOT NULL CHECK (needs_review IN (0, 1)),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (company_id, vat_id)
);

CREATE TABLE bill (
    id                    TEXT NOT NULL PRIMARY KEY,
    company_id            TEXT NOT NULL,
    document_id           TEXT NOT NULL,
    vendor_id             TEXT NOT NULL REFERENCES vendor (id),
    invoice_number        TEXT,
    invoice_date          TEXT,
    due_date              TEXT,
    currency              TEXT NOT NULL,
    net_minor             BIGINT,
    vat_minor             BIGINT,
    gross_minor           BIGINT,
    lines_json            TEXT NOT NULL,
    checks_json           TEXT NOT NULL,
    instruction_flags     TEXT NOT NULL,
    iban_changed          INTEGER NOT NULL CHECK (iban_changed IN (0, 1)),
    source                TEXT NOT NULL CHECK (source IN ('rules', 'llm', 'manual')),
    posted_entry_id       TEXT,
    created_by            TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE (company_id, document_id),
    UNIQUE (company_id, vendor_id, invoice_number)
);

