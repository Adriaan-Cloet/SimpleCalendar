use std::time::Duration;

use rusqlite::{Connection, params};
use tempfile::TempDir;
use tokio::sync::oneshot;

use super::migrations::{MIGRATIONS, test_connection};
use super::*;
use crate::model::LOCAL_SOURCE_ID;

// ---- helpers -------------------------------------------------------------------------------

fn insert_source(conn: &Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO sources (id, kind, display_name, created_at) VALUES (?1, 'ics', 'Feed', 0)",
        params![id],
    )
}

/// Inserts a source with one calendar and returns the calendar id.
fn insert_calendar(conn: &Connection) -> i64 {
    insert_source(conn, "src-1").unwrap();
    conn.execute(
        "INSERT INTO calendars (source_id, remote_id, name, access)
         VALUES ('src-1', 'cal-1', 'Calendar', 'read')",
        [],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn open_temp() -> (TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.db")).unwrap();
    // The directory is returned so it lives as long as the test; dropping it deletes the files.
    (dir, db)
}

// ---- schema (in-memory) --------------------------------------------------------------------

#[test]
fn migrations_are_valid() {
    MIGRATIONS.validate().unwrap();
}

#[test]
fn user_version_is_latest_migration() {
    assert_eq!(user_version(&test_connection()), 7);
}

#[test]
fn a_new_database_has_the_local_source_and_no_calendars() {
    let conn = test_connection();
    let sources: Vec<(String, String)> = conn
        .prepare("SELECT id, kind FROM sources")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        sources,
        vec![(LOCAL_SOURCE_ID.to_owned(), "local".to_owned())]
    );
    let calendars: i64 = conn
        .query_row("SELECT count(*) FROM calendars", [], |r| r.get(0))
        .unwrap();
    assert_eq!(calendars, 0);
}

#[test]
fn a_v0_1_database_keeps_its_events_and_gets_the_google_columns() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    super::configure_write(&conn).unwrap();
    MIGRATIONS.to_version(&mut conn, 2).unwrap();
    conn.execute_batch(
        "INSERT INTO sources (id, kind, display_name, created_at) VALUES ('s', 'ics', 'S', 0);
         INSERT INTO calendars (id, source_id, remote_id, name, access)
             VALUES (1, 's', 'c', 'C', 'read');
         INSERT INTO events (calendar_id, remote_id, start_utc, end_utc) VALUES (1, 'e', 0, 0);",
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let row: (Option<String>, bool, bool, bool, String) = conn
        .query_row(
            "SELECT e.etag, e.is_organizer, e.guests_can_modify, e.use_default_reminders,
                    c.default_reminders
             FROM events e JOIN calendars c ON c.id = e.calendar_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row, (None, true, false, true, "[]".into()));
}

#[test]
fn base_tables_are_strict() {
    let conn = test_connection();
    for table in ["sources", "calendars", "events", "settings"] {
        let strict: i64 = conn
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                params![table],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("table {table} missing: {e}"));
        assert_eq!(strict, 1, "{table} is not STRICT");
    }
}

#[test]
fn range_query_indexes_exist() {
    let conn = test_connection();
    for index in [
        "events_single",
        "events_masters",
        "events_exceptions",
        "events_unlinked",
    ] {
        let found: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                params![index],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "index {index} missing");
    }
}

#[test]
fn strict_rejects_wrong_type() {
    let conn = test_connection();
    let cal = insert_calendar(&conn);
    let result = conn.execute(
        "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc)
         VALUES (?1, 'e', 'abc', 0)",
        params![cal],
    );
    assert!(result.is_err(), "text accepted in an INTEGER column");
}

#[test]
fn checks_reject_invalid_rows() {
    let conn = test_connection();
    let cal = insert_calendar(&conn);
    let event = |sql_cols: &str, sql_vals: &str| {
        conn.execute(
            &format!(
                "INSERT INTO events (calendar_id, remote_id, {sql_cols}) VALUES (?1, {sql_vals})"
            ),
            params![cal],
        )
    };

    assert!(
        event("start_utc, end_utc", "'a', 10, 5").is_err(),
        "end before start accepted"
    );
    assert!(
        event(
            "start_utc, end_utc, recurring_remote_id",
            "'b', 0, 10, 'master'"
        )
        .is_err(),
        "exception without original_start_utc accepted"
    );
    assert!(
        event(
            "start_utc, end_utc, recurrence, recurring_remote_id, original_start_utc",
            "'c', 0, 10, 'RRULE:FREQ=DAILY', 'master', 0"
        )
        .is_err(),
        "master that is also an exception accepted"
    );
    assert!(
        conn.execute(
            "INSERT INTO sources (id, kind, display_name, created_at)
             VALUES ('x', 'dropbox', 'X', 0)",
            [],
        )
        .is_err(),
        "unknown source kind accepted"
    );

    // Control: a valid event is accepted, so the failures above come from the checks.
    event("start_utc, end_utc", "'ok', 0, 10").unwrap();
}

