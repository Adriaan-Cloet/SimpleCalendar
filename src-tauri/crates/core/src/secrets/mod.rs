//! Secrets in the OS keychain (`docs/architecture.md` §8).
//!
//! Feed URLs, refresh tokens and passwords are stored only in the keychain: Secret Service on
//! Linux, Credential Manager on Windows. There is no fallback to a file or to SQLite, in any
//! case. Without a keychain the app keeps working for local calendars, and adding a remote
//! source is refused with a `SecretsUnavailable` the frontend can show.

mod keychain;
mod memory;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

pub use keychain::KeychainStore;
pub use memory::MemoryStore;

/// A keychain backend. The calls block (on Linux the keyring may show an unlock prompt), so
/// they are only made through `Secrets`, which runs them in `spawn_blocking`.
///
/// `account` is the full account name, such as `source:<uuid>`. Values are opaque strings here;
/// `Secrets` encodes and decodes them.
pub trait SecretStore: Send + Sync {
    /// A harmless read that tells whether the keychain can be used at all.
    fn probe(&self) -> Result<(), SecretError>;
    /// The value for `account`, or `None` if there is none.
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    /// Creates or replaces the value for `account`.
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError>;
    /// Removes the value for `account`. Removing a value that does not exist is not an error.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// What is stored for one source.
//
// Rust note: `#[serde(tag = "kind")]` writes the variant name into the JSON object itself, as
// `{"kind": "ics_url", "url": ...}`, instead of nesting it as `{"IcsUrl": {...}}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Secret {
    /// The full URL of an ICS feed. Private feed URLs contain a token that grants access.
    IcsUrl { url: String },
    /// A Google account's refresh token. Its access tokens live in memory only (§8).
    GoogleRefreshToken { refresh_token: String },
    /// The secret of the user's own Google OAuth client, the override of the compiled-in one.
    /// Stored under an app account, not a source (`app_account`).
    ClientSecret { secret: String },
    /// A CalDAV account: the address the user gave, and the login for it (§8). The address is
    /// kept here too, so the three are stored and removed together.
    Caldav {
        url: String,
        username: String,
        password: String,
    },
}

/// Never prints the secret itself, so a stray `{:?}` in a log line cannot leak it.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Secret::IcsUrl { .. } => f.write_str("IcsUrl { url: <redacted> }"),
            Secret::GoogleRefreshToken { .. } => {
                f.write_str("GoogleRefreshToken { refresh_token: <redacted> }")
            }
            Secret::ClientSecret { .. } => f.write_str("ClientSecret { secret: <redacted> }"),
            Secret::Caldav { .. } => f.write_str("Caldav { <redacted> }"),
        }
    }
}

/// Version of the stored JSON. A value with another version is refused rather than guessed at.
const FORMAT_VERSION: u32 = 1;

/// The JSON document in the keychain: `{"v": 1, "kind": ..., ...}`.
//
// Rust note: `#[serde(flatten)]` puts the fields of `secret` next to `v` in the same object
// instead of under a `secret` key.
#[derive(Serialize, Deserialize)]
struct Stored {
    v: u32,
    #[serde(flatten)]
    secret: Secret,
}

fn encode(secret: &Secret) -> String {
    let stored = Stored {
        v: FORMAT_VERSION,
        secret: secret.clone(),
    };
    // Rust note: serialising a plain struct with string fields to JSON cannot fail, so this
    // `expect` is never reached.
    serde_json::to_string(&stored).expect("secret serialises to JSON")
}

fn decode(raw: &str) -> Result<Secret, SecretError> {
    // The error text from serde_json names the position, never the content.
    let stored: Stored =
        serde_json::from_str(raw).map_err(|e| SecretError::BadFormat(e.to_string()))?;
    if stored.v != FORMAT_VERSION {
        return Err(SecretError::BadFormat(format!("version {}", stored.v)));
    }
    Ok(stored.secret)
}

/// Why the keychain cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// No keyring service is running (Linux without GNOME Keyring or KWallet).
    NoService,
    /// The keyring is locked, or the unlock prompt was dismissed.
    Locked,
}

/// The structured error the frontend receives (§8, §10): it shows `message` and offers to try
/// again. Serialises as `{"kind": "secrets_unavailable", "reason": ..., "message": ...}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct SecretsUnavailable {
    pub reason: UnavailableReason,
}

impl SecretsUnavailable {
    /// The text from the architecture, telling the user what to do.
    pub fn message(&self) -> &'static str {
        match self.reason {
            UnavailableReason::NoService => {
                "SimpleCalendar could not find a system keyring. Account credentials are stored \
                 in the system keyring (GNOME Keyring or KWallet), never in files. Install and \
                 start `gnome-keyring`, log in again and restart SimpleCalendar. Local calendars \
                 keep working."
            }
            UnavailableReason::Locked => "The system keyring is locked. Unlock it and try again.",
        }
    }
}

