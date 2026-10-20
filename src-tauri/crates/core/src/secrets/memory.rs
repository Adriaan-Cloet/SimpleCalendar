//! An in-memory `SecretStore` for tests. It never touches the real keychain.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{SecretError, SecretStore, SecretsUnavailable, UnavailableReason};

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
        *lock(&self.unavailable) = reason;
    }

    /// The raw stored value, to check the encoding in tests.
    pub fn raw(&self, account: &str) -> Option<String> {
        lock(&self.values).get(account).cloned()
    }

    fn check(&self) -> Result<(), SecretError> {
        match *lock(&self.unavailable) {
            Some(reason) => Err(SecretsUnavailable { reason }.into()),
            None => Ok(()),
        }
    }
}

impl SecretStore for MemoryStore {
    fn probe(&self) -> Result<(), SecretError> {
        self.check()
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        self.check()?;
        Ok(self.raw(account))
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        self.check()?;
        lock(&self.values).insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.check()?;
        lock(&self.values).remove(account);
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
