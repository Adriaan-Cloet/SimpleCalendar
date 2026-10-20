//! The HTTP client the providers share.

use std::time::Duration;

/// A client with the app's TLS setup, user agent and timeouts.
///
/// `reqwest` is built without a crypto provider of its own (§13 point 1), so `ring` is
/// installed as rustls' process-wide default first. Installing fails if one is already
/// installed, which is fine: then it is this one.
pub(crate) fn client() -> reqwest::Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("SimpleCalendar/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
}
