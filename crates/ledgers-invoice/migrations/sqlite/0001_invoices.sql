-- Sales invoices and credit notes (docs/PLAN.md section 6, issue #24).
-- Invoices are immutable once issued; corrections are credit notes only.
-- Numbering is gapless per (company, series, fiscal_year, number).

CREATE TABLE invoice (
    id                      TEXT NOT NULL PRIMARY KEY,
    company_id              TEXT NOT NULL,
    customer_id             TEXT NOT NULL,
    series                  TEXT NOT NULL,
    number                  BIGINT NOT NULL,
    fiscal_year             BIGINT NOT NULL,
    issue_date              TEXT NOT NULL,
    supply_date             TEXT NOT NULL,
    due_date                TEXT,
    currency                TEXT NOT NULL,
    rate_scaled             TEXT,
    vies_check_id           TEXT,
    posted_entry_id         TEXT NOT NULL,
    created_at              TEXT NOT NULL,
    UNIQUE (company_id, series, fiscal_year, number)
);

CREATE TABLE invoice_line (
    id                      TEXT NOT NULL PRIMARY KEY,
    invoice_id              TEXT NOT NULL REFERENCES invoice (id),
    company_id              TEXT NOT NULL,
    sequence                BIGINT NOT NULL,
    description             TEXT NOT NULL,
    quantity_scaled         TEXT NOT NULL,
    unit                    TEXT,
    unit_price_minor        BIGINT,
    net_minor               BIGINT NOT NULL,
    tax_code                TEXT NOT NULL,
    revenue_account         TEXT NOT NULL,
    created_at              TEXT NOT NULL
);

CREATE TABLE credit_note (
    id                      TEXT NOT NULL PRIMARY KEY,
    company_id              TEXT NOT NULL,
    invoice_id              TEXT NOT NULL,
    series                  TEXT NOT NULL,
    number                  BIGINT NOT NULL,
    fiscal_year             BIGINT NOT NULL,
    issue_date              TEXT NOT NULL,
    reason                  TEXT,
    posted_entry_id         TEXT NOT NULL,
    created_at              TEXT NOT NULL,
    UNIQUE (company_id, series, fiscal_year, number)
);

CREATE TABLE credit_note_line (
    id                      TEXT NOT NULL PRIMARY KEY,
    credit_note_id          TEXT NOT NULL REFERENCES credit_note (id),
    invoice_line_id         TEXT NOT NULL,
    company_id              TEXT NOT NULL,
    net_credited_minor      BIGINT NOT NULL,
    created_at              TEXT NOT NULL
);

-- Gapless numbering counter per (company, series, fiscal_year)
CREATE TABLE invoice_number_counter (
    company_id              TEXT NOT NULL,
    series                  TEXT NOT NULL,
    fiscal_year             BIGINT NOT NULL,
    next_number             BIGINT NOT NULL,
    PRIMARY KEY (company_id, series, fiscal_year)
);

CREATE TABLE credit_note_number_counter (
    company_id              TEXT NOT NULL,
    series                  TEXT NOT NULL,
    fiscal_year             BIGINT NOT NULL,
    next_number             BIGINT NOT NULL,
    PRIMARY KEY (company_id, series, fiscal_year)
);

-- Append-only enforcement: invoice, invoice_line, credit_note, credit_note_line
-- cannot be updated or deleted after creation.
CREATE TRIGGER invoice_no_update
BEFORE UPDATE ON invoice
BEGIN SELECT RAISE(ABORT, 'invoice is append-only'); END;

CREATE TRIGGER invoice_no_delete
BEFORE DELETE ON invoice
BEGIN SELECT RAISE(ABORT, 'invoice is append-only'); END;

CREATE TRIGGER invoice_line_no_update
BEFORE UPDATE ON invoice_line
BEGIN SELECT RAISE(ABORT, 'invoice_line is append-only'); END;

CREATE TRIGGER invoice_line_no_delete
BEFORE DELETE ON invoice_line
BEGIN SELECT RAISE(ABORT, 'invoice_line is append-only'); END;

CREATE TRIGGER credit_note_no_update
BEFORE UPDATE ON credit_note
BEGIN SELECT RAISE(ABORT, 'credit_note is append-only'); END;

CREATE TRIGGER credit_note_no_delete
BEFORE DELETE ON credit_note
BEGIN SELECT RAISE(ABORT, 'credit_note is append-only'); END;

CREATE TRIGGER credit_note_line_no_update
BEFORE UPDATE ON credit_note_line
BEGIN SELECT RAISE(ABORT, 'credit_note_line is append-only'); END;

CREATE TRIGGER credit_note_line_no_delete
BEFORE DELETE ON credit_note_line
BEGIN SELECT RAISE(ABORT, 'credit_note_line is append-only'); END;
