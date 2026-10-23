-- Bill payments (issue #22): track paid amounts on posted bills.
-- The paid_minor column records cumulative payments; overpayment is rejected
-- by a guard statement in the batch.

ALTER TABLE bill ADD COLUMN paid_minor INTEGER NOT NULL DEFAULT 0;
