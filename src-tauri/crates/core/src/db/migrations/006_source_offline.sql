-- Migration 6: whether the last sync of a source failed because no connection could be made
-- (docs/architecture.md §7.1). `status` is then 'error'; the status indicator shows the source
-- as offline. A column of its own because SQLite cannot change the CHECK on `status` without
-- rebuilding the table, and dropping `sources` would cascade to every calendar and event.

ALTER TABLE sources ADD COLUMN offline INTEGER NOT NULL DEFAULT 0 CHECK (offline IN (0, 1));
