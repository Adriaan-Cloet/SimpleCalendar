mod fake;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::{OptionalExtension, params};
use tempfile::TempDir;
use tokio::sync::mpsc;

use super::*;
use crate::db::occurrences::list_occurrences;
use crate::model::{Access, EventStatus};
use crate::provider::{
    CalendarProvider, ChangeSet, Next, PageToken, ProviderError, RemoteEvent, SeriesRole,
    SyncCursor,
};
use crate::secrets::{MemoryStore, Secret, UnavailableReason};
use fake::{Fake, Pull, calendar};

const HOUR: i64 = 3600;
const MINUTE: Duration = Duration::from_secs(60);
/// How long a real-time test waits for the engine before it fails.
const PATIENCE: Duration = Duration::from_secs(10);
const DAY: Duration = Duration::from_secs(24 * 3600);

// ---- harness -------------------------------------------------------------------------------

/// A running engine on a database file, with one ICS source `src-1` served by the fake.
struct Harness {
    db: Db,
    fake: Arc<Fake>,
    store: Arc<MemoryStore>,
    handle: SyncHandle,
    events: mpsc::UnboundedReceiver<CoreEvent>,
    pulls: mpsc::UnboundedReceiver<Pull>,
    /// Every `CalendarChanged` seen so far, in order.
    changed: Vec<Vec<i64>>,
    /// Every `SyncStatus` seen so far, in order.
    statuses: Vec<SyncStatus>,
    _dir: TempDir,
}

