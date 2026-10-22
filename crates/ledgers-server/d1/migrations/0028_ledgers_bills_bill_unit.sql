-- The business unit a bill's expense lines belong to (ledgers-ledger units).
-- NULL while the company has no units, or until a person names one; once
-- units are enabled, a bill posts only with a unit. Set by bills.create and
-- bills.update, read by bills.list (unit scope) and bills.post (every line
-- on an income or expense account carries it). Ordinary column: the bill
-- row is a draft until posting, and the posted entry is what is immutable.
ALTER TABLE bill ADD COLUMN unit_code TEXT;
