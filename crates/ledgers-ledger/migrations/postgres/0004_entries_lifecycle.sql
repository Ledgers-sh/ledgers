-- Entry drafts, recurring templates, and the actor column on journal_entry.
-- This is the Postgres override set, identical DDL except where Postgres requires
-- different syntax. The immutability triggers in 0002_journal.sql (plpgsql) continue
-- to apply: the ALTER TABLE ADD COLUMN happens after those triggers are in place,
-- so no change to the trigger logic is needed.

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
    paused     BIGINT NOT NULL DEFAULT 0,  -- 0 = active, 1 = paused
    body       TEXT NOT NULL,   -- JSON of EntryDraft blueprint
    created_at TEXT NOT NULL
);

CREATE INDEX recurring_template_company_idx ON recurring_template (company_id);
CREATE INDEX recurring_template_next_due_idx ON recurring_template (next_due);
