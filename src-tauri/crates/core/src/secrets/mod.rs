//! Secrets in the OS keychain (`docs/architecture.md` §8).
//!
//! Feed URLs, refresh tokens and passwords are stored only in the keychain: Secret Service on
//! Linux, Credential Manager on Windows. There is no fallback to a file or to SQLite, in any
//! case. Without a keychain the app keeps working for local calendars, and adding a remote
//! source is refused with a `SecretsUnavailable` the frontend can show.

mod keychain;
mod memory;

use std::fmt;
use std::sync::{Arc, Mutex};

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
}

/// Never prints the secret itself, so a stray `{:?}` in a log line cannot leak it.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
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
        todo!()
    }
}

impl Serialize for SecretsUnavailable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        todo!()
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
        todo!()
    }

    /// Probes the keychain and remembers the result. Called once at startup.
    pub async fn probe(&self) -> Result<(), SecretsUnavailable> {
        todo!()
    }

    /// Whether the keychain is usable. After a failed probe this probes again, so a "Try again"
    /// button only has to ask for the status.
    pub async fn status(&self) -> Option<SecretsUnavailable> {
        todo!()
    }

    /// Fails with `SecretsUnavailable` if the keychain cannot be used. Adding a remote source
    /// calls this first.
    pub async fn require(&self) -> Result<(), SecretsUnavailable> {
        todo!()
    }

    /// The secret of `source_id`, or `None` if it has none.
    pub async fn get(&self, source_id: &str) -> Result<Option<Secret>, SecretError> {
        todo!()
    }

    /// Stores the secret of `source_id`, replacing any earlier one.
    pub async fn set(&self, source_id: &str, secret: &Secret) -> Result<(), SecretError> {
        todo!()
    }

    /// Removes the secret of `source_id`, if there is one.
    pub async fn delete(&self, source_id: &str) -> Result<(), SecretError> {
        todo!()
    }
}

/// The keychain account of a source (§8).
pub fn account(source_id: &str) -> String {
    format!("source:{source_id}")
}
