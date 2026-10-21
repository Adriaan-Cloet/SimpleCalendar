-- Migration 4: the built-in local source (docs/architecture.md §4, §5). It has no provider, is
-- never synced and holds no secret, so its id is the fixed `local` instead of a UUID. It starts
-- without calendars; `create_local_calendar` adds them.

INSERT INTO sources (id, kind, display_name, created_at)
VALUES ('local', 'local', 'On this computer', CAST(strftime('%s', 'now') AS INTEGER));
