-- Migration 0005: one-time codes for resetting a forgotten password.
-- At most one active code per user; requesting a new one replaces the old one.
CREATE TABLE password_resets (
    user_id    TEXT        PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    code_hash  TEXT        NOT NULL,             -- bcrypt hash of the 6-digit code
    attempts   INT         NOT NULL DEFAULT 0,   -- failed guesses so far
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
