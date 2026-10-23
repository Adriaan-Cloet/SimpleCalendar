use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::auth::{CLIENT_ID_SETTING, CLIENT_SECRET_ACCOUNT};
use super::loopback::{Loopback, RedirectError};
use super::oauth::{self, ClientConfig, Endpoints, OAuthError, Pkce, SCOPES};
use super::*;
use crate::db::Db;
use crate::provider::{ProviderError, SourceContext};
use crate::secrets::{MemoryStore, Secret, Secrets};

/// The app's client: `reqwest` is built without a crypto provider of its own.
fn http() -> reqwest::Client {
    crate::provider::http::client().unwrap()
}

fn client() -> ClientConfig {
    ClientConfig {
        client_id: "client-1.apps.googleusercontent.com".into(),
        client_secret: Some("shh".into()),
    }
}

fn endpoints(server: &MockServer) -> Endpoints {
    let base = Url::parse(&server.uri()).unwrap();
    Endpoints {
        authorize: base.join("auth").unwrap(),
        token: base.join("token").unwrap(),
        api: base.join("calendar/v3/").unwrap(),
    }
}

fn token_ok(refresh: Option<&str>, expires_in: u64) -> ResponseTemplate {
    let mut body = serde_json::json!({
        "access_token": "ya29.access",
        "expires_in": expires_in,
        "token_type": "Bearer",
        "scope": SCOPES.join(" ")
    });
    if let Some(refresh) = refresh {
        body["refresh_token"] = refresh.into();
    }
    ResponseTemplate::new(200).set_body_json(body)
}

fn token_error(status: u16, error: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(serde_json::json!({ "error": error }))
}

// ---- PKCE and the authorization URL --------------------------------------------------------

#[test]
fn the_pkce_challenge_matches_rfc_7636_appendix_b() {
    let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
    assert_eq!(
        pkce.challenge,
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn a_new_verifier_is_43_url_safe_characters_and_never_repeats() {
    let (a, b) = (Pkce::new(), Pkce::new());
    assert_eq!(a.verifier.len(), 43);
    assert!(
        a.verifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "{}",
        a.verifier
    );
    assert_ne!(a.verifier, b.verifier);
}

#[test]
fn the_authorize_url_asks_for_offline_access_with_s256() {
    let pkce = Pkce::new();
    let url = oauth::authorize_url(
        &Endpoints::google(),
        &client(),
        "http://127.0.0.1:4711/",
        "the-state",
        &pkce,
        Some("me@example.com"),
    );
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

    assert_eq!(url.host_str(), Some("accounts.google.com"));
    assert_eq!(query["client_id"], "client-1.apps.googleusercontent.com");
    assert_eq!(query["redirect_uri"], "http://127.0.0.1:4711/");
    assert_eq!(query["response_type"], "code");
    assert_eq!(
        query["scope"],
        "https://www.googleapis.com/auth/calendar.calendarlist.readonly \
         https://www.googleapis.com/auth/calendar.events"
    );
    assert_eq!(query["state"], "the-state");
    assert_eq!(query["code_challenge"], pkce.challenge);
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["access_type"], "offline");
    assert_eq!(query["prompt"], "consent");
    assert_eq!(query["login_hint"], "me@example.com");
    assert!(
        !url.as_str().contains("shh"),
        "the secret is not in the URL"
    );
}

// ---- token endpoint ------------------------------------------------------------------------

#[tokio::test]
async fn the_code_is_exchanged_with_its_verifier_and_the_client_credentials() {
    let server = MockServer::start().await;
    let pkce = Pkce::from_verifier("verifier-123".into());
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=4%2Fthe-code"))
        .and(body_string_contains("code_verifier=verifier-123"))
        .and(body_string_contains(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A4711%2F",
        ))
        .and(body_string_contains(
            "client_id=client-1.apps.googleusercontent.com",
        ))
        .and(body_string_contains("client_secret=shh"))
        .respond_with(token_ok(Some("1//refresh"), 3599))
        .expect(1)
        .mount(&server)
        .await;

    let tokens = oauth::exchange_code(
        &http(),
        &endpoints(&server),
        &client(),
        "4/the-code",
        &pkce,
        "http://127.0.0.1:4711/",
    )
    .await
    .unwrap();

    assert_eq!(tokens.access.token, "ya29.access");
    assert!(tokens.access.is_fresh());
    assert_eq!(tokens.refresh_token.as_deref(), Some("1//refresh"));
}

#[tokio::test]
async fn token_errors_are_classified() {
    for (response, expected) in [
        (token_error(400, "invalid_grant"), OAuthError::InvalidGrant),
        (
            token_error(401, "invalid_client"),
            OAuthError::Client("invalid_client".into()),
        ),
        (token_error(503, "backend_error"), OAuthError::Transient),
        (ResponseTemplate::new(429), OAuthError::Transient),
        (
            token_error(400, "invalid_scope"),
            OAuthError::Other("Google refused the request (invalid_scope).".into()),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(response)
            .mount(&server)
            .await;
        let err = oauth::refresh(&http(), &endpoints(&server), &client(), "1//refresh")
            .await
            .unwrap_err();
        assert_eq!(err, expected);
    }
}

#[tokio::test]
async fn a_token_request_without_a_connection_is_offline() {
    // Bind a port, then free it: nothing listens there any more.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let base = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    let endpoints = Endpoints {
        authorize: base.join("auth").unwrap(),
        token: base.join("token").unwrap(),
        api: base.join("calendar/v3/").unwrap(),
    };

    let err = oauth::refresh(&http(), &endpoints, &client(), "1//refresh")
        .await
        .unwrap_err();

    assert_eq!(err, OAuthError::Offline);
    assert_eq!(ProviderError::from(err), ProviderError::Offline);
}

#[test]
fn a_rejected_grant_means_signing_in_again() {
    assert_eq!(
        ProviderError::from(OAuthError::InvalidGrant),
        ProviderError::Auth
    );
    assert!(matches!(
        ProviderError::from(OAuthError::Transient),
        ProviderError::Transient { .. }
    ));
}

// ---- loopback ------------------------------------------------------------------------------

/// Binds a listener and waits on it in the background.
async fn listen(
    timeout: Duration,
) -> (
    String,
    tokio::task::JoinHandle<Result<String, RedirectError>>,
) {
    let loopback = Loopback::bind().await.unwrap();
    let uri = loopback.redirect_uri();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _keep = cancel;
        loopback.receive("s1", timeout, cancelled).await
    });
    (uri, task)
}

