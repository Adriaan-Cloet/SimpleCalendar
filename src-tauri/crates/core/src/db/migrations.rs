//! Schema migrations. The number of the last applied migration is stored in
//! `PRAGMA user_version`. Never edit a migration that has shipped: add a new one.

use rusqlite_migration::{M, Migrations};

// A separate `const` for the slice: `M` has a destructor, and a temporary with a destructor
// cannot be borrowed inside a `const` expression.
const MIGRATION_SLICE: &[M<'static>] = &[
    M::up(include_str!("migrations/001_base.sql")),
    M::up(include_str!("migrations/002_calendar_warnings.sql")),
    M::up(include_str!("migrations/003_google_read.sql")),
    M::up(include_str!("migrations/004_local_source.sql")),
    M::up(include_str!("migrations/005_outbox.sql")),
];

/// All migrations, in order. Migration 1 creates the v0.1 schema (`docs/architecture.md` §6),
/// migration 2 adds `calendars.warnings` (§7.7), migration 3 what Google read needs: the event
/// flags and ETag, and reminders (§7.5), migration 4 the built-in local source (§4), migration
/// 5 the outbox and `events.pending_delete` (§7.3).
pub(crate) const MIGRATIONS: Migrations<'static> = Migrations::from_slice(MIGRATION_SLICE);

/// An in-memory connection with the write pragmas and every migration applied.
/// Query tests start from this.
#[cfg(test)]
pub(crate) fn test_connection() -> rusqlite::Connection {
    let mut conn = rusqlite::Connection::open_in_memory().expect("open in-memory database");
    super::configure_write(&conn).expect("configure connection");
    MIGRATIONS.to_latest(&mut conn).expect("run migrations");
    conn
}
