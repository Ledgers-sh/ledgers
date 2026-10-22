-- The Postgres form of sqlite/0003_company_currency_frozen.sql: the same rule
-- and the same refusal text, as a plpgsql trigger function. The table
-- journal_entry is resolved when the function runs, not when it is created.
CREATE FUNCTION ledgers_company_currency_frozen() RETURNS trigger AS $$
BEGIN
    IF OLD.base_currency <> NEW.base_currency
        AND EXISTS (SELECT 1 FROM journal_entry WHERE company_id = OLD.id) THEN
        RAISE EXCEPTION 'base currency is fixed once the company has entries';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER company_base_currency_frozen
BEFORE UPDATE OF base_currency ON company
FOR EACH ROW EXECUTE FUNCTION ledgers_company_currency_frozen();
