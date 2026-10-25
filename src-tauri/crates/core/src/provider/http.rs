//! The HTTP client the providers share.

use std::time::Duration;

use reqwest::header::RETRY_AFTER;

/// A client with the app's TLS setup, user agent and timeouts.
pub(crate) fn client() -> reqwest::Result<reqwest::Client> {
    builder().build()
}

/// The same client, but it hands every redirect back instead of following it. CalDAV follows
/// them itself: `reqwest` would turn a redirected `PROPFIND` into a `GET`.
pub(crate) fn client_without_redirects() -> reqwest::Result<reqwest::Client> {
    builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// `reqwest` is built without a crypto provider of its own (§13 point 1), so `ring` is
/// installed as rustls' process-wide default first. Installing fails if one is already
/// installed, which is fine: then it is this one.
fn builder() -> reqwest::ClientBuilder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("SimpleCalendar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
}

/// Why a body could not be read.
pub(crate) enum BodyError {
    /// It grew past the limit.
    TooLarge,
    Request(reqwest::Error),
}

/// Reads the body, refusing it once it grows past `max` bytes.
pub(crate) async fn read_body(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, BodyError> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Request)? {
        if body.len() + chunk.len() > max {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `Retry-After` in seconds. The HTTP-date form is rare for calendar servers and is ignored.
pub(crate) fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}
