use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use crate::secrets::{MemoryStore, UnavailableReason, account};
use crate::sync::Requests;

const FEED: &str = "https://calendar.example.com/private/abc123token/basic.ics";

// ---- helpers -------------------------------------------------------------------------------

struct Setup {
    service: CalendarService,
    db: Db,
    store: Arc<MemoryStore>,
    requests: Requests,
    events: mpsc::UnboundedReceiver<CoreEvent>,
    _dir: TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.db")).unwrap();
    let store = Arc::new(MemoryStore::new());
    let (sync, requests) = SyncHandle::detached();
    let (tx, events) = mpsc::unbounded_channel();
    Setup {
        service: CalendarService::new(db.clone(), Secrets::new(store.clone()), sync, tx),
        db,
        store,
        requests,
        events,
        _dir: dir,
    }
}

impl Setup {
    async fn source_rows(&self) -> Vec<Vec<Option<String>>> {
        self.db
            .read(|c| {
                let mut stmt = c.prepare(
                    "SELECT id, kind, display_name, account_label, status, last_error,
                            CAST(created_at AS TEXT), CAST(sync_interval_s AS TEXT)
                     FROM sources",
                )?;
                stmt.query_map([], |r| (0..8).map(|i| r.get(i)).collect())?
                    .collect()
            })
            .await
            .unwrap()
    }

    /// A calendar with one event and `warnings` under `source_id`. Returns the calendar id.
    async fn calendar(&self, source_id: &str, remote_id: &str, synced: i64, warnings: &str) -> i64 {
        let (source_id, remote_id, warnings) = (
            source_id.to_owned(),
            remote_id.to_owned(),
            warnings.to_owned(),
        );
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO calendars (source_id, remote_id, name, access, last_synced_at,
                                            warnings)
                     VALUES (?1, ?2, 'Feed', 'read', ?3, ?4)",
                    params![source_id, remote_id, synced, warnings],
                )?;
                let id = c.last_insert_rowid();
                c.execute(
                    "INSERT INTO events (calendar_id, remote_id, start_utc, end_utc)
                     VALUES (?1, 'e', 0, 0)",
                    [id],
                )?;
                Ok(id)
            })
            .await
            .unwrap()
    }

    async fn count(&self, table: &'static str) -> i64 {
        self.db
            .read(move |c| c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)))
            .await
            .unwrap()
    }
}

// ---- adding --------------------------------------------------------------------------------

#[tokio::test]
async fn the_url_goes_to_the_keychain_and_only_the_host_to_the_database() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();

    assert_eq!(source.kind, SourceKind::Ics);
    assert_eq!(source.display_name, "Work");
    assert_eq!(
        source.account_label.as_deref(),
        Some("calendar.example.com")
    );
    assert_eq!(source.state, SyncState::Ok);
    assert_eq!(source.last_synced_at, None);

    let stored = s.store.raw(&account(&source.id)).expect("secret stored");
    assert!(stored.contains(FEED), "{stored}");
    for row in s.source_rows().await {
        for value in row.into_iter().flatten() {
            assert!(
                !value.contains("abc123token"),
                "the URL leaked into SQLite: {value}"
            );
        }
    }
}

#[tokio::test]
async fn without_a_name_the_host_is_the_name() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "  ").await.unwrap();
    assert_eq!(source.display_name, "calendar.example.com");
}

#[tokio::test]
async fn webcal_and_http_addresses_are_accepted_as_written() {
    let s = setup();
    for url in [
        "webcal://example.com/a.ics",
        "WEBCALS://example.com/b.ics",
        "http://example.com/c",
    ] {
        let source = s.service.add_ics_source(url, "").await.unwrap();
        assert_eq!(
            source.account_label.as_deref(),
            Some("example.com"),
            "{url}"
        );
        assert!(
            s.store.raw(&account(&source.id)).unwrap().contains(url),
            "{url}"
        );
    }
}

#[tokio::test]
async fn an_address_that_is_not_a_feed_url_is_refused_and_nothing_stored() {
    let s = setup();
    for url in [
        "",
        "example.com/feed.ics",
        "ftp://example.com/feed.ics",
        "https://",
        "mailto:a@b.c",
    ] {
        let err = s.service.add_ics_source(url, "x").await.unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidInput(_)),
            "{url}: {err:?}"
        );
    }
    assert!(s.source_rows().await.is_empty());
}

