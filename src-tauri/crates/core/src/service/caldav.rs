//! Adding a CalDAV account and changing its password (`docs/architecture.md` §8, §10).
//!
//! Both check the login against the server before anything is stored: the provider's
//! `list_calendars` runs discovery, so a wrong address or password is refused in the form
//! instead of showing up later as a failed sync.

use rusqlite::{OptionalExtension, params};

use super::{CalendarService, SOURCE_GONE, ServiceError, SourceInfo, now, source_info};
use crate::provider::caldav::server_url;
use crate::provider::{ProviderError, SourceContext};
use crate::secrets::Secret;

const HTTPS_ONLY: &str =
    "The address must start with https://. Plain http:// only works for this computer.";
const NOT_CALDAV: &str = "No CalDAV calendars were found at this address.";

impl CalendarService {
    /// Adds the CalDAV account `username` at `address`, once the server accepted the login and
    /// showed at least one calendar.
    ///
    /// `address` may be a server, a principal or a calendar; without a scheme it is `https://`.
    /// The login goes to the keychain only. The database stores `username` as the name and the
    /// server's host as the label.
    pub async fn add_caldav_source(
        &self,
        address: &str,
        username: &str,
        password: &str,
    ) -> Result<SourceInfo, ServiceError> {
        let (address, username) = (address.trim(), username.trim());
        let host = caldav_host(address)?;
        if username.is_empty() {
            return Err(ServiceError::InvalidInput("Enter your username.".into()));
        }
        check_password(password)?;
        self.secrets.require().await?;
        let (label, user) = (host.clone(), username.to_owned());
        let existing = self
            .db
            .read(move |c| {
                c.query_row(
                    "SELECT 1 FROM sources
                     WHERE kind = 'caldav' AND account_label = ?1 COLLATE NOCASE
                       AND display_name = ?2",
                    params![label, user],
                    |_| Ok(()),
                )
                .optional()
            })
            .await?;
        if existing.is_some() {
            return Err(ServiceError::InvalidInput(
                "This account is already added.".into(),
            ));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let secret = Secret::Caldav {
            url: address.to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
        };
        self.check_login(&id, username, &secret).await?;

        // The secret first: a source row without its secret could never sync.
        self.secrets.set(&id, &secret).await?;
        let row = (id.clone(), username.to_owned(), host);
        let inserted = self
            .db
            .write(move |c| {
                let (id, username, host) = row;
                c.execute(
                    "INSERT INTO sources (id, kind, display_name, account_label, created_at)
                     VALUES (?1, 'caldav', ?2, ?3, ?4)",
                    params![id, username, host, now()],
                )?;
                Ok(())
            })
            .await;
        if let Err(e) = inserted {
            if let Err(cleanup) = self.secrets.delete(&id).await {
                log::warn!("source {id}: cannot remove its secret after a failed add: {cleanup}");
            }
            return Err(e.into());
        }

        self.sync.sync_now(Some(id.clone()));
        let source = self.db.read(move |c| source_info(c, &id)).await?;
        source.ok_or(ServiceError::NotFound(SOURCE_GONE))
    }

    /// Replaces the password of the CalDAV source `source_id`, once the server accepted it, and
    /// syncs the source again. Its address and username stay.
    pub async fn update_caldav_password(
        &self,
        source_id: &str,
        password: &str,
    ) -> Result<SourceInfo, ServiceError> {
        check_password(password)?;
        let id = source_id.to_owned();
        self.db
            .read(move |c| {
                c.query_row(
                    "SELECT 1 FROM sources WHERE id = ?1 AND kind = 'caldav'",
                    [id],
                    |_| Ok(()),
                )
                .optional()
            })
            .await?
            .ok_or(ServiceError::NotFound(SOURCE_GONE))?;
        let Some(Secret::Caldav { url, username, .. }) = self.secrets.get(source_id).await? else {
            return Err(ServiceError::InvalidInput(
                "The login of this account is missing from the keychain. Remove the account and \
                 add it again."
                    .into(),
            ));
        };
        let secret = Secret::Caldav {
            url,
            username: username.clone(),
            password: password.to_owned(),
        };
        self.check_login(source_id, &username, &secret).await?;

        self.secrets.set(source_id, &secret).await?;
        let id = source_id.to_owned();
        self.db
            .write(move |c| {
                c.execute(
                    "UPDATE sources SET status = 'ok', last_error = NULL WHERE id = ?1",
                    [id],
                )
            })
            .await?;
        self.sync.sync_now(Some(source_id.to_owned()));
        let id = source_id.to_owned();
        let source = self.db.read(move |c| source_info(c, &id)).await?;
        source.ok_or(ServiceError::NotFound(SOURCE_GONE))
    }

    /// Asks the server for the calendars of `secret`'s login, as the source `source_id` would.
    /// A login that fails, or that shows no calendar, is refused with a message for the form.
    async fn check_login(
        &self,
        source_id: &str,
        name: &str,
        secret: &Secret,
    ) -> Result<(), ServiceError> {
        let cx = SourceContext {
            source_id: source_id.to_owned(),
            display_name: name.to_owned(),
            secret: Some(secret.clone()),
        };
        match self.caldav.list_calendars(&cx).await {
            Ok(calendars) if calendars.is_empty() => Err(ServiceError::InvalidInput(
                "This account has no calendars yet. Create one on the server, then add the \
                 account."
                    .into(),
            )),
            Ok(_) => Ok(()),
            Err(e) => Err(ServiceError::InvalidInput(check_message(e))),
        }
    }
}

/// The label of a CalDAV address: its host, with the port if it is not the default.
fn caldav_host(address: &str) -> Result<String, ServiceError> {
    let url = server_url(address).map_err(|_| {
        // Rust note: `split_once` gives the scheme of an address that has one.
        let scheme = address.split_once("://").map(|(s, _)| s);
        ServiceError::InvalidInput(match scheme {
            Some(s) if !s.eq_ignore_ascii_case("https") && !s.is_empty() => HTTPS_ONLY.into(),
            _ => "Enter the server address, such as cloud.example.com.".into(),
        })
    })?;
    let host = url.host_str().filter(|h| !h.is_empty()).ok_or_else(|| {
        ServiceError::InvalidInput("Enter the server address, such as cloud.example.com.".into())
    })?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

fn check_password(password: &str) -> Result<(), ServiceError> {
    if password.trim().is_empty() {
        return Err(ServiceError::InvalidInput("Enter the app password.".into()));
    }
    Ok(())
}

/// What the form says when the server did not accept a login.
fn check_message(e: ProviderError) -> String {
    match e {
        ProviderError::Auth => "The server refused this username or app password.".into(),
        ProviderError::Offline => {
            "SimpleCalendar can’t reach this server. Check the address and your internet \
             connection."
                .into()
        }
        ProviderError::Transient { .. } => {
            "The server is not answering properly. Try again in a moment.".into()
        }
        ProviderError::NotFound => NOT_CALDAV.into(),
        other => sentence(&other.to_string()),
    }
}

/// `message` with a capital letter and a full stop, as the provider's messages are written in
/// lower case for the sync log.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    let Some(first) = chars.next() else {
        return NOT_CALDAV.into();
    };
    let mut sentence: String = first.to_uppercase().chain(chars).collect();
    if !sentence.ends_with('.') {
        sentence.push('.');
    }
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_messages_become_sentences() {
        assert_eq!(
            sentence("the server answered HTTP 418"),
            "The server answered HTTP 418."
        );
        assert_eq!(sentence("done."), "Done.");
        assert_eq!(sentence(""), NOT_CALDAV);
    }
}
