-- Allocations: payments, receipts, offsets and write-offs applied to open items.
-- The entry_id is NULL for link-only offsets where the base cleared on both sides
-- is equal (no FX difference to post).
CREATE TABLE allocation (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    open_item_id    TEXT NOT NULL REFERENCES open_item(id),
    counter_item_id TEXT REFERENCES open_item(id),
    entry_id        TEXT,
    kind            TEXT NOT NULL CHECK (kind IN ('payment', 'offset', 'write-off')),
    date            TEXT NOT NULL,
    tx_minor        BIGINT NOT NULL,
    base_minor      BIGINT NOT NULL,
    fx_difference_minor BIGINT NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    -- Non-unique index on entry_id (multiple allocations can share one entry)
);

CREATE INDEX idx_allocation_item ON allocation(open_item_id);
CREATE INDEX idx_allocation_company ON allocation(company_id);
CREATE INDEX idx_allocation_entry ON allocation(entry_id);
