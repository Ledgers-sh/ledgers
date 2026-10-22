-- The audit chain is append-only, enforced by the database itself
-- (docs/PLAN.md §3 rule 6: an append-only, hash-chained log).
--
-- `chain::append_statement` is the only intended write, and the
-- composite PRIMARY KEY already makes a fork impossible; these two
-- triggers are the second line of defence against every path that is
-- not that statement — a hand-written SQL batch, an operator's psql
-- session, a future maintenance migration. An auditor who must be able
-- to say "no row was ever rewritten or removed" needs that refusal to
-- live below the application, not in it.
--
-- There is deliberately no escape hatch: a correction is a new event
-- (or a reversal elsewhere), never an UPDATE of a recorded one.
--
-- SQLite spelling: RAISE(ABORT, …) inside a BEFORE trigger, which the
-- batch runner reports as a failed statement and the whole batch rolls
-- back. This file is the portable set;
-- migrations/postgres/0003_audit_event_append_only.sql carries the
-- plpgsql equivalent for the Postgres override set (SQLite trigger
-- syntax is a dialect of its own).
CREATE TRIGGER audit_event_no_update
BEFORE UPDATE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit_event is append-only (docs/PLAN.md §3 rule 6)');
END;

CREATE TRIGGER audit_event_no_delete
BEFORE DELETE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit_event is append-only (docs/PLAN.md §3 rule 6)');
END;