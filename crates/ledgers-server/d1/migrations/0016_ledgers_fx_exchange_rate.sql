-- Exchange rates for foreign currency transactions.
-- Conventions as in all Ledgers migrations: no ULID surrogate where a natural
-- key exists, timestamps are ISO 8601 TEXT in UTC, and the SQL stays inside the
-- harness's portable subset (no AUTOINCREMENT, NOW(), json_extract, FTS5).
--
-- The natural key is (company_id, base_currency, quote_currency, rate_date).
-- Rates are reference data: recording a rate for an existing key replaces it
-- (upsert), never updates posted entries.
--
-- rate_scaled: rate × 10^12, as decimal text (1 base = rate quote).
-- source: 'manual' | 'csv' | 'ecb'.
-- recorded_at: ISO 8601 timestamp of when the rate was recorded.
--
-- Semantics: 1 unit of base_currency = rate_scaled / 10^12 units of quote_currency.
--
CREATE TABLE exchange_rate (
    company_id       TEXT NOT NULL,
    base_currency    TEXT NOT NULL,
    quote_currency   TEXT NOT NULL,
    rate_date        TEXT NOT NULL,
    rate_scaled      TEXT NOT NULL,
    source           TEXT NOT NULL CHECK (source IN ('manual', 'csv', 'ecb')),
    recorded_at      TEXT NOT NULL,
    PRIMARY KEY (company_id, base_currency, quote_currency, rate_date)
);
