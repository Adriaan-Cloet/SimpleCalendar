//! SQLite storage: one write connection, one read connection, and the schema migrations.
//!
//! See `docs/architecture.md` §6. All writes go through the write connection, so there is
//! never a second writer. The read connection serves the UI; in WAL mode it is not blocked by
//! an open write transaction and sees the last committed state.

mod migrations;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

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
#[derive(Clone)]
pub struct Db {
    write: Arc<Mutex<Connection>>,
    read: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (or creates) the database at `path` and runs all pending migrations.
    pub fn open(path: &Path) -> Result<Db, DbError> {
        let _ = path;
        todo!()
    }

    /// Runs `f` on the write connection in a blocking thread.
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let _ = (&self.write, f);
        todo!()
    }

    /// Runs `f` on the read-only connection in a blocking thread.
    pub async fn read<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let _ = (&self.read, f);
        todo!()
    }
}

/// Pragmas for the write connection. Also used by the in-memory test connection.
pub(crate) fn configure_write(conn: &Connection) -> rusqlite::Result<()> {
    let _ = conn;
    todo!()
}
