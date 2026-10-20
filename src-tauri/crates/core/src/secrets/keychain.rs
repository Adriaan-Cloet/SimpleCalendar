//! The OS keychain through `keyring-core`: Secret Service on Linux, Credential Manager on
//! Windows.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use keyring_core::{CredentialStore, Entry, Error};

use super::{SecretError, SecretStore, SecretsUnavailable, UnavailableReason};

/// The account the probe reads. It never holds a value; reading it only tells whether the
/// keychain answers.
const PROBE_ACCOUNT: &str = "probe";

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

impl KeychainStore {
    /// The connected platform store, connecting first if needed.
    fn store(&self) -> Result<Arc<CredentialStore>, SecretError> {
        let mut store = lock(&self.store);
        if let Some(store) = store.as_ref() {
            return Ok(Arc::clone(store));
        }
        let connected = connect().map_err(connect_error)?;
        *store = Some(Arc::clone(&connected));
        Ok(connected)
    }

    /// Runs `f` on the entry for `account`. After a failure the connection is dropped, so the
    /// next call connects again instead of reusing one to a keyring that went away.
    fn with_entry<T>(
        &self,
        account: &str,
        f: impl FnOnce(&Entry) -> keyring_core::Result<T>,
    ) -> Result<T, SecretError> {
        let result = self
            .store()?
            .build(&self.service, account, None)
            .and_then(|entry| f(&entry));
        result.map_err(|err| {
            *lock(&self.store) = None;
            entry_error(err)
        })
    }
}

impl SecretStore for KeychainStore {
    fn probe(&self) -> Result<(), SecretError> {
        self.with_entry(PROBE_ACCOUNT, |entry| match entry.get_password() {
            Ok(_) | Err(Error::NoEntry) => Ok(()),
            Err(err) => Err(err),
        })
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        self.with_entry(account, |entry| match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(err) => Err(err),
        })
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        self.with_entry(account, |entry| entry.set_password(value))
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.with_entry(account, |entry| match entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(err) => Err(err),
        })
    }
}

// Rust note: `#[cfg(...)]` compiles a function only for the matching target, so each platform
// builds exactly one `connect`. The `let` with a type turns the concrete `Arc<Store>` into the
// `Arc<CredentialStore>` (a trait object) that the rest of the code uses.

/// Connects to the Secret Service over D-Bus. Fails when none is running.
#[cfg(target_os = "linux")]
fn connect() -> keyring_core::Result<Arc<CredentialStore>> {
    let store: Arc<CredentialStore> = dbus_secret_service_keyring_store::Store::new()?;
    Ok(store)
}

/// Opens the Windows Credential Manager. Always present on a desktop session.
#[cfg(windows)]
fn connect() -> keyring_core::Result<Arc<CredentialStore>> {
    let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new()?;
    Ok(store)
}

/// Other platforms have no supported keychain yet (macOS and mobile come later, §1).
#[cfg(not(any(target_os = "linux", windows)))]
fn connect() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NotSupportedByStore(
        "no keychain backend for this platform".into(),
    ))
}

/// Maps an error from connecting to the keychain.
///
/// `NoStorageAccess` is how every store reports a locked or denied keychain. Anything else
/// means there is no keychain to talk to: the D-Bus store returns `PlatformFailure` when no
/// Secret Service is running, as `scripts/test-keychain.sh` confirms.
pub(super) fn connect_error(err: Error) -> SecretError {
    let reason = match err {
        Error::NoStorageAccess(_) => UnavailableReason::Locked,
        _ => UnavailableReason::NoService,
    };
    log::warn!("cannot connect to the keychain: {err}");
    SecretsUnavailable { reason }.into()
}

/// Maps an error from reading or writing an entry.
pub(super) fn entry_error(err: Error) -> SecretError {
    match err {
        Error::NoStorageAccess(_) => SecretsUnavailable {
            reason: UnavailableReason::Locked,
        }
        .into(),
        other => SecretError::Platform(other.to_string()),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
