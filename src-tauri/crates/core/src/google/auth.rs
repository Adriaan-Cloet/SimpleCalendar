//! Which OAuth client to use, and the access tokens of signed-in accounts (§8).
//!
//! One `GoogleAuth` is shared by the sign-in in the service and the Google provider. The
//! refresh tokens are in the keychain; the access tokens made from them only in memory here.

use std::collections::HashMap;

use rusqlite::OptionalExtension;
use tokio::sync::Mutex;

use super::oauth::{self, AccessToken, ClientConfig, Endpoints};
use crate::db::{Db, DbError};
use crate::provider::{ProviderError, SourceContext, http};
use crate::secrets::{Secret, SecretError, Secrets};

/// The settings key of the user's own client id (§8: not secret, so it is in SQLite).
pub const CLIENT_ID_SETTING: &str = "google.client_id";
/// The `app_account` name of the user's own client secret.
pub const CLIENT_SECRET_ACCOUNT: &str = "google-client-secret";

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Secrets(#[from] SecretError),
}

pub struct GoogleAuth {
    http: reqwest::Client,
    endpoints: Endpoints,
    db: Db,
    secrets: Secrets,
    /// The client compiled into this build, if any (§8).
    compiled: Option<ClientConfig>,
    /// Access tokens by source id.
    //
    // Rust note: tokio's `Mutex` can be held across an `.await`. Holding it during a refresh
    // means two pulls that need a token at the same time refresh it once, not twice.
    tokens: Mutex<HashMap<String, AccessToken>>,
}

impl GoogleAuth {
    pub fn new(
        db: Db,
        secrets: Secrets,
        compiled: Option<ClientConfig>,
        endpoints: Endpoints,
    ) -> reqwest::Result<GoogleAuth> {
        Ok(GoogleAuth {
            http: http::client()?,
            endpoints,
            db,
            secrets,
            compiled,
            tokens: Mutex::new(HashMap::new()),
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Whether a client was compiled into this build.
    pub fn has_compiled_client(&self) -> bool {
        self.compiled.is_some()
    }

    /// The user's own client id from the settings, if they set one.
    pub async fn client_override(&self) -> Result<Option<String>, DbError> {
        self.db
            .read(|c| {
                c.query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    [CLIENT_ID_SETTING],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .await
    }

    /// The client to sign in with: the user's own if they set one, else the compiled-in one,
    /// else `None`.
    pub async fn client(&self) -> Result<Option<ClientConfig>, ClientError> {
        let Some(client_id) = self.client_override().await? else {
            return Ok(self.compiled.clone());
        };
        let client_secret = match self.secrets.get_app(CLIENT_SECRET_ACCOUNT).await? {
            Some(Secret::ClientSecret { secret }) => Some(secret),
            _ => None,
        };
        Ok(Some(ClientConfig {
            client_id,
            client_secret,
        }))
    }

    /// A valid access token for the source in `cx`, refreshed with its stored refresh token if
    /// the one in memory ran out. A rejected refresh token is `ProviderError::Auth`, which marks
    /// the source `auth_required` (§7.2).
    pub async fn access_token(&self, cx: &SourceContext) -> Result<String, ProviderError> {
        let mut tokens = self.tokens.lock().await;
        if let Some(token) = tokens.get(&cx.source_id).filter(|t| t.is_fresh()) {
            return Ok(token.token.clone());
        }
        let Some(Secret::GoogleRefreshToken { refresh_token }) = &cx.secret else {
            return Err(ProviderError::Auth);
        };
        let client = self
            .client()
            .await
            .map_err(|e| ProviderError::Permanent(e.to_string()))?
            .ok_or_else(|| {
                ProviderError::Permanent("No Google client id is set up in this build.".into())
            })?;
        match oauth::refresh(&self.http, &self.endpoints, &client, refresh_token).await {
            Ok(token) => {
                let value = token.token.clone();
                tokens.insert(cx.source_id.clone(), token);
                Ok(value)
            }
            Err(e) => {
                tokens.remove(&cx.source_id);
                Err(e.into())
            }
        }
    }

    /// Keeps the access token a sign-in just received, so the first sync needs no refresh.
    pub async fn remember(&self, source_id: &str, token: AccessToken) {
        self.tokens.lock().await.insert(source_id.to_owned(), token);
    }

    /// Drops the access token of a removed or re-signed-in source.
    pub async fn forget(&self, source_id: &str) {
        self.tokens.lock().await.remove(source_id);
    }
}