impl Harness {
    async fn new() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).unwrap();
        let (fake, pulls) = Fake::new();
        let fake = Arc::new(fake);
        fake.watch(db.clone());
        let store = Arc::new(MemoryStore::new());
        let secrets = Secrets::new(store.clone());
        let providers = Providers::new().with(fake.clone());
        let (tx, events) = mpsc::unbounded_channel();
        let (engine, handle) = SyncEngine::new(db.clone(), secrets, providers, tx);
        tokio::spawn(engine.run());
        let h = Harness {
            db,
            fake,
            store,
            handle,
            events,
            pulls,
            changed: Vec::new(),
            statuses: Vec::new(),
            _dir: dir,
        };
        h.add_source("src-1", "ics", None).await;
        h
    }

    async fn add_source(&self, id: &str, kind: &str, interval_s: Option<i64>) {
        let (id, kind) = (id.to_owned(), kind.to_owned());
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO sources (id, kind, display_name, sync_interval_s, created_at)
                     VALUES (?1, ?2, ?1, ?3, 0)",
                    params![id, kind, interval_s],
                )
            })
            .await
            .unwrap();
    }

    async fn set_source_status(&self, id: &str, status: &str) {
        let (id, status) = (id.to_owned(), status.to_owned());
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE sources SET status = ?2 WHERE id = ?1",
                    params![id, status],
                )
            })
            .await
            .unwrap();
    }

    /// Syncs `src-1` on request and waits for the cycle to end.
    async fn sync(&mut self) -> SyncStatus {
        self.handle.sync_now(Some("src-1".into()));
        self.finished("src-1").await
    }

    /// Waits for the end of the next cycle of `source_id`, at most `PATIENCE`.
    async fn finished(&mut self, source_id: &str) -> SyncStatus {
        tokio::time::timeout(PATIENCE, self.finished_eventually(source_id))
            .await
            .expect("sync cycle did not finish")
    }

    /// Waits for the end of the next cycle of `source_id`, up to a day: for tests with a paused
    /// clock, where a day passes in an instant once nothing else can run.
    async fn finished_eventually(&mut self, source_id: &str) -> SyncStatus {
        tokio::time::timeout(DAY, self.finished_unbounded(source_id))
            .await
            .expect("sync cycle did not finish")
    }

    async fn finished_unbounded(&mut self, source_id: &str) -> SyncStatus {
        loop {
            match self.events.recv().await.expect("engine stopped") {
                CoreEvent::CalendarChanged { calendar_ids } => self.changed.push(calendar_ids),
                CoreEvent::SyncStatus(status) => {
                    self.statuses.push(status.clone());
                    if status.source_id == source_id && status.state != SyncState::Syncing {
                        return status;
                    }
                }
            }
        }
    }

    /// The next pull within a day, for tests with a paused clock.
    async fn next_pull_eventually(&mut self) -> Pull {
        tokio::time::timeout(DAY, self.pulls.recv())
            .await
            .expect("no pull")
            .expect("fake dropped")
    }

    async fn next_pull(&mut self) -> Pull {
        tokio::time::timeout(PATIENCE, self.pulls.recv())
            .await
            .expect("no pull")
            .expect("fake dropped")
    }

    fn pulls_so_far(&mut self) -> Vec<Pull> {
        let mut found = Vec::new();
        while let Ok(pull) = self.pulls.try_recv() {
            found.push(pull);
        }
        found
    }

    /// `(remote_id, master's remote_id if linked)` of every event, sorted.
    async fn events(&self) -> Vec<(String, Option<String>)> {
        self.db
            .read(|c| {
                let mut stmt = c.prepare(
                    "SELECT e.remote_id, m.remote_id FROM events e
                     LEFT JOIN events m ON m.id = e.recurring_event_id
                     ORDER BY e.remote_id",
                )?;
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect()
            })
            .await
            .unwrap()
    }

    async fn event_id(&self, remote_id: &str) -> Option<i64> {
        let remote_id = remote_id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT id FROM events WHERE remote_id = ?1",
                    [remote_id],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .unwrap()
    }

    async fn title(&self, remote_id: &str) -> String {
        let remote_id = remote_id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT title FROM events WHERE remote_id = ?1",
                    [remote_id],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    async fn cursor(&self, calendar: &str) -> Option<String> {
        let calendar = calendar.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT sync_cursor FROM calendars WHERE remote_id = ?1",
                    [calendar],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    async fn calendar_id(&self, remote_id: &str) -> Option<i64> {
        let remote_id = remote_id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT id FROM calendars WHERE remote_id = ?1",
                    [remote_id],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .unwrap()
    }

    /// `(name, access)` of every calendar, sorted by remote id.
    async fn calendars(&self) -> Vec<(String, String, String)> {
        self.db
            .read(|c| {
                let mut stmt =
                    c.prepare("SELECT remote_id, name, access FROM calendars ORDER BY remote_id")?;
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .await
            .unwrap()
    }

    /// `(status, last_error)` of a source.
    async fn source_status(&self, id: &str) -> (String, Option<String>) {
        let id = id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT status, last_error FROM sources WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .await
            .unwrap()
    }

    /// Checks the invariants once more, then fails on any violation found during the test.
    async fn assert_invariants(&self) {
        let mut found = self.fake.violations();
        found.extend(invariant_violations(&self.db).await);
        assert!(found.is_empty(), "invariants violated: {found:#?}");
    }
}

/// What must hold after every committed page (§7.2): an exception is linked if and only if its
/// master exists locally, to that master, and no occurrence is shown twice.
async fn invariant_violations(db: &Db) -> Vec<String> {
    db.read(|c| {
        let mut found = Vec::new();
        let mut stmt = c.prepare(
            "SELECT e.remote_id FROM events e
             WHERE e.recurring_remote_id IS NOT NULL
               AND coalesce(e.recurring_event_id, -1) <> coalesce(
                   (SELECT m.id FROM events m
                    WHERE m.calendar_id = e.calendar_id
                      AND m.remote_id = e.recurring_remote_id), -1)",
        )?;
        for remote_id in stmt.query_map([], |r| r.get::<_, String>(0))? {
            found.push(format!("{} is linked wrongly", remote_id?));
        }
        let from = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let to = Utc
            .with_ymd_and_hms(2027, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let mut seen = HashSet::new();
        for o in list_occurrences(c, from, to, chrono_tz::UTC)? {
            if !seen.insert((o.event_id, o.original_start)) {
                found.push(format!("occurrence shown twice: {o:?}"));
            }
        }
        Ok(found)
    })
    .await
    .unwrap()
}

// ---- change sets ---------------------------------------------------------------------------

/// 1 June 2026, 09:00 UTC: the start of every test series.
fn june_1() -> i64 {
    Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0)
        .unwrap()
        .timestamp()
}

fn single(remote_id: &str) -> RemoteEvent {
    RemoteEvent {
        remote_id: remote_id.to_owned(),
        ical_uid: Some(remote_id.to_owned()),
        status: EventStatus::Confirmed,
        title: remote_id.to_owned(),
        description: None,
        location: None,
        color: None,
        all_day: false,
        start_utc: june_1(),
        end_utc: june_1() + HOUR,
        start_tz: Some("UTC".to_owned()),
        end_tz: Some("UTC".to_owned()),
        series: SeriesRole::Single,
        remote_updated_at: None,
    }
}

/// Daily from 1 June 2026, 09:00 UTC.
fn master(remote_id: &str) -> RemoteEvent {
    RemoteEvent {
        series: SeriesRole::Master {
            recurrence: "RRULE:FREQ=DAILY;COUNT=10".to_owned(),
        },
        ..single(remote_id)
    }
}

/// Moves the occurrence of `master` on day `day` (0 is 1 June) one hour later.
fn exception(master: &str, day: i64) -> RemoteEvent {
    let original = june_1() + day * 24 * HOUR;
    RemoteEvent {
        remote_id: format!("{master}/{day}"),
        start_utc: original + HOUR,
        end_utc: original + 2 * HOUR,
        series: SeriesRole::Exception {
            master_remote_id: master.to_owned(),
            original_start_utc: original,
        },
        ..single(master)
    }
}

fn page(upserts: Vec<RemoteEvent>, deletions: &[&str], full: bool, next: Next) -> ChangeSet {
    ChangeSet {
        upserts,
        deletions: deletions.iter().map(|d| (*d).to_owned()).collect(),
        full,
        next,
    }
}

fn more(token: &str) -> Next {
    Next::Page(PageToken(token.to_owned()))
}

fn done(cursor: &str) -> Next {
    Next::Done(SyncCursor(cursor.to_owned()))
}

fn ids(names: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    names
        .iter()
        .map(|(id, master)| ((*id).to_owned(), master.map(str::to_owned)))
        .collect()
}

// ---- applying pages ------------------------------------------------------------------------

#[tokio::test]
async fn full_pull_stores_events_and_cursor() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![Ok(page(
            vec![single("a"), single("b")],
            &[],
            true,
            done("c1"),
        ))],
    );

    let status = h.sync().await;

    assert_eq!(status.state, SyncState::Ok);
    assert_eq!(h.events().await, ids(&[("a", None), ("b", None)]));
    assert_eq!(h.cursor("cal-1").await.as_deref(), Some("c1"));
    h.assert_invariants().await;
}

#[tokio::test]
async fn first_pull_has_no_cursor_and_the_next_uses_the_stored_one() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![Ok(page(vec![single("a")], &[], true, done("c1")))],
    );

    h.sync().await;
    h.sync().await;

    let pulls = h.pulls_so_far();
    let args: Vec<_> = pulls
        .iter()
        .map(|p| (p.cursor.clone(), p.page.clone()))
        .collect();
    assert_eq!(args, vec![(None, None), (Some("c1".to_owned()), None)]);
}

