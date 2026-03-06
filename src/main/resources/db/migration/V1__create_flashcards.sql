-- Flyway migration: V1__create_flashcards.sql
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE flashcards (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    front       TEXT        NOT NULL,
    back        TEXT        NOT NULL,
    category    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_flashcards_category ON flashcards (category);
