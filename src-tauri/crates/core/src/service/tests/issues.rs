//! The sync issues list: changes the server did not save (#21, `docs/architecture.md` §7.3).
//!
//! The engine's part is tested in `sync::tests`. Here the rows are given the outcome the engine
//! would have stored, and the service lists, dismisses and redoes them.

use chrono_tz::Europe::Brussels;

use super::events::{meeting, single};
use super::outbox::google_standup;
use super::*;

// ---- helpers -------------------------------------------------------------------------------

const CHANGED_ELSEWHERE: &str = "The event was changed elsewhere, so this change was not saved.";

impl Setup {
    /// Marks every waiting row `status` with `reason`, as the engine does after sending it.
    async fn settle(&self, status: &'static str, reason: &'static str) {
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE outbox SET status = ?1, last_error = ?2 WHERE status = 'pending'",
                    [status, reason],
                )
            })
            .await
            .unwrap();
    }

    /// Makes event `id` what the server has after someone else changed it.
    async fn changed_elsewhere(&self, id: i64, title: &'static str) {
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE events SET title = ?2, etag = '\"e2\"' WHERE id = ?1",
                    params![id, title],
                )
            })
            .await
            .unwrap();
    }

    async fn remove_event(&self, id: i64) {
        self.db
            .write(move |c| c.execute("DELETE FROM events WHERE id = ?1", [id]))
            .await
            .unwrap();
    }

    /// `(op, status)` of every outbox row, in order.
    async fn rows(&self) -> Vec<(String, String)> {
        self.db
            .read(|c| {
                c.prepare("SELECT op, status FROM outbox ORDER BY id")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect()
            })
            .await
            .unwrap()
    }

    /// `(remote_id, title, pending_delete)` of every event, by id.
    async fn event_rows(&self) -> Vec<(String, String, bool)> {
        self.db
            .read(|c| {
                c.prepare("SELECT remote_id, title, pending_delete FROM events ORDER BY id")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .await
            .unwrap()
    }

    /// A Google meeting the server has, with a change to it that met a newer version there:
    /// the user retitled it "Ours", someone else "Theirs".
    async fn conflicted_meeting(&mut self) -> (i64, i64) {
        let cal = self.google_calendar().await;
        let id = self.synced_meeting(cal).await;
        let mut ours = meeting(cal);
        ours.title = "Ours".into();
        self.service
            .update_event(single(id), Scope::All, ours, "UTC")
            .await
            .unwrap();
        self.settle("conflicted", CHANGED_ELSEWHERE).await;
        self.changed_elsewhere(id, "Theirs").await;
        self.requests.pushes();
        while self.events.try_recv().is_ok() {}
        (cal, id)
    }

    async fn issue_ids(&self) -> Vec<i64> {
        let issues = self.service.list_sync_issues().await.unwrap();
        issues.iter().map(|i| i.id).collect()
    }
}

fn rows(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(op, status)| ((*op).to_owned(), (*status).to_owned()))
        .collect()
}

// ---- listing -------------------------------------------------------------------------------

#[tokio::test]
async fn the_list_shows_conflicted_and_failed_changes_newest_first() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;
    let other = s.synced_meeting(cal).await;
    let mut ours = meeting(cal);
    ours.title = "Ours".into();
    s.service
        .update_event(single(id), Scope::All, ours, "UTC")
        .await
        .unwrap();
    s.settle("conflicted", CHANGED_ELSEWHERE).await;
    s.service
        .delete_event(single(other), Scope::All)
        .await
        .unwrap();
    s.settle("failed", "Forbidden").await;
    // The engine shows a refused delete again.
    s.db.write(move |c| {
        c.execute(
            "UPDATE events SET pending_delete = 0 WHERE id = ?1",
            [other],
        )
    })
    .await
    .unwrap();
    // A change that still waits is not an issue.
    let mut waiting = meeting(cal);
    waiting.title = "Waiting".into();
    s.service.create_event(waiting, "UTC").await.unwrap();

    let issues = s.service.list_sync_issues().await.unwrap();

    let shown: Vec<_> = issues
        .iter()
        .map(|i| (i.kind, i.change, i.title.as_str(), i.reason.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            (
                IssueKind::Failed,
                IssueChange::Delete,
                "Meeting",
                "Forbidden"
            ),
            (
                IssueKind::Conflicted,
                IssueChange::Update,
                "Ours",
                CHANGED_ELSEWHERE
            ),
        ]
    );
    assert!(issues.iter().all(|i| i.calendar_id == cal));
    assert!(issues.iter().all(|i| i.calendar_name == "Remote"));
    assert!(issues.iter().all(|i| i.can_redo));
}

