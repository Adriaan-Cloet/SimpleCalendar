//! Adding a CalDAV account and changing its password (#28, `docs/architecture.md` §8).
//!
//! Discovery itself is tested in `provider::caldav::tests`. Here a fake provider stands for the
//! server: it answers `list_calendars` as told and records the login it was given.

use async_trait::async_trait;

use super::*;
use crate::model::Access;
use crate::provider::{
    CalendarProvider, CalendarRef, Capabilities, ChangeSet, PageToken, ProviderError,
    RemoteCalendar, SourceContext, SyncCursor,
};

// ---- helpers -------------------------------------------------------------------------------

/// A CalDAV server as the service sees it: `list_calendars` gives `answer`.
pub(in crate::service) struct FakeCaldav {
    answer: Mutex<Result<Vec<RemoteCalendar>, ProviderError>>,
    /// The source id and login of every check, in order.
    seen: Mutex<Vec<(String, Option<Secret>)>>,
}

impl FakeCaldav {
    pub(in crate::service) fn new() -> FakeCaldav {
        FakeCaldav {
            answer: Mutex::new(Ok(vec![calendar("Personal")])),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn answer(&self, answer: Result<Vec<RemoteCalendar>, ProviderError>) {
        *self.answer.lock().unwrap() = answer;
    }

    fn seen(&self) -> Vec<(String, Option<Secret>)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl CalendarProvider for FakeCaldav {
    fn kind(&self) -> SourceKind {
        SourceKind::Caldav
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            writable: false,
            incremental_sync: false,
            reminders: false,
        }
    }

    async fn list_calendars(
        &self,
        cx: &SourceContext,
    ) -> Result<Vec<RemoteCalendar>, ProviderError> {
        self.seen
            .lock()
            .unwrap()
            .push((cx.source_id.clone(), cx.secret.clone()));
        self.answer.lock().unwrap().clone()
    }

    async fn pull(
        &self,
        _cx: &SourceContext,
        _calendar: &CalendarRef,
        _cursor: Option<&SyncCursor>,
        _page: Option<&PageToken>,
    ) -> Result<ChangeSet, ProviderError> {
        unreachable!("the service never pulls")
    }
}

fn calendar(name: &str) -> RemoteCalendar {
    RemoteCalendar {
        remote_id: format!("https://cloud.example.com/dav/calendars/adriaan/{name}/"),
        name: name.to_owned(),
        color: None,
        time_zone: None,
        access: Access::Write,
        is_primary: false,
        default_reminders: Vec::new(),
    }
}

fn login(url: &str, username: &str, password: &str) -> Secret {
    Secret::Caldav {
        url: url.to_owned(),
        username: username.to_owned(),
        password: password.to_owned(),
    }
}

/// The message of an `InvalidInput` error; panics on any other error.
fn refusal(err: ServiceError) -> String {
    match err {
        ServiceError::InvalidInput(message) => message,
        other => panic!("expected invalid input, got {other:?}"),
    }
}

impl Setup {
    async fn add_caldav(&self) -> SourceInfo {
        self.service
            .add_caldav_source("cloud.example.com", "adriaan", "old-password")
            .await
            .unwrap()
    }

    async fn set_status(&self, source_id: &str, status: &'static str) {
        let id = source_id.to_owned();
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE sources SET status = ?2, last_error = 'refused' WHERE id = ?1",
                    params![id, status],
                )
            })
            .await
            .unwrap();
    }
}

// ---- adding --------------------------------------------------------------------------------

#[tokio::test]
async fn the_login_is_checked_then_kept_in_the_keychain_only() {
    let mut s = setup();
    let source = s
        .service
        .add_caldav_source("cloud.example.com", "adriaan", "s3cret-app-password")
        .await
        .unwrap();

    assert_eq!(source.kind, SourceKind::Caldav);
    assert_eq!(source.display_name, "adriaan");
    assert_eq!(source.account_label.as_deref(), Some("cloud.example.com"));
    assert_eq!(source.state, SyncState::Ok);

    // The server was asked first, for the source that was then added.
    let login = login("cloud.example.com", "adriaan", "s3cret-app-password");
    assert_eq!(s.caldav.seen(), vec![(source.id.clone(), Some(login))]);
    let stored = s.store.raw(&account(&source.id)).expect("secret stored");
    assert!(stored.contains("s3cret-app-password"), "{stored}");
    for row in s.source_rows().await {
        for value in row.into_iter().flatten() {
            assert!(
                !value.contains("s3cret"),
                "the password leaked into SQLite: {value}"
            );
        }
    }
    assert_eq!(s.requests.manual(), vec![Some(source.id)]);
}

