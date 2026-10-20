//! Calendar logic for SimpleCalendar: model, storage, recurrence and sync.
//!
//! This crate must never depend on `tauri`. The app crate in `src-tauri/` wraps it in
//! Tauri commands; see `docs/architecture.md` §3.

pub mod db;
pub mod model;
pub mod recurrence;

/// Version of this crate, as set in its `Cargo.toml`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!version().is_empty());
    }
}
