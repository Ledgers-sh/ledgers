-- Entry drafts, recurring templates, and the actor column on journal_entry.
-- This migration adds actor-aware reversals (people can reverse agent posts,
-- agents cannot), draft management (save, edit, post drafts with id-based
-- idempotency), and recurring template scheduling (create drafts on a
-- cron-like schedule). It extends the journal table with:
--
-- - journal_entry.actor: 'human' or 'agent', how the entry was posted
-- - entry_draft: unposted entries, posted when a journal_entry with matching
--   id exists (no status column — the link is enough)
-- - recurring_template: cron-like schedule (monthly/quarterly/yearly) and an
--   EntryDraft blueprint; run_due creates one draft per due date
--
-- Reversals now enforce actor rules: an agent cannot reverse a human-posted
-- entry (reversing a reversal and double reversals are rejected for everyone).
-- Unique constraints prevent concurrent races on both tables. Both dialects
-- use the same portable SQL.

-- Add the actor column to journal_entry (docs/PLAN.md §3 rule 7, policy
-- enforced server-side: every post records who posted, so a reversal can check).
ALTER TABLE journal_entry ADD COLUMN actor TEXT NOT NULL DEFAULT 'agent'
  CHECK (actor IN ('human', 'agent'));

-- Add a UNIQUE index to prevent double reversals: a reversal target can only
-- be reversed once. NULL values are distinct in both SQLite and Postgres, so
-- entries that don't reverse anything don't collide here.
CREATE UNIQUE INDEX journal_entry_reverses_idx ON journal_entry (reverses)
  WHERE reverses IS NOT NULL;

-- Entry draft table: unposted entries are posted iff a journal_entry with
-- matching id exists. `body` stores a JSON serialization of the EntryDraft
-- or a stored shape (the shape decision is made in code). The unique
-- constraint on (recurring_template_id, due_date) makes concurrent runs of
-- the same template create at most one draft per due date.
CREATE TABLE entry_draft (
    id                     TEXT PRIMARY KEY,
    company_id             TEXT NOT NULL,
    journal_id             TEXT NOT NULL REFERENCES journal(id),
    body                   TEXT NOT NULL,   -- JSON of EntryDraft or stored shape
    recurring_template_id  TEXT,
    due_date               TEXT,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL,
    UNIQUE (recurring_template_id, due_date)
);

CREATE INDEX entry_draft_company_idx ON entry_draft (company_id);

-- Recurring template table: name, schedule string ('monthly'|'quarterly'|
-- 'yearly'), next_due (ISO date), paused flag, and the EntryDraft
-- blueprint to use when creating drafts. `body` stores the JSON-serialized
-- blueprint. next_due advances by the schedule when run_due creates drafts.
CREATE TABLE recurring_template (
    id         TEXT PRIMARY KEY,
    company_id TEXT NOT NULL,
    journal_id TEXT NOT NULL REFERENCES journal(id),
    name       TEXT NOT NULL,
    schedule   TEXT NOT NULL,   -- 'monthly' | 'quarterly' | 'yearly'
    next_due   TEXT NOT NULL,
    paused     INTEGER NOT NULL DEFAULT 0,  -- 0 = active, 1 = paused
    body       TEXT NOT NULL,   -- JSON of EntryDraft blueprint
    created_at TEXT NOT NULL
);

CREATE INDEX recurring_template_company_idx ON recurring_template (company_id);
CREATE INDEX recurring_template_next_due_idx ON recurring_template (next_due);
