-- The company's base currency is fixed once the company has an entry (issue
-- #68). Until then it may still be corrected, because no book depends on it;
-- after the first posted entry every entry, report and close reads it, so a
-- change would silently re-denominate the books. The refusal lives in the
-- database, not only in the code that writes the company row: it is the
-- second line of defence after the posting batch's own currency check.
-- The trigger names journal_entry, a ledgers-ledger table; SQLite resolves
-- trigger bodies when they run, so the storage module can state the rule.
CREATE TRIGGER company_base_currency_frozen
BEFORE UPDATE OF base_currency ON company
FOR EACH ROW WHEN OLD.base_currency <> NEW.base_currency
    AND EXISTS (SELECT 1 FROM journal_entry WHERE company_id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'base currency is fixed once the company has entries');
END;