#[tokio::test]
async fn pages_are_followed_and_the_cursor_comes_with_the_last() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a")], &[], true, more("p2"))),
            Ok(page(vec![single("b")], &[], true, done("c1"))),
        ],
    );

    h.sync().await;

    let pages: Vec<_> = h.pulls_so_far().into_iter().map(|p| p.page).collect();
    assert_eq!(pages, vec![None, Some("p2".to_owned())]);
    assert_eq!(h.events().await, ids(&[("a", None), ("b", None)]));
    assert_eq!(h.cursor("cal-1").await.as_deref(), Some("c1"));
}

#[tokio::test]
async fn a_failed_page_keeps_earlier_pages_but_not_the_cursor() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a")], &[], true, more("p2"))),
            Err(ProviderError::Transient { retry_after: None }),
        ],
    );

    let status = h.sync().await;

    assert_eq!(status.state, SyncState::Error);
    // Page 1 was its own transaction. The cursor would claim more than was applied.
    assert_eq!(h.events().await, ids(&[("a", None)]));
    assert_eq!(h.cursor("cal-1").await, None);

    // The next cycle starts the pull over.
    h.pulls_so_far();
    h.sync().await;
    let restart = h.pulls_so_far();
    assert_eq!(
        (restart[0].cursor.clone(), restart[0].page.clone()),
        (None, None)
    );
}

