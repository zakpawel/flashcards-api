-- Make password_hash nullable so social-only accounts don't need one.
ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;

-- Each row links a user to a social identity provider.
CREATE TABLE auth_providers (
    id           TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider     TEXT NOT NULL,               -- 'google' | 'facebook'
    provider_uid TEXT NOT NULL,               -- provider's stable user id
    email        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_uid)
);

CREATE INDEX auth_providers_user_id_idx ON auth_providers(user_id);
