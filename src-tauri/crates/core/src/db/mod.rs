//! SQLite storage: one write connection, one read connection, and the schema migrations.
//!
//! See `docs/architecture.md` §6. All writes go through the write connection, so there is
//! never a second writer. The read connection serves the UI; in WAL mode it is not blocked by
//! an open write transaction and sees the last committed state.

pub(crate) mod migrations;
pub mod occurrences;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use migrations::MIGRATIONS;

/// How long a connection waits for a lock held by another connection before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Page cache per connection. Negative means KiB, so this is 2000 KiB.
const CACHE_SIZE_KIB: i64 = -2000;

/// Errors from the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    /// The blocking task panicked or was cancelled.
    #[error("database task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// The database: a write connection and a read connection to the same file.
///
/// Cloning is cheap and gives another handle to the same two connections.
//
// Rust note: a `Connection` may move between threads (`Send`) but not be used from two threads
// at once (it is not `Sync`). The `Mutex` makes sure only one thread uses it at a time, and the
// `Arc` (a reference-counted pointer) lets several owners share that one `Mutex`.
#[derive(Clone)]
pub struct Db {
    write: Arc<Mutex<Connection>>,
    read: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (or creates) the database at `path` and runs all pending migrations.
    ///
    /// This blocks; it is meant to be called once at startup. The parent directory must exist.
    pub fn open(path: &Path) -> Result<Db, DbError> {
        // The writer goes first: it creates the file and switches it to WAL before the
        // read-only connection opens it.
        let mut write = Connection::open(path)?;
        configure_write(&write)?;
        MIGRATIONS.to_latest(&mut write)?;

        let read = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure_read(&read)?;

        Ok(Db {
            write: Arc::new(Mutex::new(write)),
            read: Arc::new(Mutex::new(read)),
        })
    }

    /// Runs `f` on the write connection in a blocking thread.
    ///
    /// Must be called from inside a Tokio runtime (Tauri commands run in one).
    //
    // Rust note: `Send + 'static` is what `spawn_blocking` requires of anything it moves to
    // another thread. `Send`: it is safe to hand to that thread. `'static`: it borrows nothing
    // from the caller's stack, which could be gone by the time the thread runs. That is why
    // the closure takes its connection as an argument instead of borrowing `self`.
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.write);
        // The first `?` handles a panicked task, the second the SQLite error from `f`.
        Ok(tokio::task::spawn_blocking(move || f(&mut lock(&conn))).await??)
    }

    /// Runs `f` on the read-only connection in a blocking thread.
    ///
    /// It sees the last committed state and is never blocked by an open write transaction.
    pub async fn read<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.read);
        Ok(tokio::task::spawn_blocking(move || f(&lock(&conn))).await??)
    }
}

/// Locks a connection, also when an earlier user panicked while holding it.
//
// Rust note: when a thread panics while it holds a `Mutex`, the mutex is "poisoned" and
// `lock()` returns an error from then on. Here that error is safe to ignore: a panic inside a
// transaction drops the `Transaction`, which rolls it back, so the connection is still in a
// clean state. Without this, one panic would make the database unusable until restart.
fn lock(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pragmas for the write connection. Also used by the in-memory test connection.
pub(crate) fn configure_write(conn: &Connection) -> rusqlite::Result<()> {
    // `journal_mode` answers with the mode that is now active, so it needs the `_and_check`
    // variant, which reads that row. The mode is stored in the file; readers inherit it.
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    // Off by default in SQLite; without it `ON DELETE CASCADE` does nothing.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "cache_size", CACHE_SIZE_KIB)
}

/// Pragmas for the read connection.
fn configure_read(conn: &Connection) -> rusqlite::Result<()> {
    // The connection is already opened read-only; `query_only` also rejects statements such
    // as `PRAGMA user_version = 2` that would otherwise still change the file.
    conn.pragma_update(None, "query_only", "ON")?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "cache_size", CACHE_SIZE_KIB)
}
