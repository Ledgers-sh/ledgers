-- Connected sources (docs/PLAN.md section 6, issue #142).
--
-- connector_connection: one connection to a vendor for one company. config_json
-- holds the connector's non-secret settings (account id, expense account,
-- rates). default_unit receives every resource with no allocation rule.
-- is_demo marks the seeded demo connections (recorded fixtures, no tokens).
--
-- deleted_at: set by connections.delete, which removes the token and stops
-- syncs; the imported bills and the run history stay for the books.
--
-- connector_credential: the token, sealed with AES-256-GCM under the server key
-- and bound to the connection id (see ledgers-connect secret.rs). Written once
-- per connection and replaced on rotation; no command returns it.
--
-- connector_run: append-only sync history. A run is written once, when the
-- sync finishes, with its outcome and the notes the vendor left.
--
-- connector_invoice: one row per vendor invoice (or per vendor-period total).
-- UNIQUE (connection_id, external_id) is the idempotency guard: re-syncing the
-- same invoice creates nothing new. bill_id links the draft bill.
--
-- connector_usage: usage per period and resource (the allocation key).
-- connector_split: the exact split of an invoice across units. Amounts are
-- integer minor units and the splits of one invoice sum to its total.
-- allocation_rule: resource pattern to unit, per connection.
--
-- Ids are ULIDs, timestamps ISO 8601 text, amounts BIGINT (portable SQL).
CREATE TABLE connector_connection (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connector       TEXT NOT NULL CHECK (connector IN ('cloudflare', 'anthropic', 'planetscale')),
    name            TEXT NOT NULL,
    config_json     TEXT NOT NULL,
    default_unit    TEXT NOT NULL,
    is_demo         INTEGER NOT NULL DEFAULT 0 CHECK (is_demo IN (0, 1)),
    last_sync_at    TEXT,
    deleted_at      TEXT,
    created_by      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (company_id, name)
);

CREATE TABLE connector_credential (
    connection_id   TEXT NOT NULL PRIMARY KEY REFERENCES connector_connection (id),
    company_id      TEXT NOT NULL,
    sealed          TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE connector_run (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    window_since    TEXT NOT NULL,
    window_until    TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('succeeded', 'failed')),
    invoices_found  BIGINT NOT NULL,
    bills_created   BIGINT NOT NULL,
    usage_rows      BIGINT NOT NULL,
    error_message   TEXT,
    notes_json      TEXT NOT NULL,
    created_by      TEXT NOT NULL,
    finished_at     TEXT NOT NULL
);

CREATE INDEX connector_run_connection_idx ON connector_run (company_id, connection_id, finished_at);

CREATE TABLE connector_invoice (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    external_id     TEXT NOT NULL,
    period          TEXT NOT NULL,
    issued_on       TEXT NOT NULL,
    vendor_id       TEXT NOT NULL,
    vendor_name     TEXT NOT NULL,
    invoice_number  TEXT,
    currency        TEXT NOT NULL,
    total_minor     BIGINT NOT NULL,
    description     TEXT NOT NULL,
    bill_id         TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (connection_id, external_id)
);

CREATE INDEX connector_invoice_period_idx ON connector_invoice (company_id, connection_id, period);

CREATE TABLE connector_usage (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    period          TEXT NOT NULL,
    resource_kind   TEXT NOT NULL,
    resource_id     TEXT NOT NULL,
    resource_name   TEXT NOT NULL,
    quantity        BIGINT NOT NULL CHECK (quantity >= 0),
    updated_at      TEXT NOT NULL,
    UNIQUE (connection_id, period, resource_kind, resource_id)
);

CREATE TABLE connector_split (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    invoice_id      TEXT NOT NULL REFERENCES connector_invoice (id),
    period          TEXT NOT NULL,
    unit_code       TEXT NOT NULL,
    currency        TEXT NOT NULL,
    amount_minor    BIGINT NOT NULL,
    UNIQUE (invoice_id, unit_code)
);

CREATE INDEX connector_split_report_idx ON connector_split (company_id, period, unit_code);

CREATE TABLE allocation_rule (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    connection_id   TEXT NOT NULL REFERENCES connector_connection (id),
    pattern         TEXT NOT NULL,
    unit_code       TEXT NOT NULL,
    position        INTEGER NOT NULL,
    UNIQUE (connection_id, pattern)
);

CREATE TRIGGER connector_run_no_update
BEFORE UPDATE ON connector_run
BEGIN SELECT RAISE(ABORT, 'connector_run is append-only'); END;

CREATE TRIGGER connector_run_no_delete
BEFORE DELETE ON connector_run
BEGIN SELECT RAISE(ABORT, 'connector_run is append-only'); END;
