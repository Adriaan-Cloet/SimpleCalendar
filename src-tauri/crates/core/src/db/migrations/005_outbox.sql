-- Migration 5: the outbox that Google write needs (docs/architecture.md §6, §7.3).
-- A local edit to a remote calendar changes `events` and adds an outbox row in one transaction.
-- `payload` is the state the user wants and `base` the state before the edit, both as the JSON
-- of `outbox::EventData`; the engine sends the fields that differ. A deleted event stays in
-- `events` with `pending_delete = 1`, hidden from every read, until the server confirms.
-- `attempts`, `next_attempt_at` and `in_flight` are for the offline outbox of v0.3.

ALTER TABLE events ADD COLUMN pending_delete INTEGER NOT NULL DEFAULT 0
    CHECK (pending_delete IN (0, 1));

CREATE TABLE outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT, -- processing order
    calendar_id     INTEGER NOT NULL REFERENCES calendars (id) ON DELETE CASCADE,
    event_id        INTEGER REFERENCES events (id) ON DELETE SET NULL,
    event_remote_id TEXT NOT NULL,
    op              TEXT NOT NULL CHECK (op IN ('create', 'update', 'delete')),
    payload         TEXT,                       -- JSON: the state the user wants; NULL for delete
    base            TEXT,                       -- JSON: the state before the edit; NULL for create
    base_etag       TEXT,                       -- the ETag the edit was based on
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'in_flight', 'conflicted', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    created_at      INTEGER NOT NULL
) STRICT;

CREATE INDEX outbox_due ON outbox (status, next_attempt_at);
CREATE INDEX outbox_event ON outbox (event_id);
