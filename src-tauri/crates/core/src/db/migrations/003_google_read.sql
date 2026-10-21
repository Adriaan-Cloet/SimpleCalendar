-- Migration 3: what Google read needs (docs/architecture.md §4, §6, §7.5).
-- `etag` is sent back as If-Match by Google write. An event is editable only if its calendar is
-- writable and (`is_organizer` or `guests_can_modify`); local events are always the organizer.
-- Reminders: the event's own (`event_reminders`), or its calendar's defaults when
-- `use_default_reminders` is set. `default_reminders` holds only popup reminders, as minutes
-- before the start, because only those produce a notification; an event keeps its `email`
-- reminders so they survive being saved back to Google.

ALTER TABLE events ADD COLUMN etag TEXT;
ALTER TABLE events ADD COLUMN is_organizer INTEGER NOT NULL DEFAULT 1
    CHECK (is_organizer IN (0, 1));
ALTER TABLE events ADD COLUMN guests_can_modify INTEGER NOT NULL DEFAULT 0
    CHECK (guests_can_modify IN (0, 1));
ALTER TABLE events ADD COLUMN use_default_reminders INTEGER NOT NULL DEFAULT 1
    CHECK (use_default_reminders IN (0, 1));

ALTER TABLE calendars ADD COLUMN default_reminders TEXT NOT NULL DEFAULT '[]';

CREATE TABLE event_reminders (
    event_id       INTEGER NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    method         TEXT NOT NULL DEFAULT 'popup' CHECK (method IN ('popup', 'email')),
    minutes_before INTEGER NOT NULL CHECK (minutes_before BETWEEN 0 AND 40320),
    PRIMARY KEY (event_id, method, minutes_before)
) STRICT, WITHOUT ROWID;
