use std::time::Duration;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::model::Access;
use crate::provider::Next;
use crate::secrets::Secret;

const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
                    BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260601T090000Z\r\nSUMMARY:A\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:b\r\nDTSTART:20260602T090000Z\r\nSUMMARY:B\r\nEND:VEVENT\r\n\
                    END:VCALENDAR\r\n";

// ---- helpers -------------------------------------------------------------------------------

fn provider() -> IcsProvider {
    IcsProvider::new().unwrap()
}

fn cx(url: Option<String>) -> SourceContext {
    SourceContext {
        source_id: "src-1".into(),
        display_name: "Holidays".into(),
        secret: url.map(|url| Secret::IcsUrl { url }),
    }
}

fn calendar() -> CalendarRef {
    CalendarRef {
        remote_id: CALENDAR_ID.into(),
        time_zone: None,
    }
}

async fn server() -> MockServer {
    MockServer::start().await
}

fn feed_at(server: &MockServer) -> SourceContext {
    cx(Some(format!("{}/feed.ics", server.uri())))
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/feed.ics"))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn pull(cx: &SourceContext, cursor: Option<&SyncCursor>) -> Result<ChangeSet, ProviderError> {
    provider().pull(cx, &calendar(), cursor, None).await
}

fn cursor_of(changes: &ChangeSet) -> SyncCursor {
    match &changes.next {
        Next::Done(cursor) => cursor.clone(),
        Next::Page(_) => panic!("a feed has one page"),
    }
}

fn decode(cursor: &SyncCursor) -> Cursor {
    serde_json::from_str(&cursor.0).expect("cursor is JSON")
}

async fn status_error(status: u16) -> ProviderError {
    let server = server().await;
    serve(&server, ResponseTemplate::new(status)).await;
    pull(&feed_at(&server), None).await.unwrap_err()
}

// ---- urls ----------------------------------------------------------------------------------

#[test]
fn webcal_is_rewritten_to_https() {
    assert_eq!(
        feed_url("webcal://example.com/cal.ics"),
        "https://example.com/cal.ics"
    );
    assert_eq!(
        feed_url("webcals://example.com/cal.ics"),
        "https://example.com/cal.ics"
    );
    assert_eq!(
        feed_url("WEBCAL://example.com/cal.ics"),
        "https://example.com/cal.ics"
    );
    assert_eq!(
        feed_url("https://example.com/cal.ics"),
        "https://example.com/cal.ics"
    );
    assert_eq!(
        feed_url("http://example.com/cal.ics"),
        "http://example.com/cal.ics"
    );
}

// ---- calendars -----------------------------------------------------------------------------

#[tokio::test]
async fn a_feed_is_one_read_only_calendar_named_after_the_source() {
    let calendars = provider()
        .list_calendars(&cx(Some("https://example.com/cal.ics".into())))
        .await
        .unwrap();
    assert_eq!(
        calendars,
        vec![RemoteCalendar {
            remote_id: CALENDAR_ID.into(),
            name: "Holidays".into(),
            color: None,
            time_zone: None,
            access: Access::Read,
        }]
    );
}

#[test]
fn capabilities_match_as_writable() {
    let provider = provider();
    let provider: &dyn CalendarProvider = &provider;
    assert_eq!(
        provider.capabilities().writable,
        provider.as_writable().is_some()
    );
    assert!(!provider.capabilities().writable);
}

// ---- fetching ------------------------------------------------------------------------------

#[tokio::test]
async fn a_changed_feed_is_one_full_change_set() {
    let server = server().await;
    serve(
        &server,
        ResponseTemplate::new(200)
            .insert_header("ETag", "\"v1\"")
            .insert_header("Last-Modified", "Mon, 01 Jun 2026 09:00:00 GMT")
            .set_body_string(FEED),
    )
    .await;

    let changes = pull(&feed_at(&server), None).await.unwrap();

    assert!(changes.full);
    assert!(changes.deletions.is_empty());
    let ids: Vec<_> = changes
        .upserts
        .iter()
        .map(|e| e.remote_id.as_str())
        .collect();
    assert_eq!(ids, vec!["a", "b"]);
    let cursor = decode(&cursor_of(&changes));
    assert_eq!(cursor.etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        cursor.last_modified.as_deref(),
        Some("Mon, 01 Jun 2026 09:00:00 GMT")
    );
    assert_eq!(cursor.hash.len(), 64, "SHA-256 in hex");
}

