//! The OS keychain through `keyring-core`: Secret Service on Linux, Credential Manager on
//! Windows.

use std::sync::{Arc, Mutex};

use keyring_core::{CredentialStore, Entry, Error};

use super::{SecretError, SecretStore};

/// The OS keychain. Entries use `service` (the bundle identifier) as service name.
///
/// On Linux, connecting to the Secret Service fails when none is running. The connection is
/// therefore made on first use and retried on every call until it succeeds, so starting a
/// keyring later and pressing "Try again" works without restarting the app.
pub struct KeychainStore {
    service: String,
    store: Mutex<Option<Arc<CredentialStore>>>,
}

impl KeychainStore {
    pub fn new(service: impl Into<String>) -> KeychainStore {
        KeychainStore {
            service: service.into(),
            store: Mutex::new(None),
        }
    }
}

impl SecretStore for KeychainStore {
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

/// Maps an error from connecting to the keychain.
pub(super) fn connect_error(err: Error) -> SecretError {
    todo!()
}

/// Maps an error from reading or writing an entry.
pub(super) fn entry_error(err: Error) -> SecretError {
    todo!()
}
