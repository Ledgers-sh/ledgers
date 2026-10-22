-- Source documents (docs/PLAN.md section 6, issue #20). One row per distinct
-- file: the SHA-256 of the bytes is the dedupe key, so the same file uploaded
-- twice is one document and two upload events.
--
-- Conventions as in every Ledgers migration: ids are ULID TEXT, timestamps are
-- ISO 8601 TEXT in UTC, sizes are BIGINT, and the SQL stays in the portable
-- subset. No cross-module foreign key: company_id is checked by the command.
--
-- retention_class: 'ten-years' (default, GoBD-style retention), 'six-years' or
-- 'indefinite'. text_layer holds the PDF text layer, which the policy screen
-- reads; it is NULL for images and scanned PDFs.
CREATE TABLE document (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    sha256          TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    mime            TEXT NOT NULL,
    original_name   TEXT NOT NULL,
    uploaded_by     TEXT NOT NULL,
    retention_class TEXT NOT NULL CHECK (retention_class IN ('ten-years', 'six-years', 'indefinite')),
    text_layer      TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (company_id, sha256)
);

CREATE TABLE document_upload_event (
    id            TEXT NOT NULL PRIMARY KEY,
    company_id    TEXT NOT NULL,
    document_id   TEXT NOT NULL REFERENCES document (id),
    uploaded_by   TEXT NOT NULL,
    original_name TEXT NOT NULL,
    uploaded_at   TEXT NOT NULL
);

CREATE INDEX document_upload_event_document ON document_upload_event (document_id);