#[tokio::test]
async fn the_label_is_the_host_with_its_port() {
    let s = setup();
    for (address, label) in [
        (
            "https://Cloud.Example.com/remote.php/dav",
            "cloud.example.com",
        ),
        ("http://localhost:5232/", "localhost:5232"),
        ("dav.example.org:8443/cal", "dav.example.org:8443"),
    ] {
        let source = s
            .service
            .add_caldav_source(address, label, "pw")
            .await
            .unwrap();
        assert_eq!(source.account_label.as_deref(), Some(label), "{address}");
    }
}

#[tokio::test]
async fn a_failed_check_stores_nothing_and_says_what_went_wrong() {
    let s = setup();
    for (answer, says) in [
        (Err(ProviderError::Auth), "username or app password"),
        (Err(ProviderError::Offline), "can’t reach this server"),
        (
            Err(ProviderError::Transient { retry_after: None }),
            "Try again",
        ),
        (
            Err(ProviderError::Permanent(
                "no CalDAV calendars were found at this address".into(),
            )),
            "No CalDAV calendars were found at this address.",
        ),
        (Err(ProviderError::NotFound), "No CalDAV calendars"),
        (Ok(Vec::new()), "no calendars yet"),
    ] {
        s.caldav.answer(answer.clone());
        let err = s
            .service
            .add_caldav_source("cloud.example.com", "adriaan", "pw")
            .await
            .unwrap_err();
        let message = refusal(err);
        assert!(message.contains(says), "{answer:?}: {message}");
    }
    assert!(s.source_rows().await.is_empty());
    for (id, _) in s.caldav.seen() {
        assert_eq!(s.store.raw(&account(&id)), None);
    }
}

#[tokio::test]
async fn empty_fields_and_unsafe_addresses_are_refused_before_asking_the_server() {
    let s = setup();
    for (address, username, password, says) in [
        ("", "adriaan", "pw", "server address"),
        ("cloud.example.com", " ", "pw", "username"),
        ("cloud.example.com", "adriaan", "", "password"),
        ("http://cloud.example.com", "adriaan", "pw", "https://"),
        ("ftp://cloud.example.com", "adriaan", "pw", "https://"),
        ("https://", "adriaan", "pw", "server address"),
    ] {
        let err = s
            .service
            .add_caldav_source(address, username, password)
            .await
            .unwrap_err();
        let message = refusal(err);
        assert!(message.contains(says), "{address:?}: {message}");
    }
    assert!(s.caldav.seen().is_empty());
    assert!(s.source_rows().await.is_empty());
}

#[tokio::test]
async fn the_same_account_twice_is_refused_before_asking_the_server() {
    let s = setup();
    s.add_caldav().await;

    let err = s
        .service
        .add_caldav_source("https://CLOUD.example.com/remote.php/dav", "adriaan", "pw")
        .await
        .unwrap_err();

    assert!(refusal(err).contains("already added"));
    assert_eq!(s.caldav.seen().len(), 1);
    // Another user on the same server is another account.
    s.service
        .add_caldav_source("cloud.example.com", "someone-else", "pw")
        .await
        .unwrap();
    assert_eq!(s.source_rows().await.len(), 2);
}

// A locked keyring only shows at the first write (§8), so the server is asked first.
#[tokio::test]
async fn without_a_keychain_nothing_is_added() {
    let s = setup();
    s.store.set_unavailable(Some(UnavailableReason::Locked));

    let err = s
        .service
        .add_caldav_source("cloud.example.com", "adriaan", "pw")
        .await
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::SecretsUnavailable(_)),
        "{err:?}"
    );
    assert!(s.source_rows().await.is_empty());
}

// ---- a new password ------------------------------------------------------------------------

#[tokio::test]
async fn a_new_password_is_checked_then_replaces_the_old_one() {
    let mut s = setup();
    let source = s.add_caldav().await;
    s.set_status(&source.id, "auth_required").await;
    s.requests.manual();

    let updated = s
        .service
        .update_caldav_password(&source.id, "new-password")
        .await
        .unwrap();

    assert_eq!(updated.state, SyncState::Ok);
    assert_eq!(updated.last_error, None);
    let new = login("cloud.example.com", "adriaan", "new-password");
    assert_eq!(
        s.caldav.seen().last(),
        Some(&(source.id.clone(), Some(new)))
    );
    let stored = s.store.raw(&account(&source.id)).unwrap();
    assert!(stored.contains("new-password"), "{stored}");
    assert!(!stored.contains("old-password"), "{stored}");
    assert_eq!(s.requests.manual(), vec![Some(source.id)]);
}

