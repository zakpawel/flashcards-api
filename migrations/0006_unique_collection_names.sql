-- Migration 0006: a user can't have two collections with the same name
-- (ignoring case and surrounding spaces).
--
-- Existing duplicates are renamed first, oldest keeps its name: "Food", "Food (2)", "Food (3)".
-- A second pass covers the unlikely case where a generated name collides with an existing one.
WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY user_id, lower(btrim(name))
               ORDER BY created_at, id
           ) AS rn
    FROM collections
)
UPDATE collections c
SET name = btrim(c.name) || ' (' || r.rn || ')'
FROM ranked r
WHERE c.id = r.id AND r.rn > 1;

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY user_id, lower(btrim(name))
               ORDER BY created_at, id
           ) AS rn
    FROM collections
)
UPDATE collections c
SET name = btrim(c.name) || ' [' || left(c.id, 6) || ']'
FROM ranked r
WHERE c.id = r.id AND r.rn > 1;

CREATE UNIQUE INDEX collections_user_name_unique
    ON collections (user_id, lower(btrim(name)));
