-- Migration 7: the reminders that were shown, so none is shown twice, also across a restart
-- (docs/architecture.md §9). `occurrence_start` is the instant the occurrence starts, in the
-- display zone for all-day and floating events: a moved event gets its reminders again. Rows
-- older than a week are pruned at startup. The display zone itself is the `display_zone` row in
-- `settings`, which needs no schema change.

CREATE TABLE fired_reminders (
    event_id         INTEGER NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    occurrence_start INTEGER NOT NULL,
    minutes_before   INTEGER NOT NULL,
    fired_at         INTEGER NOT NULL,
    PRIMARY KEY (event_id, occurrence_start, minutes_before)
) STRICT, WITHOUT ROWID;
