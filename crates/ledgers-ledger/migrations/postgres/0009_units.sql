-- Business units (docs/PLAN.md section 6, the `ledgers-ledger` dimensions row).
--
-- A unit is a `dimension_value` of kind 'unit' that a company uses as a
-- business unit: a venture, a department or a shared-cost centre inside one
-- legal company. Units are not companies: they share one chart, one set of
-- journals and one numbering sequence. What a unit adds is its own books
-- view (its lines net to zero in base, through the inter-unit clearing
-- account) and its own reports.
--
-- colour: '#RRGGBB', shown as the unit's dot in the app. Validated in code.
-- logo_document_id: an optional document in the document store (a logo).
-- archived: 1 hides the unit from the switcher and from new postings. Its
--   history stays: archiving is the only retirement, as for accounts.
--
-- Renaming, recolouring and archiving write UPDATEs to this table; posted
-- entries never change, and every such change is audited in the same batch.
ALTER TABLE dimension_value ADD COLUMN colour TEXT NOT NULL DEFAULT '#7FA6C9';
ALTER TABLE dimension_value ADD COLUMN logo_document_id TEXT;
ALTER TABLE dimension_value ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;

-- Journal entries that move money between units (a shared-cost recharge)
-- are recognised by the chart, not by a flag: an entry that touches an
-- account tagged inter-unit-due-from or inter-unit-due-to is intercompany.
-- Reports eliminate intercompany entries from the consolidated view; the
-- posting guards keep their profit-and-loss lines netting to zero, which
-- is what makes that elimination exact (see crate::units).