#[test]
fn deleting_source_cascades() {
    let conn = test_connection();
    let cal = insert_calendar(&conn);
    conn.execute(
        "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc) VALUES (?1, 'e', 0, 10)",
        params![cal],
    )
    .unwrap();

    conn.execute("DELETE FROM sources WHERE id = 'src-1'", [])
        .unwrap();

    assert_eq!(count(&conn, "calendars"), 0);
    assert_eq!(count(&conn, "events"), 0);
}

#[test]
fn upsert_keeps_row_id_and_exceptions() {
    let conn = test_connection();
    let cal = insert_calendar(&conn);
    let upsert_master = |title: &str| {
        conn.execute(
            "INSERT INTO events (calendar_id, remote_id, title, start_utc, end_utc, recurrence)
             VALUES (?1, 'master', ?2, 0, 3600, 'RRULE:FREQ=DAILY')
             ON CONFLICT (calendar_id, remote_id) DO UPDATE SET title = excluded.title",
            params![cal, title],
        )
        .unwrap();
        conn.query_row(
            "SELECT id FROM events WHERE remote_id = 'master'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };

    let id = upsert_master("Standup");
    conn.execute(
        "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc,
                             recurring_remote_id, recurring_event_id, original_start_utc)
         VALUES (?1, 'master_1', 86400, 90000, 'master', ?2, 86400)",
        params![cal, id],
    )
    .unwrap();

    assert_eq!(
        upsert_master("Daily standup"),
        id,
        "upsert changed the row id"
    );
    assert_eq!(count(&conn, "events"), 2, "upsert removed the exception");
}

// ---- connections (file in a temp directory) ------------------------------------------------

#[tokio::test]
async fn write_connection_uses_wal_and_foreign_keys() {
    let (_dir, db) = open_temp();
    let (mode, fk) = db
        .write(|c| {
            let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
            let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
            Ok((mode, fk))
        })
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(fk, 1);
}

#[tokio::test]
async fn read_connection_rejects_writes() {
    let (_dir, db) = open_temp();
    let result = db.read(|c| insert_source(c, "nope")).await;
    assert!(
        result.is_err(),
        "write through the read connection succeeded"
    );
}

#[tokio::test]
async fn open_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    drop(Db::open(&path).unwrap());
    let db = Db::open(&path).unwrap();
    assert_eq!(db.read(|c| Ok(user_version(c))).await.unwrap(), 7);
}

#[tokio::test]
async fn read_sees_committed_state_while_write_is_open() {
    let (_dir, db) = open_temp();
    db.write(|c| insert_source(c, "a")).await.unwrap();

    // The writer inserts "b" inside a transaction, tells us, then waits until we let it commit.
    let (inserted_tx, inserted_rx) = oneshot::channel::<()>();
    let (commit_tx, commit_rx) = oneshot::channel::<()>();
    let writer = tokio::spawn({
        let db = db.clone();
        async move {
            db.write(move |c| {
                let tx = c.transaction()?;
                insert_source(&tx, "b")?;
                inserted_tx.send(()).unwrap();
                commit_rx.blocking_recv().unwrap();
                tx.commit()
            })
            .await
        }
    });
    inserted_rx.await.unwrap();

    // The write transaction is open now. The read must not wait for it, and must not see "b".
    // The local source (migration 4) and "a" are there.
    let during = tokio::time::timeout(Duration::from_secs(1), db.read(|c| Ok(count(c, "sources"))))
        .await
        .expect("read blocked by the open write transaction")
        .unwrap();
    assert_eq!(during, 2);

    commit_tx.send(()).unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(db.read(|c| Ok(count(c, "sources"))).await.unwrap(), 3);
}
