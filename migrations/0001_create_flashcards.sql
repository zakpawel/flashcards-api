-- sqlx migration: 0001_create_flashcards.sql
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS flashcards (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    front       TEXT        NOT NULL,
    back        TEXT        NOT NULL,
    category    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_flashcards_category ON flashcards (category);