#[tokio::test]
async fn without_a_keychain_nothing_is_added() {
    let s = setup();
    s.store.set_unavailable(Some(UnavailableReason::Locked));

    let err = s.service.add_ics_source(FEED, "Work").await.unwrap_err();

    assert!(
        matches!(err, ServiceError::SecretsUnavailable(u) if u.reason == UnavailableReason::Locked),
        "{err:?}"
    );
    assert!(s.source_rows().await.is_empty());
}

#[tokio::test]
async fn a_new_source_is_synced_at_once() {
    let mut s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    assert_eq!(s.requests.manual(), vec![Some(source.id)]);
}

// ---- listing -------------------------------------------------------------------------------

#[tokio::test]
async fn sources_are_listed_oldest_first_with_their_calendars_state() {
    let s = setup();
    let first = s.service.add_ics_source(FEED, "First").await.unwrap();
    let second = s.service.add_ics_source(FEED, "Second").await.unwrap();
    s.calendar(&second.id, "a", 100, r#"["b warning","a warning"]"#)
        .await;
    s.calendar(&second.id, "b", 200, r#"["a warning"]"#).await;
    let id = second.id.clone();
    s.db.write(move |c| {
        c.execute(
            "UPDATE sources SET status = 'error', last_error = 'HTTP 404',
                                    created_at = created_at + 1
                 WHERE id = ?1",
            [id],
        )?;
        c.execute(
            "INSERT INTO sources (id, kind, display_name, created_at)
                 VALUES ('local', 'local', 'On this computer', 0)",
            [],
        )
    })
    .await
    .unwrap();

    let sources = s.service.list_sources().await.unwrap();

    let names: Vec<_> = sources.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, vec!["First", "Second"]);
    assert_eq!(sources[0].state, SyncState::Ok);
    assert_eq!(sources[1].state, SyncState::Error);
    assert_eq!(sources[1].last_error.as_deref(), Some("HTTP 404"));
    assert_eq!(sources[1].last_synced_at, Some(200));
    assert_eq!(sources[1].warnings, vec!["a warning", "b warning"]);
    assert_eq!(sources[0].id, first.id);
}

// ---- removing ------------------------------------------------------------------------------

#[tokio::test]
async fn removing_a_source_removes_its_data_and_its_secret() {
    let mut s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    let a = s.calendar(&source.id, "a", 0, "[]").await;
    let b = s.calendar(&source.id, "b", 0, "[]").await;

    s.service.remove_source(&source.id).await.unwrap();

    assert!(s.source_rows().await.is_empty());
    assert_eq!(s.count("calendars").await, 0);
    assert_eq!(s.count("events").await, 0);
    assert_eq!(s.store.raw(&account(&source.id)), None);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![a, b]
        }
    );
}

#[tokio::test]
async fn removing_an_unknown_source_is_not_found() {
    let s = setup();
    let err = s.service.remove_source("nope").await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");
}

#[tokio::test]
async fn without_a_keychain_the_source_is_kept() {
    let s = setup();
    let source = s.service.add_ics_source(FEED, "Work").await.unwrap();
    s.store.set_unavailable(Some(UnavailableReason::Locked));

    let err = s.service.remove_source(&source.id).await.unwrap_err();

    assert!(
        matches!(err, ServiceError::SecretsUnavailable(_)),
        "{err:?}"
    );
    assert_eq!(s.source_rows().await.len(), 1);
}

// ---- errors for the frontend ---------------------------------------------------------------

#[test]
fn errors_serialise_with_a_kind_and_a_message() {
    let json = |e: ServiceError| serde_json::to_value(e).unwrap();
    assert_eq!(
        json(ServiceError::InvalidInput("Enter an address.".into())),
        serde_json::json!({"kind": "invalid_input", "message": "Enter an address."})
    );
    assert_eq!(json(ServiceError::NotFound)["kind"], "not_found");
    let locked = json(ServiceError::SecretsUnavailable(SecretsUnavailable {
        reason: UnavailableReason::Locked,
    }));
    assert_eq!(locked["kind"], "secrets_unavailable");
    assert_eq!(locked["reason"], "locked");
}
