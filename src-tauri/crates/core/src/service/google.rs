//! Google sign-in and the user's own OAuth client (`docs/architecture.md` §8, §10).
//
// Rust note: this is a child module of `service`, so it can add methods to `CalendarService`
// in its own `impl` block and read the struct's private fields.

use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::{CalendarService, ServiceError, SourceInfo, lock, now, source_info};
use crate::google::auth::{CLIENT_ID_SETTING, CLIENT_SECRET_ACCOUNT};
use crate::google::loopback::{Loopback, RedirectError};
use crate::google::oauth::{self, AccessToken, Endpoints, Pkce, random_token};
use crate::secrets::Secret;

/// How long the loopback listener waits for the browser (§8).
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Every Google client id ends in this.
const CLIENT_ID_SUFFIX: &str = ".apps.googleusercontent.com";

/// Which OAuth client Google sign-in uses, for the settings form. Never holds the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoogleClientInfo {
    /// The user's own client id, if they entered one. It takes precedence.
    pub custom_client_id: Option<String>,
    /// Whether this build has a client compiled in.
    pub built_in: bool,
}

impl CalendarService {
    /// Signs in to a Google account in the system browser and returns its source.
    ///
    /// `open` opens the consent page; the shell passes the opener plugin, tests a fake browser.
    /// Resolves when the browser step finishes, after 5 minutes, or when the sign-in is
    /// cancelled. With `source_id`, the user signs in again to that account, which must be the
    /// same one. Without it, signing in to an account that is already there signs that one in
    /// again instead of adding it twice.
    pub async fn begin_google_sign_in<F>(
        &self,
        source_id: Option<&str>,
        open: F,
    ) -> Result<SourceInfo, ServiceError>
    where
        F: FnOnce(&str) -> Result<(), String> + Send,
    {
        self.secrets.require().await?;
        let client = self
            .google
            .client()
            .await
            .map_err(|e| ServiceError::SignIn(e.to_string()))?
            .ok_or(ServiceError::GoogleClientMissing)?;
        let expected = match source_id {
            Some(id) => Some(self.google_account(id).await?),
            None => None,
        };

        let loopback = Loopback::bind()
            .await
            .map_err(|e| ServiceError::SignIn(format!("Cannot start the sign-in: {e}")))?;
        let redirect_uri = loopback.redirect_uri();
        let (pkce, state) = (Pkce::new(), random_token());
        let url = oauth::authorize_url(
            self.google.endpoints(),
            &client,
            &redirect_uri,
            &state,
            &pkce,
            expected.as_deref(),
        );

        // A new sign-in replaces one that is still waiting.
        let (cancel, cancelled) = oneshot::channel();
        if let Some(earlier) = lock(&self.sign_in).replace(cancel) {
            let _ = earlier.send(());
        }
        open(url.as_str())
            .map_err(|e| ServiceError::SignIn(format!("Cannot open the browser: {e}")))?;
        let code = loopback
            .receive(&state, SIGN_IN_TIMEOUT, cancelled)
            .await
            .map_err(|e| match e {
                RedirectError::Cancelled => ServiceError::Cancelled,
                other => ServiceError::SignIn(other.to_string()),
            })?;

        let http = self.google.http();
        let endpoints = self.google.endpoints();
        let tokens = oauth::exchange_code(http, endpoints, &client, &code, &pkce, &redirect_uri)
            .await
            .map_err(|e| ServiceError::SignIn(e.to_string()))?;
        let refresh_token = tokens.refresh_token.ok_or_else(|| {
            ServiceError::SignIn("Google did not allow offline access. Try again.".into())
        })?;
        let account = account_address(http, endpoints, &tokens.access).await?;
        // Rust note: `if let ... && ...` (a let chain) tests the pattern and the condition at once.
        if let Some(expected) = &expected
            && !account.eq_ignore_ascii_case(expected)
        {
            return Err(ServiceError::SignIn(format!(
                "You signed in as {account}. Sign in as {expected} to reconnect this account."
            )));
        }

        let source_id = self.store_google_account(&account, refresh_token).await?;
        self.google.remember(&source_id, tokens.access).await;
        self.sync.sync_now(Some(source_id.clone()));
        let id = source_id.clone();
        let source = self.db.read(move |c| source_info(c, &id)).await?;
        source.ok_or(ServiceError::NotFound(super::SOURCE_GONE))
    }

    /// Stops a sign-in that is waiting for the browser. Does nothing if none is.
    pub fn cancel_google_sign_in(&self) {
        if let Some(cancel) = lock(&self.sign_in).take() {
            let _ = cancel.send(());
        }
    }