impl Serialize for SecretsUnavailable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("SecretsUnavailable", 3)?;
        s.serialize_field("kind", "secrets_unavailable")?;
        s.serialize_field("reason", &self.reason)?;
        s.serialize_field("message", self.message())?;
        s.end()
    }
}

/// Errors from the secret store.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error(transparent)]
    Unavailable(#[from] SecretsUnavailable),
    /// A stored value could not be read back: not JSON, or a version this build does not know.
    #[error("stored secret has an unknown format: {0}")]
    BadFormat(String),
    /// Any other keychain failure.
    #[error("keychain: {0}")]
    Platform(String),
    /// The blocking task panicked or was cancelled.
    #[error("keychain task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// The secrets of all sources, on top of a `SecretStore`.
///
/// Cloning is cheap and gives another handle to the same store and the same status.
#[derive(Clone)]
pub struct Secrets {
    store: Arc<dyn SecretStore>,
    /// The result of the last probe: `None` while the keychain is usable.
    unavailable: Arc<Mutex<Option<SecretsUnavailable>>>,
}

impl Secrets {
    pub fn new(store: Arc<dyn SecretStore>) -> Secrets {
        Secrets {
            store,
            unavailable: Arc::new(Mutex::new(None)),
        }
    }

    /// Probes the keychain and remembers the result. Called once at startup.
    pub async fn probe(&self) -> Result<(), SecretsUnavailable> {
        match self.run(|store| store.probe()).await {
            Err(SecretError::Unavailable(unavailable)) => Err(unavailable),
            // Neither of the two cases the user can act on. Adding a source will report the
            // actual error, so the keychain is not declared unusable for it.
            Err(other) => {
                log::warn!("keychain probe failed: {other}");
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Whether the keychain is usable. After a failed probe this probes again, so a "Try again"
    /// button only has to ask for the status.
    pub async fn status(&self) -> Option<SecretsUnavailable> {
        let last = *lock(&self.unavailable);
        match last {
            Some(_) => self.probe().await.err(),
            None => None,
        }
    }

    /// Fails with `SecretsUnavailable` if the keychain cannot be used. Adding a remote source
    /// calls this first.
    pub async fn require(&self) -> Result<(), SecretsUnavailable> {
        match self.status().await {
            Some(unavailable) => Err(unavailable),
            None => Ok(()),
        }
    }

    /// The secret of `source_id`, or `None` if it has none.
    pub async fn get(&self, source_id: &str) -> Result<Option<Secret>, SecretError> {
        self.get_account(account(source_id)).await
    }

    /// Stores the secret of `source_id`, replacing any earlier one.
    pub async fn set(&self, source_id: &str, secret: &Secret) -> Result<(), SecretError> {
        self.set_account(account(source_id), secret).await
    }

    /// Removes the secret of `source_id`, if there is one.
    pub async fn delete(&self, source_id: &str) -> Result<(), SecretError> {
        self.delete_account(account(source_id)).await
    }

    /// An app-wide secret that belongs to no source, such as `google-client-secret`.
    pub async fn get_app(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        self.get_account(app_account(name)).await
    }

    pub async fn set_app(&self, name: &str, secret: &Secret) -> Result<(), SecretError> {
        self.set_account(app_account(name), secret).await
    }

    pub async fn delete_app(&self, name: &str) -> Result<(), SecretError> {
        self.delete_account(app_account(name)).await
    }

    async fn get_account(&self, account: String) -> Result<Option<Secret>, SecretError> {
        let raw = self.run(move |store| store.get(&account)).await?;
        raw.as_deref().map(decode).transpose()
    }

    async fn set_account(&self, account: String, secret: &Secret) -> Result<(), SecretError> {
        let value = encode(secret);
        self.run(move |store| store.set(&account, &value)).await
    }

    async fn delete_account(&self, account: String) -> Result<(), SecretError> {
        self.run(move |store| store.delete(&account)).await
    }

    /// Runs `f` on the store in a blocking thread and remembers whether the keychain answered.
    //
    // Rust note: the closure gets `&dyn SecretStore`, a reference to "some type that implements
    // the trait". Which type it is, the keychain or the in-memory store, is decided at runtime.
    async fn run<T, F>(&self, f: F) -> Result<T, SecretError>
    where
        F: FnOnce(&dyn SecretStore) -> Result<T, SecretError> + Send + 'static,
        T: Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let result = tokio::task::spawn_blocking(move || f(store.as_ref())).await?;
        match &result {
            Ok(_) => *lock(&self.unavailable) = None,
            Err(SecretError::Unavailable(unavailable)) => {
                *lock(&self.unavailable) = Some(*unavailable)
            }
            Err(_) => {}
        }
        result
    }
}

/// Locks the status, also after a panic elsewhere: it is a plain value that is always valid.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The keychain account of a source (§8).
pub fn account(source_id: &str) -> String {
    format!("source:{source_id}")
}

/// The keychain account of an app-wide secret. Source ids are UUIDs, so the two never collide.
pub fn app_account(name: &str) -> String {
    format!("app:{name}")
}
