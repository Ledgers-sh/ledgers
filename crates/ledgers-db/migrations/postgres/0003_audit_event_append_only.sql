-- The Postgres override of the audit-chain append-only defence: the
-- same two BEFORE triggers as the portable set in
-- migrations/sqlite/0003_audit_event_append_only.sql, plus a TRUNCATE
-- guard, in Postgres's own trigger dialect. SQLite's
-- `CREATE TRIGGER … BEGIN SELECT RAISE(ABORT, …); END;` is not valid
-- Postgres, so the refusal lives in a plpgsql function the triggers
-- call. Same rule, same error text, second line of defence against
-- every path that is not `chain::append_statement`
-- (docs/PLAN.md §3 rule 6).
--
-- Why an override set at all: the Postgres migration runner prefers a
-- module's postgres set when one exists and applies the sqlite set to
-- Postgres only when it passes the portable lint — which the sqlite
-- file does, but its trigger syntax would still fail on arrival. The
-- runner cannot know that, so this file says it explicitly.
--
-- 0001 and 0002 are the portable sqlite files, included from code
-- (SQLITE_MIGRATIONS in src/lib.rs) rather than copied here: they are
-- byte-identical DDL, and this set must be complete and contiguous
-- from 0001 on its own (assert_migration_set).
--
-- One refusal function for both triggers: the message is the rule, and
-- a second copy of it is a second thing to keep true.
CREATE OR REPLACE FUNCTION audit_event_append_only() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'audit_event is append-only (docs/PLAN.md §3 rule 6)'
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_event_no_update
BEFORE UPDATE ON audit_event
FOR EACH ROW EXECUTE FUNCTION audit_event_append_only();

CREATE TRIGGER audit_event_no_delete
BEFORE DELETE ON audit_event
FOR EACH ROW EXECUTE FUNCTION audit_event_append_only();

CREATE TRIGGER audit_event_no_truncate
BEFORE TRUNCATE ON audit_event
FOR EACH STATEMENT EXECUTE FUNCTION audit_event_append_only();