    /// The OAuth client Google sign-in uses.
    pub async fn google_client(&self) -> Result<GoogleClientInfo, ServiceError> {
        Ok(GoogleClientInfo {
            custom_client_id: self.google.client_override().await?,
            built_in: self.google.has_compiled_client(),
        })
    }

    /// Sets the user's own OAuth client, which takes precedence over the built-in one (§8). An
    /// empty `client_id` goes back to the built-in client. The id goes to SQLite, the secret to
    /// the keychain.
    pub async fn set_google_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<GoogleClientInfo, ServiceError> {
        let (client_id, client_secret) = (client_id.trim(), client_secret.trim());
        if !client_id.is_empty()
            && (!client_id.ends_with(CLIENT_ID_SUFFIX) || client_id.contains(char::is_whitespace))
        {
            return Err(ServiceError::InvalidInput(format!(
                "A Google client id ends in {CLIENT_ID_SUFFIX}."
            )));
        }
        self.secrets.require().await?;
        if client_id.is_empty() || client_secret.is_empty() {
            self.secrets.delete_app(CLIENT_SECRET_ACCOUNT).await?;
        } else {
            let secret = Secret::ClientSecret {
                secret: client_secret.to_owned(),
            };
            self.secrets.set_app(CLIENT_SECRET_ACCOUNT, &secret).await?;
        }
        let id = client_id.to_owned();
        self.db
            .write(move |c| {
                if id.is_empty() {
                    c.execute("DELETE FROM settings WHERE key = ?1", [CLIENT_ID_SETTING])?;
                } else {
                    c.execute(
                        "INSERT INTO settings (key, value) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        [CLIENT_ID_SETTING, &id],
                    )?;
                }
                Ok(())
            })
            .await?;
        self.google_client().await
    }

    /// The address of the Google source `source_id`.
    async fn google_account(&self, source_id: &str) -> Result<String, ServiceError> {
        let id = source_id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT account_label FROM sources WHERE id = ?1 AND kind = 'google'",
                    [id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
            })
            .await?
            .flatten()
            .ok_or(ServiceError::NotFound(
                "This Google account is no longer in SimpleCalendar.",
            ))
    }

    /// Stores the refresh token of `account` and returns its source id: the existing source of
    /// that account, marked as signed in again, or a new one.
    async fn store_google_account(
        &self,
        account: &str,
        refresh_token: String,
    ) -> Result<String, ServiceError> {
        let label = account.to_owned();
        let existing = self
            .db
            .read(move |c| {
                c.query_row(
                    "SELECT id FROM sources
                     WHERE kind = 'google' AND account_label = ?1 COLLATE NOCASE",
                    [label],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .await?;
        let id = existing
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // The secret first, as for a feed: a source row without its secret could never sync.
        self.secrets
            .set(&id, &Secret::GoogleRefreshToken { refresh_token })
            .await?;
        self.google.forget(&id).await;
        let (row_id, label, is_new) = (id.clone(), account.to_owned(), existing.is_none());
        let stored = self
            .db
            .write(move |c| {
                if is_new {
                    c.execute(
                        "INSERT INTO sources (id, kind, display_name, account_label, created_at)
                         VALUES (?1, 'google', ?2, ?2, ?3)",
                        params![row_id, label, now()],
                    )?;
                } else {
                    c.execute(
                        "UPDATE sources SET status = 'ok', last_error = NULL WHERE id = ?1",
                        [row_id],
                    )?;
                }
                Ok(())
            })
            .await;
        if let Err(e) = stored {
            if existing.is_none()
                && let Err(cleanup) = self.secrets.delete(&id).await
            {
                log::warn!("source {id}: cannot remove its secret after a failed add: {cleanup}");
            }
            return Err(e.into());
        }
        Ok(id)
    }
}

/// The address of the signed-in account: the id of its primary calendar. This needs only the
/// calendar list scope, not `email` (§13 point 7).
async fn account_address(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    access: &AccessToken,
) -> Result<String, ServiceError> {
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }
    let failed = |detail: String| {
        ServiceError::SignIn(format!(
            "Cannot read the Google account's address ({detail})."
        ))
    };
    let url = endpoints
        .api
        .join("users/me/calendarList/primary")
        .map_err(|e| failed(e.to_string()))?;
    let response = http
        .get(url)
        .bearer_auth(&access.token)
        .send()
        .await
        .map_err(|e| failed(e.to_string()))?;
    if !response.status().is_success() {
        return Err(failed(format!("HTTP {}", response.status())));
    }
    let body = response.bytes().await.map_err(|e| failed(e.to_string()))?;
    let entry: Entry = serde_json::from_slice(&body).map_err(|e| failed(e.to_string()))?;
    Ok(entry.id)
}