#[tokio::test]
async fn full_pull_deletes_events_that_were_not_seen() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(
                vec![single("a"), single("b"), single("c")],
                &[],
                true,
                done("c1"),
            )),
            Ok(page(vec![single("a")], &[], true, more("p2"))),
            Ok(page(vec![single("c")], &[], true, done("c2"))),
        ],
    );

    h.sync().await;
    h.sync().await;

    assert_eq!(h.events().await, ids(&[("a", None), ("c", None)]));
    h.assert_invariants().await;
}

#[tokio::test]
async fn incremental_pull_applies_deletions_and_keeps_the_rest() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(
                vec![single("a"), single("b"), single("c")],
                &[],
                true,
                done("c1"),
            )),
            Ok(page(vec![single("d")], &["b"], false, done("c2"))),
        ],
    );

    h.sync().await;
    h.sync().await;

    assert_eq!(
        h.events().await,
        ids(&[("a", None), ("c", None), ("d", None)])
    );
    assert_eq!(h.cursor("cal-1").await.as_deref(), Some("c2"));
}

#[tokio::test]
async fn an_upsert_updates_the_existing_row() {
    let mut h = Harness::new().await;
    let renamed = RemoteEvent {
        title: "renamed".into(),
        ..single("a")
    };
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a")], &[], true, done("c1"))),
            Ok(page(vec![renamed], &[], false, done("c2"))),
        ],
    );

    h.sync().await;
    let id = h.event_id("a").await;
    h.sync().await;

    assert_eq!(h.event_id("a").await, id);
    assert_eq!(h.title("a").await, "renamed");
}

#[tokio::test]
async fn invalid_cursor_restarts_with_a_full_pull() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a"), single("b")], &[], true, done("c1"))),
            Err(ProviderError::CursorInvalid),
            Ok(page(vec![single("b")], &[], true, done("c2"))),
        ],
    );

    h.sync().await;
    h.pulls_so_far();
    let status = h.sync().await;

    assert_eq!(status.state, SyncState::Ok);
    let cursors: Vec<_> = h.pulls_so_far().into_iter().map(|p| p.cursor).collect();
    assert_eq!(cursors, vec![Some("c1".to_owned()), None]);
    assert_eq!(h.events().await, ids(&[("b", None)]));
    assert_eq!(h.cursor("cal-1").await.as_deref(), Some("c2"));
}

// ---- exceptions and masters (§7.2) ---------------------------------------------------------

#[tokio::test]
async fn exception_before_its_master_is_linked_when_the_master_arrives() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![exception("m", 2)], &[], true, more("p2"))),
            Ok(page(vec![single("a")], &[], true, more("p3"))),
            Ok(page(vec![master("m")], &[], true, done("c1"))),
        ],
    );

    h.sync().await;

    assert_eq!(
        h.events().await,
        ids(&[("a", None), ("m", None), ("m/2", Some("m"))])
    );
    h.assert_invariants().await;
}

#[tokio::test]
async fn exception_after_its_master_is_linked_at_once() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![master("m")], &[], true, more("p2"))),
            Ok(page(vec![single("a")], &[], true, more("p3"))),
            Ok(page(vec![exception("m", 2)], &[], true, done("c1"))),
        ],
    );

    h.sync().await;

    assert_eq!(
        h.events().await,
        ids(&[("a", None), ("m", None), ("m/2", Some("m"))])
    );
    h.assert_invariants().await;
}

#[tokio::test]
async fn exception_and_master_on_one_page_are_linked_in_either_order() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![Ok(page(
            vec![exception("m", 2), master("m"), exception("m", 4)],
            &[],
            true,
            done("c1"),
        ))],
    );

    h.sync().await;

    assert_eq!(
        h.events().await,
        ids(&[("m", None), ("m/2", Some("m")), ("m/4", Some("m"))])
    );
    h.assert_invariants().await;
}

#[tokio::test]
async fn full_pull_drops_exceptions_whose_master_never_arrives() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![exception("m", 2)], &[], true, more("p2"))),
            Ok(page(vec![single("a")], &[], true, done("c1"))),
        ],
    );

    h.sync().await;

    assert_eq!(h.events().await, ids(&[("a", None)]));
    h.assert_invariants().await;
}

