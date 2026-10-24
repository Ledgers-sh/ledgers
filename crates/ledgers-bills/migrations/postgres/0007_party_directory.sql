-- The party directory (issue: clients and suppliers with full metadata and
-- images). Additive only: every column is nullable or has a default, so the
-- rows that exist keep their meaning and no table is rebuilt.
--
-- `name` stays the legal name; `trading_name` is the name people use.
-- `tax_ids` holds further VAT and tax identifiers as a JSON array of
-- {country, kind, value} (`vat_id` stays the primary VAT ID that VIES checks).
-- `tags` is a JSON array of short labels; `notes` is free text.
-- `default_unit` names the business unit a party's documents book to.
-- `logo_document_id` names a stored PNG or JPEG of the party's company
-- documents (at most 2 MiB; the command checks it, the server serves it).

ALTER TABLE vendor ADD COLUMN trading_name TEXT;
ALTER TABLE vendor ADD COLUMN registration_number TEXT;
ALTER TABLE vendor ADD COLUMN website TEXT;
ALTER TABLE vendor ADD COLUMN email TEXT;
ALTER TABLE vendor ADD COLUMN phone TEXT;
ALTER TABLE vendor ADD COLUMN default_unit TEXT;
ALTER TABLE vendor ADD COLUMN tags TEXT;
ALTER TABLE vendor ADD COLUMN notes TEXT;
ALTER TABLE vendor ADD COLUMN tax_ids TEXT;
ALTER TABLE vendor ADD COLUMN logo_document_id TEXT;
