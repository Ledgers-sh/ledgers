-- Approval inbox: risk, counterparty, decision and result references.
ALTER TABLE approval_request ADD COLUMN risk TEXT NOT NULL DEFAULT 'high';
ALTER TABLE approval_request ADD COLUMN counterparty TEXT;
ALTER TABLE approval_request ADD COLUMN decided_at TEXT;
ALTER TABLE approval_request ADD COLUMN decision_reason TEXT;
ALTER TABLE approval_request ADD COLUMN result TEXT; -- JSON: the command output after approval
ALTER TABLE approval_request ADD COLUMN audit_ref TEXT;
CREATE INDEX IF NOT EXISTS idx_approval_request_risk_status ON approval_request(status, risk);
