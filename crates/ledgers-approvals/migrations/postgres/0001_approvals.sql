-- Approval requests: frozen input, preview, and reads for review before execution.
CREATE TABLE IF NOT EXISTS approval_request (
    id TEXT PRIMARY KEY,
    company_id TEXT NOT NULL,
    command TEXT NOT NULL,
    input TEXT NOT NULL,
    preview TEXT NOT NULL,
    reads TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    effect TEXT NOT NULL, -- JSON: per-currency amounts
    requested_by TEXT NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected', 'expired', 'superseded')),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    decided_by TEXT,
    credential_id TEXT,
    FOREIGN KEY (company_id) REFERENCES company(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_approval_request_company_status ON approval_request(company_id, status);
CREATE INDEX IF NOT EXISTS idx_approval_request_expires_at ON approval_request(expires_at);

-- Nonces for passkey step-up: single-use, short-lived.
CREATE TABLE IF NOT EXISTS approval_nonce (
    nonce TEXT PRIMARY KEY,
    approval_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT,
    FOREIGN KEY (approval_id) REFERENCES approval_request(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_approval_nonce_approval_id ON approval_nonce(approval_id);
CREATE INDEX IF NOT EXISTS idx_approval_nonce_expires_at ON approval_nonce(expires_at);
