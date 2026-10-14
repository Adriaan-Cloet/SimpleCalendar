-- Migration 1: the v0.1 schema (docs/architecture.md §6).
-- Columns that later versions need (etag, is_organizer, guests_can_modify, pending_delete,
-- default_reminders, use_default_reminders) are added by the migration of their own feature.

CREATE TABLE sources (
    id              TEXT PRIMARY KEY,           -- UUID; also the keychain key
    kind            TEXT NOT NULL CHECK (kind IN ('local', 'ics', 'google', 'caldav')),
    display_name    TEXT NOT NULL,
    account_label   TEXT,                       -- Google account or ICS host; never a secret
    status          TEXT NOT NULL DEFAULT 'ok'
                    CHECK (status IN ('ok', 'auth_required', 'error')),
    last_error      TEXT,
    sync_interval_s INTEGER,                    -- NULL = provider default
    created_at      INTEGER NOT NULL
) STRICT;

CREATE TABLE calendars (
    id             INTEGER PRIMARY KEY,
    source_id      TEXT NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    remote_id      TEXT NOT NULL,               -- provider's calendar id; a UUID for local ones
    name           TEXT NOT NULL,
    color          TEXT,                        -- '#rrggbb' from the provider
    color_override TEXT,                        -- user's choice, never synced
    time_zone      TEXT,                        -- IANA name; default zone for new events
    access         TEXT NOT NULL CHECK (access IN ('read', 'write')),
    visible        INTEGER NOT NULL DEFAULT 1 CHECK (visible IN (0, 1)),
    is_primary     INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1)),
    sync_cursor    TEXT,                        -- opaque, owned by the provider
    last_synced_at INTEGER,
    UNIQUE (source_id, remote_id)
) STRICT;

CREATE TABLE events (
    id                  INTEGER PRIMARY KEY,
    calendar_id         INTEGER NOT NULL REFERENCES calendars (id) ON DELETE CASCADE,
    remote_id           TEXT NOT NULL,          -- provider's id; generated here for new events
    ical_uid            TEXT,
    status              TEXT NOT NULL DEFAULT 'confirmed'
                        CHECK (status IN ('confirmed', 'tentative', 'cancelled')),
    title               TEXT NOT NULL DEFAULT '',
    description         TEXT,
    location            TEXT,
    color               TEXT,
    all_day             INTEGER NOT NULL DEFAULT 0 CHECK (all_day IN (0, 1)),
    start_utc           INTEGER NOT NULL,       -- epoch seconds
    end_utc             INTEGER NOT NULL,       -- epoch seconds, exclusive
    start_tz            TEXT,                   -- IANA name, '+hh:mm', or NULL (floating, all-day)
    end_tz              TEXT,
    recurrence          TEXT,                   -- RRULE/RDATE/EXDATE lines; NULL if not a master
    recurrence_end_utc  INTEGER,                -- end of the last occurrence; NULL = no end
    recurring_remote_id TEXT,                   -- exceptions: remote id of the master, always set
    recurring_event_id  INTEGER REFERENCES events (id) ON DELETE CASCADE,
                                                -- exceptions: master's row id, once it exists
    original_start_utc  INTEGER,                -- the occurrence an exception replaces
    remote_updated_at   INTEGER,
    UNIQUE (calendar_id, remote_id),
    CHECK (end_utc >= start_utc),
    CHECK ((recurring_remote_id IS NULL) = (original_start_utc IS NULL)),
    CHECK (recurring_event_id IS NULL OR recurring_remote_id IS NOT NULL),
    CHECK (recurrence IS NULL OR recurring_remote_id IS NULL)
) STRICT;

CREATE INDEX events_single ON events (calendar_id, start_utc, end_utc)
    WHERE recurrence IS NULL;
CREATE INDEX events_masters ON events (calendar_id)
    WHERE recurrence IS NOT NULL;
CREATE INDEX events_exceptions ON events (recurring_event_id)
    WHERE recurring_event_id IS NOT NULL;
CREATE INDEX events_unlinked ON events (calendar_id, recurring_remote_id)
    WHERE recurring_remote_id IS NOT NULL AND recurring_event_id IS NULL;

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;