#[tokio::test]
async fn incremental_pull_leaves_an_orphan_exception_unlinked() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a")], &[], true, done("c1"))),
            Ok(page(vec![exception("m", 2)], &[], false, done("c2"))),
        ],
    );

    h.sync().await;
    h.sync().await;

    // Kept, unlinked and therefore invisible; the next full pull removes it.
    assert_eq!(h.events().await, ids(&[("a", None), ("m/2", None)]));
    h.assert_invariants().await;
}

#[tokio::test]
async fn an_updated_master_keeps_its_row_id_and_its_exceptions() {
    let mut h = Harness::new().await;
    let updated = RemoteEvent {
        title: "updated".into(),
        ..master("m")
    };
    h.fake.script(
        "src-1",
        vec![
            Ok(page(
                vec![master("m"), exception("m", 2)],
                &[],
                true,
                done("c1"),
            )),
            Ok(page(vec![updated], &[], false, done("c2"))),
        ],
    );

    h.sync().await;
    let id = h.event_id("m").await;
    h.sync().await;

    assert_eq!(h.event_id("m").await, id);
    assert_eq!(h.title("m").await, "updated");
    assert_eq!(h.events().await, ids(&[("m", None), ("m/2", Some("m"))]));
    h.assert_invariants().await;
}

#[tokio::test]
async fn deleting_a_master_removes_its_exceptions() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(
                vec![master("m"), exception("m", 2)],
                &[],
                true,
                done("c1"),
            )),
            Ok(page(vec![], &["m"], false, done("c2"))),
        ],
    );

    h.sync().await;
    h.sync().await;

    assert_eq!(h.events().await, vec![]);
}

#[tokio::test]
async fn a_cancelled_occurrence_is_stored_as_an_exception() {
    let mut h = Harness::new().await;
    let cancelled = RemoteEvent {
        status: EventStatus::Cancelled,
        ..exception("m", 2)
    };
    h.fake.script(
        "src-1",
        vec![Ok(page(
            vec![cancelled, master("m")],
            &[],
            true,
            done("c1"),
        ))],
    );

    h.sync().await;

    assert_eq!(h.events().await, ids(&[("m", None), ("m/2", Some("m"))]));
    h.assert_invariants().await;
}

// ---- calendars -----------------------------------------------------------------------------

#[tokio::test]
async fn calendars_are_added_updated_and_removed() {
    let mut h = Harness::new().await;
    h.fake
        .set_calendars(vec![calendar("cal-1", "One"), calendar("cal-2", "Two")]);
    h.fake.script(
        "src-1",
        vec![Ok(page(vec![single("a")], &[], true, done("c1")))],
    );
    h.sync().await;
    assert_eq!(
        h.calendars().await,
        vec![
            ("cal-1".into(), "One".into(), "read".into()),
            ("cal-2".into(), "Two".into(), "read".into()),
        ]
    );
    let kept = h.calendar_id("cal-2").await;

    let mut renamed = calendar("cal-2", "Renamed");
    renamed.access = Access::Write;
    h.fake.set_calendars(vec![renamed]);
    h.sync().await;

    assert_eq!(
        h.calendars().await,
        vec![("cal-2".into(), "Renamed".into(), "write".into())]
    );
    assert_eq!(h.calendar_id("cal-2").await, kept);
    // The events of a removed calendar go with it.
    assert_eq!(h.events().await, vec![]);
}

#[tokio::test]
async fn every_calendar_is_pulled() {
    let mut h = Harness::new().await;
    h.fake
        .set_calendars(vec![calendar("cal-1", "One"), calendar("cal-2", "Two")]);

    h.sync().await;

    let mut pulled: Vec<_> = h.pulls_so_far().into_iter().map(|p| p.calendar).collect();
    pulled.sort();
    assert_eq!(pulled, vec!["cal-1", "cal-2"]);
}

// ---- events and status ---------------------------------------------------------------------

#[tokio::test]
async fn a_cycle_reports_syncing_then_ok() {
    let mut h = Harness::new().await;

    h.sync().await;

    let states: Vec<_> = h.statuses.iter().map(|s| s.state).collect();
    assert_eq!(states, vec![SyncState::Syncing, SyncState::Ok]);
    assert_eq!(h.statuses[1].last_error, None);
    assert_eq!(h.source_status("src-1").await, ("ok".into(), None));
}

