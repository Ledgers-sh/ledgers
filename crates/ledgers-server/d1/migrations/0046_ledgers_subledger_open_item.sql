-- Open items (invoices, bills, credit notes, on-account prepayments and receipts).
-- Amounts are signed in the control account's normal direction: AR invoice positive,
-- AR credit note / on-account receipt negative; AP bill positive, AP on-account
-- payment / debit note negative.
CREATE TABLE open_item (
    id                  TEXT NOT NULL PRIMARY KEY,
    company_id          TEXT NOT NULL,
    ledger              TEXT NOT NULL CHECK (ledger IN ('ar', 'ap')),
    kind                TEXT NOT NULL CHECK (kind IN ('invoice', 'credit-note', 'bill', 'on-account')),
    party_id            TEXT NOT NULL,
    source_id           TEXT,
    entry_id            TEXT NOT NULL,
    control_account     TEXT NOT NULL,
    currency            TEXT NOT NULL,
    rate_scaled         TEXT,
    document_date       TEXT NOT NULL,
    posting_date        TEXT NOT NULL,
    due_date            TEXT,
    tx_original_minor   BIGINT NOT NULL,
    base_original_minor BIGINT NOT NULL,
    tx_remaining_minor  BIGINT NOT NULL,
    base_remaining_minor BIGINT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

-- Unique constraint on (company_id, kind, source_id) for non-NULL source_id
-- SQLite: create a unique index instead (partial indices not fully supported in all versions)
CREATE UNIQUE INDEX idx_open_item_source_id_unique ON open_item(company_id, kind, source_id) WHERE source_id IS NOT NULL;

CREATE INDEX idx_open_item_company ON open_item(company_id);
CREATE INDEX idx_open_item_ledger ON open_item(company_id, ledger);
CREATE INDEX idx_open_item_party ON open_item(company_id, party_id);
CREATE INDEX idx_open_item_open ON open_item(company_id, ledger) WHERE tx_remaining_minor != 0;
