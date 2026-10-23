-- Document anchors and upload-event source tracking (issue #20).
--
-- `document_anchor`: associates semantic fields (e.g., "total", "invoice_number")
-- with their locations in the source PDF or image. Each anchor is either a
-- page-based bounding box (page 1-based, x/y/w/h in 1/10000 units) or an
-- XML path (e.g., for CII/ZUGFeRD e-invoices). UNIQUE on (company_id, document_id, field).
--
-- `document_upload_event`: adds source (default 'upload', also 'email'/'folder'/'api').

ALTER TABLE document_upload_event ADD COLUMN source TEXT NOT NULL DEFAULT 'upload'
  CHECK (source IN ('upload', 'email', 'folder', 'api'));

CREATE TABLE document_anchor (
    id              TEXT NOT NULL PRIMARY KEY,
    company_id      TEXT NOT NULL,
    document_id     TEXT NOT NULL REFERENCES document (id),
    field           TEXT NOT NULL,
    page            INTEGER,
    x               INTEGER,
    y               INTEGER,
    w               INTEGER,
    h               INTEGER,
    xml_path        TEXT,
    created_at      TEXT NOT NULL,
    CHECK ((page IS NOT NULL AND x IS NOT NULL AND y IS NOT NULL AND w IS NOT NULL AND h IS NOT NULL AND xml_path IS NULL)
        OR (page IS NULL AND x IS NULL AND y IS NULL AND w IS NULL AND h IS NULL AND xml_path IS NOT NULL)),
    UNIQUE (company_id, document_id, field)
);

CREATE INDEX document_anchor_document ON document_anchor (document_id);