#[tokio::test]
async fn a_refused_new_password_keeps_the_old_one() {
    let s = setup();
    let source = s.add_caldav().await;
    s.set_status(&source.id, "auth_required").await;
    s.caldav.answer(Err(ProviderError::Auth));

    let err = s
        .service
        .update_caldav_password(&source.id, "wrong")
        .await
        .unwrap_err();

    assert!(refusal(err).contains("username or app password"));
    let stored = s.store.raw(&account(&source.id)).unwrap();
    assert!(stored.contains("old-password"), "{stored}");
    let listed = s.service.list_sources().await.unwrap();
    assert_eq!(listed[0].state, SyncState::AuthRequired);
}

#[tokio::test]
async fn a_new_password_needs_a_caldav_source_and_a_password() {
    let s = setup();
    let source = s.add_caldav().await;
    let feed = s.service.add_ics_source(FEED, "Feed").await.unwrap();

    let err = s
        .service
        .update_caldav_password(&source.id, "  ")
        .await
        .unwrap_err();
    assert!(refusal(err).contains("password"));
    for id in [feed.id.as_str(), "gone", LOCAL_SOURCE_ID] {
        let err = s
            .service
            .update_caldav_password(id, "pw")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)), "{id}: {err:?}");
    }
    assert_eq!(s.caldav.seen().len(), 1);
}

// ---- a real server -------------------------------------------------------------------------

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

/// The account as the dialog adds it, against a real server with the real provider and sync
/// engine. Opt-in, with the same variables as `provider::caldav::live`:
///
/// ```sh
/// SIMPLECALENDAR_CALDAV_URL=... SIMPLECALENDAR_CALDAV_USER=... \
/// SIMPLECALENDAR_CALDAV_PASSWORD=... cargo test -p simplecalendar-core caldav_live -- --ignored
/// ```
///
/// The account needs at least one calendar. Nothing on the server is changed.
#[tokio::test]
#[ignore = "needs a CalDAV server: see the comment"]
async fn caldav_live_account() {
    use crate::provider::Providers;
    use crate::provider::caldav::CaldavProvider;
    use crate::sync::SyncEngine;

    let (url, user, password) = (
        env("SIMPLECALENDAR_CALDAV_URL"),
        env("SIMPLECALENDAR_CALDAV_USER"),
        env("SIMPLECALENDAR_CALDAV_PASSWORD"),
    );
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("live.db")).unwrap();
    let store = Arc::new(MemoryStore::new());
    let secrets = Secrets::new(store.clone());
    let caldav = Arc::new(CaldavProvider::new().unwrap());
    let (tx, _events) = mpsc::unbounded_channel();
    let (engine, sync) = SyncEngine::new(
        db.clone(),
        secrets.clone(),
        Providers::new().with(caldav.clone()),
        tx.clone(),
    );
    tokio::spawn(engine.run());
    let google = GoogleAuth::new(db.clone(), secrets.clone(), None, Endpoints::google()).unwrap();
    let service = CalendarService::new(
        db.clone(),
        secrets,
        sync,
        tx,
        Arc::new(google),
        caldav,
        ReminderHandle::default(),
    );

    let err = service
        .add_caldav_source(&url, &user, "not-the-password")
        .await
        .unwrap_err();
    assert!(refusal(err).contains("username or app password"));
    assert!(service.list_sources().await.unwrap().is_empty());

    let source = service
        .add_caldav_source(&url, &user, &password)
        .await
        .unwrap();
    assert_eq!(source.display_name, user);
    let stored = store.raw(&account(&source.id)).unwrap();
    assert!(stored.contains(&password));

    // The engine syncs the new account at once: its calendars appear.
    let mut calendars = Vec::new();
    for _ in 0..100 {
        calendars = service.list_calendars().await.unwrap();
        calendars.retain(|c| c.source_id == source.id);
        if !calendars.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!calendars.is_empty(), "no calendars after the first sync");
    println!(
        "calendars: {:?}",
        calendars
            .iter()
            .map(|c| (&c.name, c.editable))
            .collect::<Vec<_>>()
    );

    let err = service
        .update_caldav_password(&source.id, "still-not-it")
        .await
        .unwrap_err();
    assert!(refusal(err).contains("username or app password"));
    assert!(store.raw(&account(&source.id)).unwrap().contains(&password));
    let updated = service
        .update_caldav_password(&source.id, &password)
        .await
        .unwrap();
    assert_eq!(updated.state, SyncState::Ok);

    let err = service
        .add_caldav_source(&url, &user, &password)
        .await
        .unwrap_err();
    assert!(refusal(err).contains("already added"));
}
