use std::sync::Arc;
use std::thread::{self, ThreadId};

use keyring_core::Error;
use serde_json::json;

use super::keychain::{connect_error, entry_error};
use super::*;

const NO_SERVICE: SecretsUnavailable = SecretsUnavailable {
    reason: UnavailableReason::NoService,
};
const LOCKED: SecretsUnavailable = SecretsUnavailable {
    reason: UnavailableReason::Locked,
};

fn feed() -> Secret {
    Secret::IcsUrl {
        url: "https://calendar.example.com/private-abc123/basic.ics".into(),
    }
}

fn memory() -> (Arc<MemoryStore>, Secrets) {
    let store = Arc::new(MemoryStore::new());
    // Rust note: `Arc<MemoryStore>` converts to `Arc<dyn SecretStore>` here because
    // `MemoryStore` implements the trait. The test keeps its own handle to inspect the store.
    let secrets = Secrets::new(store.clone());
    (store, secrets)
}

// ---- reading and writing -------------------------------------------------------------------

#[tokio::test]
async fn set_get_delete_round_trip() {
    let (_, secrets) = memory();
    secrets.set("src-1", &feed()).await.unwrap();
    assert_eq!(secrets.get("src-1").await.unwrap(), Some(feed()));

    secrets.delete("src-1").await.unwrap();
    assert_eq!(secrets.get("src-1").await.unwrap(), None);
}

#[tokio::test]
async fn set_replaces_the_earlier_value() {
    let (_, secrets) = memory();
    secrets.set("src-1", &feed()).await.unwrap();
    let other = Secret::IcsUrl {
        url: "https://example.org/other.ics".into(),
    };
    secrets.set("src-1", &other).await.unwrap();
    assert_eq!(secrets.get("src-1").await.unwrap(), Some(other));
}

#[tokio::test]
async fn missing_secret_is_none_and_deleting_it_is_fine() {
    let (_, secrets) = memory();
    assert_eq!(secrets.get("nothing").await.unwrap(), None);
    secrets.delete("nothing").await.unwrap();
}

#[tokio::test]
async fn value_is_small_versioned_json_under_the_source_account() {
    let (store, secrets) = memory();
    secrets.set("4f1c", &feed()).await.unwrap();

    assert_eq!(account("4f1c"), "source:4f1c");
    let raw = store.raw("source:4f1c").expect("stored under source:<id>");
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value,
        json!({
            "v": 1,
            "kind": "ics_url",
            "url": "https://calendar.example.com/private-abc123/basic.ics"
        })
    );
}

#[tokio::test]
async fn unknown_version_or_garbage_is_a_format_error() {
    let (store, secrets) = memory();
    store
        .set(
            "source:new",
            r#"{"v":2,"kind":"ics_url","url":"https://example.org"}"#,
        )
        .unwrap();
    store.set("source:junk", "not json").unwrap();

    for id in ["new", "junk"] {
        assert!(
            matches!(secrets.get(id).await, Err(SecretError::BadFormat(_))),
            "{id}"
        );
    }
}

#[test]
fn debug_output_never_contains_the_secret() {
    let printed = format!("{:?}", feed());
    assert!(!printed.contains("abc123"), "{printed}");
    assert!(printed.contains("IcsUrl"), "{printed}");
}

fn caldav() -> Secret {
    Secret::Caldav {
        url: "https://dav.example.com/".into(),
        username: "alice@example.com".into(),
        password: "app-password-xyz".into(),
    }
}

#[tokio::test]
async fn a_caldav_account_is_stored_whole() {
    let (store, secrets) = memory();
    secrets.set("dav", &caldav()).await.unwrap();

    let raw = store.raw("source:dav").unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value,
        json!({
            "v": 1,
            "kind": "caldav",
            "url": "https://dav.example.com/",
            "username": "alice@example.com",
            "password": "app-password-xyz"
        })
    );
    assert_eq!(secrets.get("dav").await.unwrap(), Some(caldav()));
}

#[test]
fn debug_output_never_contains_the_caldav_password() {
    let printed = format!("{:?}", caldav());
    assert!(!printed.contains("app-password-xyz"), "{printed}");
    assert!(printed.contains("Caldav"), "{printed}");
}

// ---- availability --------------------------------------------------------------------------

#[tokio::test]
async fn probe_reports_and_remembers_why_the_keychain_is_unavailable() {
    let (store, secrets) = memory();
    store.set_unavailable(Some(UnavailableReason::NoService));

    assert_eq!(secrets.probe().await, Err(NO_SERVICE));
    assert_eq!(secrets.status().await, Some(NO_SERVICE));
    assert_eq!(secrets.require().await, Err(NO_SERVICE));
}

#[tokio::test]
async fn writing_without_a_keychain_fails_and_falls_back_to_nothing() {
    let (store, secrets) = memory();
    store.set_unavailable(Some(UnavailableReason::Locked));

    let err = secrets.set("src-1", &feed()).await.unwrap_err();
    assert!(matches!(err, SecretError::Unavailable(LOCKED)), "{err:?}");

    // Nothing was stored anywhere: once the keychain is back, the value is still missing.
    store.set_unavailable(None);
    assert_eq!(secrets.get("src-1").await.unwrap(), None);
}

