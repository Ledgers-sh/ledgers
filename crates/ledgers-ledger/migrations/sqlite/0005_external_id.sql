-- Track external IDs for app-level idempotency
ALTER TABLE journal_entry ADD COLUMN external_id TEXT;
CREATE UNIQUE INDEX journal_entry_external_id_idx
  ON journal_entry(company_id, external_id)
  WHERE external_id IS NOT NULL;