#[tokio::test]
async fn the_cursor_sends_conditional_headers_and_304_means_no_change() {
    let server = server().await;
    let cursor = SyncCursor(
        serde_json::to_string(&Cursor {
            etag: Some("\"v1\"".into()),
            last_modified: Some("Mon, 01 Jun 2026 09:00:00 GMT".into()),
            hash: "00".repeat(32),
        })
        .unwrap(),
    );
    Mock::given(method("GET"))
        .and(path("/feed.ics"))
        .and(header("If-None-Match", "\"v1\""))
        .and(header("If-Modified-Since", "Mon, 01 Jun 2026 09:00:00 GMT"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;

    let changes = pull(&feed_at(&server), Some(&cursor)).await.unwrap();

    assert!(!changes.full);
    assert!(changes.upserts.is_empty() && changes.deletions.is_empty());
    assert_eq!(cursor_of(&changes), cursor);
}

#[tokio::test]
async fn an_unchanged_body_means_no_change() {
    let server = server().await;
    serve(&server, ResponseTemplate::new(200).set_body_string(FEED)).await;
    let first = pull(&feed_at(&server), None).await.unwrap();
    let cursor = cursor_of(&first);

    let second = pull(&feed_at(&server), Some(&cursor)).await.unwrap();

    assert!(!second.full);
    assert!(second.upserts.is_empty() && second.deletions.is_empty());
    assert_eq!(decode(&cursor_of(&second)).hash, decode(&cursor).hash);
}

#[tokio::test]
async fn a_changed_body_with_a_cursor_is_parsed_again() {
    let server = server().await;
    serve(&server, ResponseTemplate::new(200).set_body_string(FEED)).await;
    let old = SyncCursor(
        serde_json::to_string(&Cursor {
            hash: "00".repeat(32),
            ..Cursor::default()
        })
        .unwrap(),
    );

    let changes = pull(&feed_at(&server), Some(&old)).await.unwrap();

    assert!(changes.full);
    assert_eq!(changes.upserts.len(), 2);
}

#[tokio::test]
async fn a_cursor_that_is_not_ours_is_ignored() {
    let server = server().await;
    serve(&server, ResponseTemplate::new(200).set_body_string(FEED)).await;

    let changes = pull(&feed_at(&server), Some(&SyncCursor("garbage".into())))
        .await
        .unwrap();

    assert!(changes.full);
    let request = &server.received_requests().await.unwrap()[0];
    assert!(!request.headers.contains_key("If-None-Match"));
}

#[tokio::test]
async fn webcal_urls_are_fetched_over_https() {
    // The mock server speaks plain HTTP, so an `https://` request to it fails to connect:
    // proof that the rewritten URL was used, and that such a failure is transient.
    let server = server().await;
    serve(&server, ResponseTemplate::new(200).set_body_string(FEED)).await;
    let url = format!("{}/feed.ics", server.uri()).replace("http://", "webcal://");

    let error = pull(&cx(Some(url)), None).await.unwrap_err();

    assert!(
        matches!(error, ProviderError::Transient { .. }),
        "{error:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---- errors --------------------------------------------------------------------------------

#[tokio::test]
async fn rejected_credentials_are_auth_errors() {
    assert_eq!(status_error(401).await, ProviderError::Auth);
    assert_eq!(status_error(403).await, ProviderError::Auth);
}

#[tokio::test]
async fn server_trouble_is_transient() {
    for status in [500, 502, 503, 504, 408] {
        let error = status_error(status).await;
        assert!(
            matches!(error, ProviderError::Transient { .. }),
            "{status}: {error:?}"
        );
    }
}

#[tokio::test]
async fn rate_limits_honour_retry_after() {
    let server = server().await;
    serve(
        &server,
        ResponseTemplate::new(429).insert_header("Retry-After", "120"),
    )
    .await;

    let error = pull(&feed_at(&server), None).await.unwrap_err();

    assert_eq!(
        error,
        ProviderError::Transient {
            retry_after: Some(Duration::from_secs(120))
        }
    );
}

#[tokio::test]
async fn other_client_errors_are_permanent() {
    for status in [400, 404, 410] {
        let error = status_error(status).await;
        assert!(
            matches!(error, ProviderError::Permanent(_)),
            "{status}: {error:?}"
        );
    }
}

#[tokio::test]
async fn an_unreachable_server_is_transient() {
    // Bind a port, then free it: nothing listens there any more.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let error = pull(&cx(Some(format!("http://127.0.0.1:{port}/feed.ics"))), None)
        .await
        .unwrap_err();

    assert!(
        matches!(error, ProviderError::Transient { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_body_that_is_not_a_feed_is_permanent() {
    let server = server().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_string("<!doctype html><title>Sign in</title>"),
    )
    .await;

    let error = pull(&feed_at(&server), None).await.unwrap_err();

    assert!(matches!(error, ProviderError::Permanent(_)), "{error:?}");
}

#[tokio::test]
async fn a_body_over_the_limit_is_permanent() {
    let server = server().await;
    serve(&server, ResponseTemplate::new(200).set_body_string(FEED)).await;
    let provider = IcsProvider {
        max_bytes: 100,
        ..provider()
    };

    let error = provider
        .pull(&feed_at(&server), &calendar(), None, None)
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Permanent(_)), "{error:?}");
}

#[tokio::test]
async fn a_source_without_url_is_permanent() {
    let error = pull(&cx(None), None).await.unwrap_err();
    assert!(matches!(error, ProviderError::Permanent(_)), "{error:?}");
}
