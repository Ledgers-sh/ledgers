-- What the company shows about a person it has seen sign in: the verified
-- address from their last sign-in, and when they last used the app (epoch
-- seconds, moved by the session slide). Keyed by the subject id, the same id
-- the organization module keeps as a membership's `user_sub`.
CREATE TABLE IF NOT EXISTS people_profile (
    person_id TEXT PRIMARY KEY,
    email TEXT,
    last_seen_at INTEGER NOT NULL
);
