-- Migration 0002: users, collections, translations for WordTranslator app

CREATE TABLE IF NOT EXISTS users (
    id          TEXT        PRIMARY KEY,          -- opaque string ID (like Parse objectId)
    username    TEXT        NOT NULL UNIQUE,
    email       TEXT        NOT NULL UNIQUE,
    password_hash TEXT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS collections (
    id          TEXT        PRIMARY KEY,
    name        TEXT        NOT NULL,
    language    TEXT        NOT NULL,             -- pipe-separated locales, e.g. "en-US|pl-PL"
    user_id     TEXT        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_collections_user_id ON collections(user_id);

CREATE TABLE IF NOT EXISTS translations (
    id              TEXT        PRIMARY KEY,
    "from"          TEXT        NOT NULL,
    "to"            TEXT        NOT NULL,
    language        TEXT,                         -- optional override locale
    collection_id   TEXT        NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_translations_collection_id ON translations(collection_id);