#[tokio::test]
async fn an_issue_serialises_for_the_frontend() {
    let mut s = setup();
    s.conflicted_meeting().await;

    let issue = s.service.list_sync_issues().await.unwrap().pop().unwrap();

    let json = serde_json::to_value(&issue).unwrap();
    assert_eq!(json["kind"], "conflicted");
    assert_eq!(json["change"], "update");
    assert_eq!(json["title"], "Ours");
    assert_eq!(json["can_redo"], true);
}

#[tokio::test]
async fn nothing_is_left_to_redo_for_a_delete_of_an_event_that_is_gone() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;
    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();
    s.settle("conflicted", CHANGED_ELSEWHERE).await;
    s.remove_event(id).await;

    let issue = s.service.list_sync_issues().await.unwrap().pop().unwrap();

    assert!(!issue.can_redo);
}

#[tokio::test]
async fn a_change_to_a_calendar_that_became_read_only_cannot_be_redone() {
    let mut s = setup();
    let (cal, _) = s.conflicted_meeting().await;
    s.db.write(move |c| c.execute("UPDATE calendars SET access = 'read' WHERE id = ?1", [cal]))
        .await
        .unwrap();

    let issue = s.service.list_sync_issues().await.unwrap().pop().unwrap();
    assert!(!issue.can_redo);

    let refused = s.service.redo_sync_issue(issue.id).await;
    assert!(
        matches!(refused, Err(ServiceError::NotEditable)),
        "{refused:?}"
    );
    assert_eq!(s.issue_ids().await, [issue.id]);
}

// ---- dismissing ----------------------------------------------------------------------------

#[tokio::test]
async fn dismissing_forgets_the_change_and_keeps_the_servers_version() {
    let mut s = setup();
    let (_, id) = s.conflicted_meeting().await;
    let issue = s.issue_ids().await[0];

    s.service.dismiss_sync_issue(issue).await.unwrap();

    assert!(s.service.list_sync_issues().await.unwrap().is_empty());
    assert!(s.rows().await.is_empty());
    assert_eq!(
        s.service.get_event(single(id)).await.unwrap().title,
        "Theirs"
    );
    assert!(s.requests.pushes().is_empty());
}

#[tokio::test]
async fn a_change_that_still_waits_cannot_be_dismissed() {
    let s = setup();
    let cal = s.google_calendar().await;
    s.service.create_event(meeting(cal), "UTC").await.unwrap();
    let id: i64 =
        s.db.read(|c| c.query_row("SELECT id FROM outbox", [], |r| r.get(0)))
            .await
            .unwrap();

    let refused = s.service.dismiss_sync_issue(id).await;

    assert!(
        matches!(refused, Err(ServiceError::NotFound(_))),
        "{refused:?}"
    );
    assert_eq!(s.rows().await, rows(&[("create", "pending")]));
}

// ---- redoing -------------------------------------------------------------------------------

#[tokio::test]
async fn redoing_a_conflicted_edit_makes_it_again_on_the_servers_version() {
    let mut s = setup();
    let (cal, id) = s.conflicted_meeting().await;
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    assert_eq!(s.service.get_event(single(id)).await.unwrap().title, "Ours");
    // A new edit, against what the server has now and with its ETag.
    let outbox = s.outbox().await;
    assert_eq!(outbox.len(), 1);
    let (op, _, payload, base, etag) = &outbox[0];
    assert_eq!(op, "update");
    assert_eq!(payload.as_ref().unwrap().title, "Ours");
    assert_eq!(base.as_ref().unwrap().title, "Theirs");
    assert_eq!(etag.as_deref(), Some("\"e2\""));
    assert!(s.service.list_sync_issues().await.unwrap().is_empty());
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
    assert_eq!(
        s.events.try_recv().unwrap(),
        CoreEvent::CalendarChanged {
            calendar_ids: vec![cal]
        }
    );
}

#[tokio::test]
async fn redoing_a_failed_edit_applies_it_again() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;
    let mut ours = meeting(cal);
    ours.title = "Ours".into();
    s.service
        .update_event(single(id), Scope::All, ours, "UTC")
        .await
        .unwrap();
    s.settle("failed", "Rate limit").await;
    // The engine rolled it back.
    s.changed_elsewhere(id, "Meeting").await;
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    assert_eq!(s.service.get_event(single(id)).await.unwrap().title, "Ours");
    assert_eq!(s.rows().await, rows(&[("update", "pending")]));
}