/// Binding the port again works only if nothing listens there. (Connecting instead would be
/// racy: another test's server can be given the freed port.)
fn assert_port_closed(uri: &str) {
    let port: u16 = uri
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::net::TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
}

#[tokio::test]
async fn the_redirect_with_the_right_state_gives_the_code() {
    let (uri, task) = listen(Duration::from_secs(5)).await;
    assert!(uri.starts_with("http://127.0.0.1:"), "{uri}");

    let page = http()
        .get(format!("{uri}?state=s1&code=4%2Fabc&scope=x"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(task.await.unwrap(), Ok("4/abc".to_owned()));
    assert!(page.contains("close this tab"), "{page}");
}

#[tokio::test]
async fn the_listener_closes_after_one_request() {
    let (uri, task) = listen(Duration::from_secs(5)).await;
    http()
        .get(format!("{uri}?state=s1&code=c"))
        .send()
        .await
        .unwrap();
    task.await.unwrap().unwrap();

    assert_port_closed(&uri);
}

#[tokio::test]
async fn a_wrong_state_or_a_refusal_gives_no_code() {
    for (query, expected) in [
        ("state=other&code=c", RedirectError::StateMismatch),
        ("code=c", RedirectError::StateMismatch),
        ("state=s1&error=access_denied", RedirectError::Denied),
        (
            "state=s1&error=server_error",
            RedirectError::Google("server_error".into()),
        ),
    ] {
        let (uri, task) = listen(Duration::from_secs(5)).await;
        let page = http()
            .get(format!("{uri}?{query}"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(task.await.unwrap(), Err(expected), "{query}");
        assert!(page.contains("did not complete"), "{page}");
    }
}

#[tokio::test]
async fn an_idle_connection_does_not_hold_up_the_redirect() {
    let (uri, task) = listen(Duration::from_secs(5)).await;
    let port = uri.trim_end_matches('/').rsplit(':').next().unwrap();
    // A browser's speculative preconnect: a connection that sends nothing.
    let _idle = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    http()
        .get(format!("{uri}?state=s1&code=c"))
        .send()
        .await
        .unwrap();
    assert_eq!(task.await.unwrap(), Ok("c".to_owned()));
}

#[tokio::test]
async fn cancelling_stops_the_wait_and_closes_the_port() {
    let loopback = Loopback::bind().await.unwrap();
    let uri = loopback.redirect_uri();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loopback
            .receive("s1", Duration::from_secs(5), cancelled)
            .await
    });

    cancel.send(()).unwrap();

    assert_eq!(task.await.unwrap(), Err(RedirectError::Cancelled));
    assert_port_closed(&uri);
}

#[tokio::test]
async fn without_a_redirect_it_gives_up_after_the_timeout() {
    let (_uri, task) = listen(Duration::from_millis(50)).await;
    assert_eq!(task.await.unwrap(), Err(RedirectError::TimedOut));
}

// ---- GoogleAuth ----------------------------------------------------------------------------

struct Setup {
    auth: GoogleAuth,
    db: Db,
    secrets: Secrets,
    server: MockServer,
    _dir: TempDir,
}

async fn setup(compiled: Option<ClientConfig>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.db")).unwrap();
    let secrets = Secrets::new(Arc::new(MemoryStore::new()));
    let server = MockServer::start().await;
    let auth = GoogleAuth::new(db.clone(), secrets.clone(), compiled, endpoints(&server)).unwrap();
    Setup {
        auth,
        db,
        secrets,
        server,
        _dir: dir,
    }
}

fn signed_in(source_id: &str) -> SourceContext {
    SourceContext {
        source_id: source_id.into(),
        display_name: "me@example.com".into(),
        secret: Some(Secret::GoogleRefreshToken {
            refresh_token: "1//refresh".into(),
        }),
    }
}

#[tokio::test]
async fn an_access_token_is_refreshed_once_and_then_kept_in_memory() {
    let s = setup(Some(client())).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=1%2F%2Frefresh"))
        .respond_with(token_ok(None, 3599))
        .expect(1)
        .mount(&s.server)
        .await;

    let cx = signed_in("src-1");
    assert_eq!(s.auth.access_token(&cx).await.unwrap(), "ya29.access");
    assert_eq!(s.auth.access_token(&cx).await.unwrap(), "ya29.access");
}

#[tokio::test]
async fn a_token_about_to_expire_is_refreshed_again() {
    let s = setup(Some(client())).await;
    // Shorter than the safety margin: expired as soon as it arrives.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_ok(None, 30))
        .expect(2)
        .mount(&s.server)
        .await;

    let cx = signed_in("src-1");
    s.auth.access_token(&cx).await.unwrap();
    s.auth.access_token(&cx).await.unwrap();
}

#[tokio::test]
async fn a_rejected_or_missing_refresh_token_needs_a_new_sign_in() {
    let s = setup(Some(client())).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_error(400, "invalid_grant"))
        .mount(&s.server)
        .await;

    assert_eq!(
        s.auth.access_token(&signed_in("src-1")).await,
        Err(ProviderError::Auth)
    );
    let mut without = signed_in("src-2");
    without.secret = None;
    assert_eq!(
        s.auth.access_token(&without).await,
        Err(ProviderError::Auth)
    );
}

#[tokio::test]
async fn the_users_own_client_wins_over_the_compiled_one() {
    let s = setup(Some(client())).await;
    assert_eq!(s.auth.client().await.unwrap(), Some(client()));

    s.db.write(|c| {
        c.execute(
            "INSERT INTO settings (key, value) VALUES (?1, 'mine.apps.googleusercontent.com')",
            [CLIENT_ID_SETTING],
        )
    })
    .await
    .unwrap();
    s.secrets
        .set_app(
            CLIENT_SECRET_ACCOUNT,
            &Secret::ClientSecret {
                secret: "my-secret".into(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        s.auth.client().await.unwrap(),
        Some(ClientConfig {
            client_id: "mine.apps.googleusercontent.com".into(),
            client_secret: Some("my-secret".into()),
        })
    );
}

#[tokio::test]
async fn without_any_client_there_is_none() {
    let s = setup(None).await;
    assert_eq!(s.auth.client().await.unwrap(), None);
    assert!(matches!(
        s.auth.access_token(&signed_in("src-1")).await,
        Err(ProviderError::Permanent(_))
    ));
}

#[test]
fn debug_output_never_shows_a_secret() {
    let printed = format!("{:?}", client());
    assert!(!printed.contains("shh"), "{printed}");
    let printed = format!(
        "{:?}",
        Secret::GoogleRefreshToken {
            refresh_token: "1//refresh".into()
        }
    );
    assert!(!printed.contains("1//refresh"), "{printed}");
}
