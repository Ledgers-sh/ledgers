-- Approval inbox: risk, counterparty, decision and result references.
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS risk TEXT NOT NULL DEFAULT 'high';
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS counterparty TEXT;
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS decided_at TEXT;
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS decision_reason TEXT;
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS result TEXT;
ALTER TABLE approval_request ADD COLUMN IF NOT EXISTS audit_ref TEXT;
CREATE INDEX IF NOT EXISTS idx_approval_request_risk_status ON approval_request(status, risk);
