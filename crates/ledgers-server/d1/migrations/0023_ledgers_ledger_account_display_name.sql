-- Chart of accounts management (accounts.create / update / archive / restore).
--
-- `name_key` stays the account's translatable key: template accounts keep
-- their i18n names in every locale. `display_name` is the name a person
-- typed for an account, or a renamed template account's name: when it is
-- set it wins over the translated name in every language. NULL means "use
-- the i18n name for name_key". Text is trimmed and length-checked in code
-- (crate::account), never by a dialect function.
ALTER TABLE account ADD COLUMN display_name TEXT;

-- The before/after record of every chart change, written in the same
-- batch as the change and its audit event. The audit log stores only a
-- hash of each command's input (ledgers-audit never keeps payloads), so
-- the readable history of an account lives here: one append-only row per
-- create, update, archive or restore, keyed by the audit event it belongs
-- to. before_json is NULL for a creation. Rows are never updated or
-- deleted; corrections to an account are new changes.
CREATE TABLE account_change (
    event_id    TEXT PRIMARY KEY,    -- the audit_event id of the change
    company_id  TEXT NOT NULL,
    number      TEXT NOT NULL,
    command     TEXT NOT NULL,       -- account.create|account.update|account.archive|account.restore
    actor       TEXT NOT NULL,       -- person:<id> or agent:<key id>
    changed_at  TEXT NOT NULL,       -- ISO 8601 UTC
    before_json TEXT,                -- JSON snapshot before; NULL for a create
    after_json  TEXT NOT NULL        -- JSON snapshot after
);
CREATE INDEX account_change_by_account_idx ON account_change (company_id, number, changed_at);