#[tokio::test]
async fn status_probes_again_after_a_failure() {
    let (store, secrets) = memory();
    store.set_unavailable(Some(UnavailableReason::Locked));
    assert_eq!(secrets.probe().await, Err(LOCKED));

    // The user unlocks the keyring and presses "Try again".
    store.set_unavailable(None);
    assert_eq!(secrets.status().await, None);
    assert_eq!(secrets.require().await, Ok(()));
}

#[tokio::test]
async fn a_working_keychain_reports_no_problem() {
    let (_, secrets) = memory();
    assert_eq!(secrets.probe().await, Ok(()));
    assert_eq!(secrets.status().await, None);
}

#[test]
fn messages_are_the_ones_from_the_architecture() {
    assert_eq!(
        NO_SERVICE.message(),
        "SimpleCalendar could not find a system keyring. Account credentials are stored in \
         the system keyring (GNOME Keyring or KWallet), never in files. Install and start \
         `gnome-keyring`, log in again and restart SimpleCalendar. Local calendars keep working."
    );
    assert_eq!(
        LOCKED.message(),
        "The system keyring is locked. Unlock it and try again."
    );
}

#[test]
fn unavailable_serialises_as_a_structured_error() {
    assert_eq!(
        serde_json::to_value(NO_SERVICE).unwrap(),
        json!({
            "kind": "secrets_unavailable",
            "reason": "no_service",
            "message": NO_SERVICE.message()
        })
    );
    assert_eq!(serde_json::to_value(LOCKED).unwrap()["reason"], "locked");
}

// ---- blocking calls ------------------------------------------------------------------------

/// A store that records the thread every call runs on.
#[derive(Default)]
struct ThreadRecorder(std::sync::Mutex<Vec<ThreadId>>);

impl ThreadRecorder {
    fn record(&self) {
        self.0.lock().unwrap().push(thread::current().id());
    }
}

impl SecretStore for ThreadRecorder {
    fn probe(&self) -> Result<(), SecretError> {
        self.record();
        Ok(())
    }
    fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
        self.record();
        Ok(None)
    }
    fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
        self.record();
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), SecretError> {
        self.record();
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn keychain_calls_run_on_a_blocking_thread() {
    // The runtime has one thread, this one. A call made directly would run here.
    let recorder = Arc::new(ThreadRecorder::default());
    let secrets = Secrets::new(recorder.clone());

    secrets.probe().await.unwrap();
    secrets.get("a").await.unwrap();
    secrets.set("a", &feed()).await.unwrap();
    secrets.delete("a").await.unwrap();

    let threads = recorder.0.lock().unwrap();
    assert_eq!(threads.len(), 4);
    assert!(threads.iter().all(|t| *t != thread::current().id()));
}

// ---- keychain error mapping ----------------------------------------------------------------

#[test]
fn connection_failure_means_no_keyring_service() {
    // What the D-Bus store returns when no Secret Service runs (pinned down by
    // `no_secret_service_is_reported_as_no_service` below).
    let err = connect_error(Error::PlatformFailure(
        "org.freedesktop.DBus.Error.ServiceUnknown".into(),
    ));
    assert!(
        matches!(err, SecretError::Unavailable(NO_SERVICE)),
        "{err:?}"
    );
}

#[test]
fn no_storage_access_means_locked() {
    for err in [
        connect_error(Error::NoStorageAccess("locked".into())),
        entry_error(Error::NoStorageAccess("prompt dismissed".into())),
    ] {
        assert!(matches!(err, SecretError::Unavailable(LOCKED)), "{err:?}");
    }
}

#[test]
fn other_entry_errors_are_platform_failures() {
    for err in [
        entry_error(Error::PlatformFailure("D-Bus went away".into())),
        entry_error(Error::TooLong("password".into(), 2560)),
    ] {
        assert!(matches!(err, SecretError::Platform(_)), "{err:?}");
    }
}

// ---- the real keychain (opt-in) ------------------------------------------------------------
//
// Run with `scripts/test-keychain.sh`. It starts throwaway D-Bus sessions, so these tests never
// see the user's own keyring: one with an unlocked GNOME Keyring, one with no Secret Service.

/// A service name that cannot collide with the app's own entries.
const TEST_SERVICE: &str = "com.adriaancloet.simplecalendar.test";

#[tokio::test]
#[ignore = "needs a Secret Service; run scripts/test-keychain.sh"]
async fn real_keychain_round_trip() {
    let secrets = Secrets::new(Arc::new(KeychainStore::new(TEST_SERVICE)));
    assert_eq!(secrets.probe().await, Ok(()));

    secrets.set("keychain-test", &feed()).await.unwrap();
    assert_eq!(secrets.get("keychain-test").await.unwrap(), Some(feed()));
    secrets.delete("keychain-test").await.unwrap();
    assert_eq!(secrets.get("keychain-test").await.unwrap(), None);
    secrets.delete("keychain-test").await.unwrap();
}

#[tokio::test]
#[ignore = "needs a session without a Secret Service; run scripts/test-keychain.sh"]
async fn no_secret_service_is_reported_as_no_service() {
    let secrets = Secrets::new(Arc::new(KeychainStore::new(TEST_SERVICE)));
    assert_eq!(secrets.probe().await, Err(NO_SERVICE));
    let err = secrets.set("keychain-test", &feed()).await.unwrap_err();
    assert!(
        matches!(err, SecretError::Unavailable(NO_SERVICE)),
        "{err:?}"
    );
}
