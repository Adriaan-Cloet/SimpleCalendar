//! OAuth 2.0 for an installed application, with PKCE (RFC 7636): the authorization URL, the
//! code exchange and the token refresh (`docs/architecture.md` §8 "Google sign-in").
//!
//! Written on `reqwest` and `sha2` instead of the `oauth2` crate, which would add four crates
//! the app does not build yet (§11).

use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::provider::ProviderError;

/// The narrowest scopes that list calendars and edit events (§13 point 7): the calendar list,
/// read only, and the events of every calendar the account can see. The account's address is
/// the id of its primary calendar, so no `email` or `openid` scope is needed.
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.events",
];

/// An access token is treated as expired this long before Google says it expires, so a request
/// never leaves with a token that runs out on the way.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// Where Google's endpoints are. Tests point them at a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: Url,
    pub token: Url,
    /// The Calendar API, ending in a slash.
    pub api: Url,
}

impl Endpoints {
    pub fn google() -> Endpoints {
        // Rust note: these are fixed, valid URLs, so the `expect`s can never fail.
        let parse = |url: &str| Url::parse(url).expect("valid Google endpoint");
        Endpoints {
            authorize: parse("https://accounts.google.com/o/oauth2/v2/auth"),
            token: parse("https://oauth2.googleapis.com/token"),
            api: parse("https://www.googleapis.com/calendar/v3/"),
        }
    }
}

/// The app's OAuth client in a Google Cloud project. Google treats the secret of an installed
/// application as not confidential, but it still never goes into git or a log (§8).
#[derive(Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A PKCE pair: the verifier stays here, the challenge goes to the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// A fresh pair with a random verifier.
    pub fn new() -> Pkce {
        Pkce::from_verifier(random_token())
    }

    /// The `S256` challenge of `verifier`: the URL-safe base64 of its SHA-256, without padding.
    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Pkce {
        Pkce::new()
    }
}

/// 32 random bytes as 43 URL-safe characters: a PKCE verifier or a `state` value.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    // Rust note: the OS random source only fails on platforms this app does not run on.
    getrandom::fill(&mut bytes).expect("the OS random number generator works");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The consent page the browser opens. `login_hint` preselects the account when the user signs
/// in again.
pub fn authorize_url(
    endpoints: &Endpoints,
    client: &ClientConfig,
    redirect_uri: &str,
    state: &str,
    pkce: &Pkce,
    login_hint: Option<&str>,
) -> Url {
    let mut url = endpoints.authorize.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("client_id", &client.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("state", state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            // A refresh token, and a new one on every sign-in, also when the account agreed
            // before: signing in again must replace a rejected one.
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
        if let Some(hint) = login_hint {
            query.append_pair("login_hint", hint);
        }
    }
    url
}

/// A short-lived access token. Kept in memory only (§8).
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken {
    pub token: String,
    /// Already shortened by `EXPIRY_MARGIN`.
    pub expires_at: Instant,
}

impl AccessToken {
    pub fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// What the code exchange returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: AccessToken,
    /// Google sends one because the URL asks for offline access with `prompt=consent`.
    pub refresh_token: Option<String>,
}

/// Why a token request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OAuthError {
    /// The code or the refresh token is no longer valid: revoked, expired, or replaced. The
    /// user has to sign in again.
    #[error("Google no longer accepts this sign-in. Sign in again.")]
    InvalidGrant,
    /// Google does not know the client id, or the secret does not match it.
    #[error("Google does not accept the client id or secret ({0}).")]
    Client(String),
    /// No connection to Google could be made (§7.1).
    #[error("Google could not be reached. Check your internet connection.")]
    Offline,
    /// Network trouble once connected, HTTP 5xx or a rate limit. Try again later.
    #[error("Google could not be reached. Try again later.")]
    Transient,
    #[error("{0}")]
    Other(String),
}

impl From<OAuthError> for ProviderError {
    fn from(e: OAuthError) -> ProviderError {
        match e {
            OAuthError::InvalidGrant => ProviderError::Auth,
            OAuthError::Offline => ProviderError::Offline,
            OAuthError::Transient => ProviderError::Transient { retry_after: None },
            other => ProviderError::Permanent(other.to_string()),
        }
    }
}

/// Exchanges the code from the redirect for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client: &ClientConfig,
    code: &str,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<Tokens, OAuthError> {
    let response = token_request(
        http,
        endpoints,
        client,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", redirect_uri),
        ],
    )
    .await?;
    Ok(Tokens {
        access: response.access_token(),
        refresh_token: response.refresh_token,
    })
}

/// A new access token for a stored refresh token.
pub async fn refresh(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client: &ClientConfig,
    refresh_token: &str,
) -> Result<AccessToken, OAuthError> {
    let response = token_request(
        http,
        endpoints,
        client,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    Ok(response.access_token())
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Seconds.
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

impl TokenResponse {
    fn access_token(&self) -> AccessToken {
        let lifetime = Duration::from_secs(self.expires_in.unwrap_or(3600));
        AccessToken {
            token: self.access_token.clone(),
            expires_at: Instant::now() + lifetime.saturating_sub(EXPIRY_MARGIN),
        }
    }
}

/// The error body of the token endpoint (RFC 6749 §5.2).
#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
}

/// POSTs `params` plus the client's credentials to the token endpoint.
async fn token_request(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client: &ClientConfig,
    params: &[(&str, &str)],
) -> Result<TokenResponse, OAuthError> {
    let mut pairs = params.to_vec();
    pairs.push(("client_id", &client.client_id));
    if let Some(secret) = &client.client_secret {
        pairs.push(("client_secret", secret));
    }
    let response = http
        .post(endpoints.token.clone())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&pairs))
        .send()
        .await
        .map_err(|e| {
            log::warn!("token request failed: {e}");
            match ProviderError::unanswered(&e) {
                ProviderError::Offline => OAuthError::Offline,
                _ => OAuthError::Transient,
            }
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|_| OAuthError::Transient)?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|_| {
            OAuthError::Other("Google sent a token response that cannot be read.".into())
        });
    }
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(OAuthError::Transient);
    }
    let error = serde_json::from_slice::<TokenErrorBody>(&body)
        .map(|b| b.error)
        .unwrap_or_else(|_| format!("HTTP {status}"));
    Err(match error.as_str() {
        "invalid_grant" => OAuthError::InvalidGrant,
        "invalid_client" | "unauthorized_client" => OAuthError::Client(error),
        _ => OAuthError::Other(format!("Google refused the request ({error}).")),
    })
}

/// `application/x-www-form-urlencoded` for `pairs`.
//
// Rust note: `Url` already has a form encoder for its query string, so this borrows it instead
// of adding the `form_urlencoded` crate as a direct dependency.
fn form(pairs: &[(&str, &str)]) -> String {
    let mut url = Url::parse("http://form.invalid/").expect("valid URL");
    url.query_pairs_mut().extend_pairs(pairs);
    url.query().unwrap_or_default().to_owned()
}
