-- Track which app posted the entry
ALTER TABLE journal_entry ADD COLUMN source_app TEXT;
CREATE INDEX journal_entry_source_app_idx
  ON journal_entry(company_id, source_app);
