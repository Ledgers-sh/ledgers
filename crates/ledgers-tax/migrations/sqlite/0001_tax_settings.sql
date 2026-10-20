-- Company-level tax settings: small-business exemption and VAT reporting method.
-- Conventions as in all Ledgers migrations: no ULID surrogate where a natural
-- key exists (company_id is the identity), timestamps are ISO 8601 TEXT in UTC,
-- and the SQL stays inside the harness's portable subset (no AUTOINCREMENT,
-- NOW(), json_extract, FTS5 or backticks).
--
-- small_business: 0 = not exempt, 1 = exempt (Kleinunternehmer, § 19 UStG).
-- vat_basis: 'accrual' (Sollversteuerung, default) or 'cash' (Istversteuerung).
--
-- As with other Ledgers modules: no cross-module foreign keys — module
-- application order is undefined, so company_id carries no FK to company(id);
-- the command layer owns the reference.
--
CREATE TABLE tax_settings (
    company_id     TEXT NOT NULL PRIMARY KEY,
    small_business INTEGER NOT NULL CHECK (small_business IN (0, 1)),
    vat_basis      TEXT NOT NULL CHECK (vat_basis IN ('accrual', 'cash')),
    updated_at     TEXT NOT NULL
);