#[tokio::test]
async fn redoing_an_edit_of_an_event_deleted_elsewhere_creates_it_again() {
    let mut s = setup();
    let (_, id) = s.conflicted_meeting().await;
    s.remove_event(id).await;
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    let events = s.event_rows().await;
    assert_eq!(events.len(), 1);
    let (remote_id, title, _) = &events[0];
    assert_eq!(title, "Ours");
    let outbox = s.outbox().await;
    assert_eq!(outbox.len(), 1);
    assert_eq!((outbox[0].0.as_str(), &outbox[0].1), ("create", remote_id));
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
}

#[tokio::test]
async fn redoing_a_failed_create_creates_it_under_a_new_id() {
    let s = setup();
    let cal = s.google_calendar().await;
    let created = s.service.create_event(meeting(cal), "UTC").await.unwrap();
    let (first_id, _) = s.remote_row(created.event_id).await.unwrap();
    s.settle("failed", "Bad request").await;
    s.remove_event(created.event_id).await;
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    let events = s.event_rows().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, "Meeting");
    assert_ne!(events[0].0, first_id);
    assert_eq!(s.rows().await, rows(&[("create", "pending")]));
}

#[tokio::test]
async fn redoing_a_delete_deletes_the_event_again() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;
    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();
    s.settle("conflicted", CHANGED_ELSEWHERE).await;
    // The server's version shows again.
    s.db.write(move |c| c.execute("UPDATE events SET pending_delete = 0 WHERE id = ?1", [id]))
        .await
        .unwrap();
    s.changed_elsewhere(id, "Theirs").await;
    s.requests.pushes();
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    assert_eq!(s.remote_row(id).await.map(|r| r.1), Some(true));
    let outbox = s.outbox().await;
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].0, "delete");
    assert_eq!(outbox[0].3.as_ref().unwrap().title, "Theirs");
    assert_eq!(s.requests.pushes(), vec!["google".to_owned()]);
}

#[tokio::test]
async fn redoing_a_delete_of_an_event_that_is_gone_only_clears_the_issue() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = s.synced_meeting(cal).await;
    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();
    s.settle("failed", "Forbidden").await;
    s.remove_event(id).await;
    s.requests.pushes();
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    assert!(s.rows().await.is_empty());
    assert!(s.event_rows().await.is_empty());
    assert!(s.requests.pushes().is_empty());
}

#[tokio::test]
async fn an_edit_of_an_event_deleted_here_since_is_not_redone() {
    let mut s = setup();
    let (_, id) = s.conflicted_meeting().await;
    s.service
        .delete_event(single(id), Scope::All)
        .await
        .unwrap();
    let issue = s.issue_ids().await[0];

    let refused = s.service.redo_sync_issue(issue).await;

    assert!(
        matches!(refused, Err(ServiceError::NotFound(_))),
        "{refused:?}"
    );
    assert_eq!(
        s.rows().await,
        rows(&[("update", "conflicted"), ("delete", "pending")])
    );
}

#[tokio::test]
async fn an_issue_that_is_gone_cannot_be_redone() {
    let s = setup();

    let refused = s.service.redo_sync_issue(42).await;

    assert!(
        matches!(refused, Err(ServiceError::NotFound(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn redoing_the_delete_of_an_occurrence_cancels_it_again() {
    let mut s = setup();
    let cal = s.google_calendar().await;
    let id = google_standup(&mut s, cal).await;
    let nov3 = OccurrenceRef {
        event_id: id,
        original_start: instant(Brussels, 2026, 11, 3, 9, 0),
    };
    s.service.delete_event(nov3, Scope::This).await.unwrap();
    s.settle("conflicted", CHANGED_ELSEWHERE).await;
    // The server kept the occurrence.
    s.db.write(|c| {
        c.execute(
            "UPDATE events SET status = 'confirmed' WHERE recurring_event_id IS NOT NULL",
            [],
        )
    })
    .await
    .unwrap();
    assert!(s.service.get_event(nov3).await.is_ok());
    let issue = s.issue_ids().await[0];

    s.service.redo_sync_issue(issue).await.unwrap();

    let gone = s.service.get_event(nov3).await;
    assert!(matches!(gone, Err(ServiceError::NotFound(_))), "{gone:?}");
    assert_eq!(s.rows().await, rows(&[("delete", "pending")]));
}
