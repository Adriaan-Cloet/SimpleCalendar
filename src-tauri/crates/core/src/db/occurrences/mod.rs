//! Reading a date range: the occurrences the views show (`docs/architecture.md` §6 "Reading a
//! date range").
//!
//! Two queries on the read connection, then expansion in Rust. Nothing here writes: occurrences
//! are computed per query and never stored (§4).

use chrono_tz::Tz;
use rusqlite::Connection;

use crate::model::Occurrence;

/// Every occurrence that overlaps `[from, to)` on a visible calendar, sorted by start.
///
/// `from` and `to` are UTC instants (epoch seconds). Times come back in the `viewer` zone.
/// Run it inside `Db::read`. A series whose recurrence cannot be expanded is logged and shows
/// only its first occurrence, so one broken event never hides the rest.
pub fn list_occurrences(
    conn: &Connection,
    from: i64,
    to: i64,
    viewer: Tz,
) -> rusqlite::Result<Vec<Occurrence>> {
    todo!()
}
