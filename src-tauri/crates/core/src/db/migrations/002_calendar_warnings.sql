-- Migration 2: warnings about a calendar's data, such as ICS time zones that could not be
-- resolved (docs/architecture.md §7.7). A JSON array of messages, sorted, each once.

ALTER TABLE calendars ADD COLUMN warnings TEXT NOT NULL DEFAULT '[]';