#[tokio::test]
async fn calendar_changed_is_sent_after_each_page_that_changed_something() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![
            Ok(page(vec![single("a")], &[], true, more("p2"))),
            Ok(page(vec![single("b")], &[], true, done("c1"))),
        ],
    );

    h.sync().await;
    let cal = h.calendar_id("cal-1").await.unwrap();
    assert_eq!(h.changed, vec![vec![cal], vec![cal]]);

    // Nothing changed: no event, so the frontend does not re-query for nothing.
    h.changed.clear();
    h.sync().await;
    assert_eq!(h.changed, Vec::<Vec<i64>>::new());
}

#[tokio::test]
async fn calendar_changed_comes_after_the_commit() {
    let mut h = Harness::new().await;
    h.fake.script(
        "src-1",
        vec![Ok(page(vec![single("a")], &[], true, done("c1")))],
    );
    h.handle.sync_now(Some("src-1".into()));

    let event = tokio::time::timeout(PATIENCE, async {
        loop {
            if let CoreEvent::CalendarChanged { calendar_ids } = h.events.recv().await.unwrap() {
                return calendar_ids;
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(event.len(), 1);
    // The read connection already sees the change when the event arrives.
    assert_eq!(h.events().await, ids(&[("a", None)]));
}

#[tokio::test]
async fn auth_error_marks_the_source_auth_required() {
    let mut h = Harness::new().await;
    h.fake.script("src-1", vec![Err(ProviderError::Auth)]);

    let status = h.sync().await;

    assert_eq!(status.state, SyncState::AuthRequired);
    assert_eq!(h.source_status("src-1").await.0, "auth_required");
}

#[tokio::test]
async fn transient_error_marks_the_source_error_with_a_message() {
    let mut h = Harness::new().await;
    h.fake
        .fail_list(Some(ProviderError::Transient { retry_after: None }));

    let status = h.sync().await;

    assert_eq!(status.state, SyncState::Error);
    let message = status.last_error.expect("a message");
    assert_eq!(
        h.source_status("src-1").await,
        ("error".into(), Some(message))
    );

    // The next good cycle clears it.
    h.fake.fail_list(None);
    let status = h.sync().await;
    assert_eq!((status.state, status.last_error), (SyncState::Ok, None));
    assert_eq!(h.source_status("src-1").await, ("ok".into(), None));
}

#[tokio::test]
async fn the_secret_is_passed_to_the_provider() {
    let mut h = Harness::new().await;
    let secret = Secret::IcsUrl {
        url: "https://example.com/private.ics".into(),
    };
    Secrets::new(h.store.clone())
        .set("src-1", &secret)
        .await
        .unwrap();

    h.sync().await;

    assert_eq!(h.next_pull().await.secret, Some(secret));
}

#[tokio::test]
async fn without_a_keychain_the_source_reports_an_error_and_is_not_pulled() {
    let mut h = Harness::new().await;
    h.store.set_unavailable(Some(UnavailableReason::NoService));

    let status = h.sync().await;

    assert_eq!(status.state, SyncState::Error);
    assert!(status.last_error.is_some());
    assert!(h.pulls_so_far().is_empty());
}

// ---- scheduling (§7.1) ---------------------------------------------------------------------

#[tokio::test]
async fn sync_all_syncs_every_remote_source_and_skips_local_ones() {
    let mut h = Harness::new().await;
    h.add_source("src-2", "ics", None).await;
    h.add_source("local", "local", None).await;

    h.handle.sync_all();
    h.finished("src-1").await;
    h.finished("src-2").await;

    let mut sources: Vec<_> = h.pulls_so_far().into_iter().map(|p| p.source_id).collect();
    sources.sort();
    assert_eq!(sources, vec!["src-1", "src-2"]);
    assert!(h.statuses.iter().all(|s| s.source_id != "local"));
}

#[tokio::test]
async fn sync_all_skips_sources_that_need_sign_in_but_sync_now_tries() {
    let mut h = Harness::new().await;
    h.add_source("src-2", "ics", None).await;
    h.set_source_status("src-1", "auth_required").await;

    h.handle.sync_all();
    h.finished("src-2").await;
    assert!(h.statuses.iter().all(|s| s.source_id != "src-1"));
    let sources: Vec<_> = h.pulls_so_far().into_iter().map(|p| p.source_id).collect();
    assert_eq!(sources, vec!["src-2"]);

    let status = h.sync().await;
    assert_eq!(status.state, SyncState::Ok);
}

#[tokio::test]
async fn requests_during_a_cycle_run_exactly_one_more_cycle() {
    let mut h = Harness::new().await;
    h.fake.gate("src-1");

    h.handle.sync_now(Some("src-1".into()));
    h.next_pull().await;
    // The cycle is now inside `pull`. Three more requests arrive.
    for _ in 0..3 {
        h.handle.sync_now(Some("src-1".into()));
    }
    h.fake.release(1);
    h.finished("src-1").await;
    h.next_pull().await;
    h.fake.release(1);
    h.finished("src-1").await;

    // Give a third cycle, which must not exist, the chance to start.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.pulls_so_far().is_empty());
    let syncing = h
        .statuses
        .iter()
        .filter(|s| s.state == SyncState::Syncing)
        .count();
    assert_eq!(syncing, 2);
}

#[tokio::test]
async fn sources_sync_in_parallel() {
    let mut h = Harness::new().await;
    h.add_source("src-2", "ics", None).await;
    h.fake.gate("src-1");

    h.handle.sync_now(Some("src-1".into()));
    assert_eq!(h.next_pull().await.source_id, "src-1");
    // src-1 is held inside its pull; src-2 still completes.
    h.handle.sync_now(Some("src-2".into()));
    let status = h.finished("src-2").await;
    assert_eq!(status.state, SyncState::Ok);

    h.fake.release(1);
    h.finished("src-1").await;
}

#[tokio::test]
async fn shutdown_stops_the_engine() {
    let mut h = Harness::new().await;

    h.handle.shutdown();

    // The engine drops its event sender when it stops.
    let rest =
        tokio::time::timeout(PATIENCE, async { while h.events.recv().await.is_some() {} }).await;
    assert!(rest.is_ok(), "engine still running");
}

#[test]
fn default_intervals() {
    assert_eq!(default_interval(SourceKind::Ics), Some(60 * MINUTE));
    assert_eq!(default_interval(SourceKind::Google), Some(15 * MINUTE));
    assert_eq!(default_interval(SourceKind::Caldav), Some(15 * MINUTE));
    assert_eq!(default_interval(SourceKind::Local), None);
}

// The clock is paused: when every task waits, tokio jumps straight to the next timer, so an
// hour passes in an instant and the measured times are exact.

#[tokio::test(start_paused = true)]
async fn the_timer_syncs_ics_every_60_minutes() {
    let mut h = Harness::new().await;

    h.handle.sync_all();
    let first = h.next_pull_eventually().await;
    let second = h.next_pull_eventually().await;
    let third = h.next_pull_eventually().await;

    assert_eq!(second.at - first.at, 60 * MINUTE);
    assert_eq!(third.at - second.at, 60 * MINUTE);
}

#[tokio::test(start_paused = true)]
async fn the_timer_uses_the_source_interval_override() {
    let mut h = Harness::new().await;
    h.add_source("src-2", "ics", Some(300)).await;

    h.handle.sync_now(Some("src-2".into()));
    let first = h.next_pull_eventually().await;
    let second = h.next_pull_eventually().await;

    assert_eq!(second.source_id, "src-2");
    assert_eq!(second.at - first.at, 5 * MINUTE);
}

#[tokio::test(start_paused = true)]
async fn the_timer_skips_sources_that_need_sign_in() {
    let mut h = Harness::new().await;
    h.fake.script("src-1", vec![Err(ProviderError::Auth)]);
    h.add_source("src-2", "ics", Some(3 * 3600)).await;

    h.handle.sync_all();
    h.finished_eventually("src-1").await;
    h.finished_eventually("src-2").await;
    // src-2 syncs again after three hours. src-1 would have synced at one and two hours.
    h.finished_eventually("src-2").await;

    let src_1 = h
        .pulls_so_far()
        .into_iter()
        .filter(|p| p.source_id == "src-1")
        .count();
    // Its only pull was the failed one.
    assert_eq!(src_1, 1);
}

// ---- providers -----------------------------------------------------------------------------

#[test]
fn capabilities_match_as_writable() {
    let (fake, _) = Fake::new();
    let provider: &dyn CalendarProvider = &fake;
    assert_eq!(
        provider.capabilities().writable,
        provider.as_writable().is_some()
    );
}
