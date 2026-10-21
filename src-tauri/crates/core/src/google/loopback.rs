//! The loopback listener that receives Google's redirect (§8 "Google sign-in").
//!
//! It listens on `127.0.0.1` on a free port, answers the first request that arrives and then
//! closes, or closes after the timeout. Connections that send nothing, such as a browser's
//! speculative preconnect, do not hold up the real request.

use std::io;
use std::time::Duration;

use reqwest::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

/// What ends the wait.
enum Arrival {
    /// A request and its target (`/?code=...`).
    Request(TcpStream, String),
    Cancelled,
}

/// The largest request head that is read. Google's redirect is well under 2 KB.
const MAX_REQUEST: usize = 8 * 1024;

/// Why no code arrived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RedirectError {
    #[error("Sign-in took too long. Try again.")]
    TimedOut,
    #[error("Sign-in was cancelled.")]
    Cancelled,
    #[error("Google sign-in was cancelled in the browser.")]
    Denied,
    /// The `state` did not match: the request did not come from this sign-in.
    #[error("The answer from the browser did not match this sign-in. Try again.")]
    StateMismatch,
    #[error("Google refused the sign-in ({0}).")]
    Google(String),
    #[error("The sign-in listener failed: {0}")]
    Io(String),
}

pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    /// Listens on `127.0.0.1` on a port the OS picks.
    pub async fn bind() -> io::Result<Loopback> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Loopback { listener, port })
    }

    /// The redirect URI to register with this sign-in.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Waits for the redirect and returns its code. Answers the browser with a page that says
    /// how it went, then stops listening. Gives up after `timeout`, or when `cancel` fires.
    //
    // Rust note: `self` is taken by value, so the listener moves into the accept task, which is
    // aborted on the way out: the port closes whatever the outcome.
    pub async fn receive(
        self,
        state: &str,
        timeout: Duration,
        cancel: oneshot::Receiver<()>,
    ) -> Result<String, RedirectError> {
        let (tx, mut rx) = mpsc::channel::<Arrival>(1);
        let listener = self.listener;
        let accepting = tx.clone();
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                // Read each connection on its own task, so an idle one blocks nothing.
                tokio::spawn(read_target(stream, accepting.clone()));
            }
        });
        let cancelled = tokio::spawn(async move {
            // A sender dropped without sending is not a cancel; the timeout still applies.
            if cancel.await.is_ok() {
                let _ = tx.send(Arrival::Cancelled).await;
            }
        });
        let arrival = tokio::time::timeout(timeout, rx.recv()).await;
        // `abort` only asks; awaiting the task makes sure the listener is dropped, and the port
        // closed, before this returns.
        accept.abort();
        cancelled.abort();
        let _ = accept.await;
        let (mut stream, target) = match arrival {
            Err(_) => return Err(RedirectError::TimedOut),
            Ok(Some(Arrival::Cancelled)) => return Err(RedirectError::Cancelled),
            Ok(Some(Arrival::Request(stream, target))) => (stream, target),
            Ok(None) => return Err(RedirectError::Io("the listener stopped".into())),
        };

        let result = parse_redirect(&target, state);
        let page = match &result {
            Ok(_) => "You are signed in. You can close this tab and go back to SimpleCalendar.",
            Err(_) => "Sign-in did not complete. Go back to SimpleCalendar to try again.",
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{}",
            html(page).len(),
            html(page)
        );
        // The code is already here; a browser that went away does not change that.
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        result
    }
}

/// Reads the request head of `stream` and sends its target (`/?code=...`) on. Sends nothing
/// for a connection that closes, stays silent, or is not an HTTP `GET`.
async fn read_target(mut stream: TcpStream, tx: mpsc::Sender<Arrival>) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
        if head.len() > MAX_REQUEST {
            return;
        }
    }
    let line = String::from_utf8_lossy(&head);
    let mut parts = line.split_whitespace();
    if let (Some("GET"), Some(target)) = (parts.next(), parts.next()) {
        let _ = tx.send(Arrival::Request(stream, target.to_owned())).await;
    }
}

/// The code from a redirect target, if its `state` matches.
fn parse_redirect(target: &str, state: &str) -> Result<String, RedirectError> {
    let url = Url::parse("http://127.0.0.1")
        .and_then(|base| base.join(target))
        .map_err(|_| RedirectError::Google("an unreadable redirect".into()))?;
    let get = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    if get("state").as_deref() != Some(state) {
        return Err(RedirectError::StateMismatch);
    }
    match (get("code"), get("error")) {
        (_, Some(error)) if error == "access_denied" => Err(RedirectError::Denied),
        (_, Some(error)) => Err(RedirectError::Google(error)),
        (Some(code), None) => Ok(code),
        (None, None) => Err(RedirectError::Google("no code".into())),
    }
}

fn html(message: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>SimpleCalendar</title>\
         <body style=\"font: 15px system-ui, sans-serif; margin: 4em auto; max-width: 32em\">\
         <p>{message}</p></body>"
    )
}
