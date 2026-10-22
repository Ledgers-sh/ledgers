-- Parties master data (issue #44): customers and vendors.
-- The existing vendor table becomes the single party master record, so bills,
-- bank matching and parties share one source of truth. This is an in-place
-- migration: every existing vendor row survives with its id, name, VAT ID,
-- IBAN, source and needs_review, and gets roles = 'vendor' (the default), so
-- bill.vendor_id links keep resolving. Nothing is renamed or copied.
-- It adds: roles, tax numbers, addresses, contacts, payment terms, bank
-- change history (append-only), and VIES VAT checking results (append-only).

ALTER TABLE vendor ADD COLUMN roles TEXT NOT NULL DEFAULT 'vendor'
    CHECK (roles IN ('customer', 'vendor', 'both'));
ALTER TABLE vendor ADD COLUMN tax_number TEXT;
ALTER TABLE vendor ADD COLUMN addresses TEXT;
ALTER TABLE vendor ADD COLUMN contacts TEXT;
ALTER TABLE vendor ADD COLUMN payment_terms_days INTEGER CHECK (payment_terms_days IS NULL OR payment_terms_days >= 0);
ALTER TABLE vendor ADD COLUMN default_account TEXT;
ALTER TABLE vendor ADD COLUMN default_tax_code TEXT;
ALTER TABLE vendor ADD COLUMN language TEXT;
ALTER TABLE vendor ADD COLUMN currency TEXT;
ALTER TABLE vendor ADD COLUMN bic TEXT;
ALTER TABLE vendor ADD COLUMN personal_data INTEGER NOT NULL DEFAULT 0
    CHECK (personal_data IN (0, 1));
ALTER TABLE vendor ADD COLUMN merged_into TEXT;

-- Append-only bank change history: triggered INSERT or UPDATE on vendor
-- to record before/after IBAN/BIC, who changed it and when.
CREATE TABLE vendor_bank_change (
    id           TEXT NOT NULL PRIMARY KEY,
    company_id   TEXT NOT NULL,
    vendor_id    TEXT NOT NULL,
    before_iban  TEXT,
    before_bic   TEXT,
    after_iban   TEXT,
    after_bic    TEXT,
    changed_by   TEXT NOT NULL,
    changed_at   TEXT NOT NULL,
    reason       TEXT,
    created_at   TEXT NOT NULL
);

-- Append-only VIES check results: VAT number validity, returned data.
CREATE TABLE vies_check (
    id                  TEXT NOT NULL PRIMARY KEY,
    company_id          TEXT NOT NULL,
    vendor_id           TEXT NOT NULL,
    country_code        TEXT NOT NULL,
    vat_number          TEXT NOT NULL,
    valid               INTEGER NOT NULL CHECK (valid IN (0, 1)),
    name                TEXT,
    address             TEXT,
    consultation_number TEXT,
    request_date        TEXT,
    checked_at          TEXT NOT NULL
);

-- Append-only enforcement: vendor_bank_change cannot be updated or deleted.
CREATE TRIGGER vendor_bank_change_no_update
BEFORE UPDATE ON vendor_bank_change
BEGIN SELECT RAISE(ABORT, 'vendor_bank_change is append-only'); END;

CREATE TRIGGER vendor_bank_change_no_delete
BEFORE DELETE ON vendor_bank_change
BEGIN SELECT RAISE(ABORT, 'vendor_bank_change is append-only'); END;

-- Append-only enforcement: vies_check cannot be updated or deleted.
CREATE TRIGGER vies_check_no_update
BEFORE UPDATE ON vies_check
BEGIN SELECT RAISE(ABORT, 'vies_check is append-only'); END;

CREATE TRIGGER vies_check_no_delete
BEFORE DELETE ON vies_check
BEGIN SELECT RAISE(ABORT, 'vies_check is append-only'); END;
