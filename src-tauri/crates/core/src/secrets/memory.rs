//! An in-memory `SecretStore` for tests. It never touches the real keychain.

use std::collections::HashMap;
use std::sync::Mutex;

use super::{SecretError, SecretStore, UnavailableReason};

/// Keeps values in a map. `set_unavailable` makes every call fail as a missing or locked
/// keychain would.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
    unavailable: Mutex<Option<UnavailableReason>>,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// From now on, every call fails with this reason, or works again with `None`.
    pub fn set_unavailable(&self, reason: Option<UnavailableReason>) {
        todo!()
    }

    /// The raw stored value, to check the encoding in tests.
    pub fn raw(&self, account: &str) -> Option<String> {
        todo!()
    }
}

impl SecretStore for MemoryStore {
    fn probe(&self) -> Result<(), SecretError> {
        todo!()
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        todo!()
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        todo!()
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        todo!()
    }
}
