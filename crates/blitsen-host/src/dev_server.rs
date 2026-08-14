//! Proxy mode: the application is served by the user's own dev server (#67).
//!
//! ```sh
//! blitsen http://localhost:5173
//! ```
//!
//! The native window replaces the browser tab and nothing else about the inner
//! loop changes: Vite (or webpack, or anything that serves files over HTTP) goes
//! on transforming, hot-reloading and source-mapping, and Blitsen reads what it
//! serves.
//!
//! # It is a third [`AppSource`], and that is the whole design
//!
//! An application's files reach the runtime through one trait — a directory
//! being run, a section inside the executable, and now a server answering GETs.
//! Everything downstream is unchanged: the document is on the application origin
//! (`blitsen://app/…`) exactly as it is in an export, the module resolver
//! resolves against it, `fetch` reads the application through it, and the
//! renderer asks it for images and fonts. So a module's `import.meta.url` is the
//! same *kind* of thing here as everywhere else (#126), and what proxy mode adds
//! is where the bytes come from rather than a second way to address them.
//!
//! Two consequences worth stating:
//!
//! - **A query string is part of the path here.** `/src/main.jsx?t=1738` and
//!   `/src/main.jsx` are different responses from a dev server, so the resolver
//!   keeps the query and the file-backed sources drop it, which is what a file
//!   server would have done with it anyway.
//! - **Reads are synchronous.** Loading a document is not a frame, so the thread
//!   that will own the DOM waits on the request rather than turning a loop
//!   around it. They run on the same pool `fetch` and `WebSocket` already use.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blitsen_js::JsError;
use url::Url;

use crate::modules::AppSource;

/// How long a single request to the dev server may take before it is a failure.
///
/// Generous, because the first request after a cold start waits for the server
/// to transform the entrypoint's whole module graph, and short enough that a
/// server which has gone away is reported rather than hung on.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `connect` keeps retrying a server that is not answering yet.
///
/// The common case is `blitsen http://localhost:5173` in one terminal and
/// `vite` in another, started in whichever order — so a refused connection is
/// answered by waiting for it rather than by an error the user has to react to.
/// `BLITSEN_DEV_SERVER_GRACE_MS` shortens it, which is what the acceptance run
/// uses to assert the refusal without waiting out a human's patience.
fn startup_grace() -> Duration {
    std::env::var("BLITSEN_DEV_SERVER_GRACE_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map_or(Duration::from_secs(10), Duration::from_millis)
}

/// An application served over HTTP by a development server.
pub struct DevServer {
    /// Where the application is served from, with a trailing slash.
    origin: Url,
    client: reqwest::Client,
    /// Why the last read failed, for a message that names the cause rather than
    /// reporting an empty file.
    last_error: Mutex<Option<String>>,
}

impl DevServer {
    /// Connects to `origin`, waiting for it to start if it has not yet.
    ///
    /// The entrypoint is fetched here rather than at first use: a URL that is
    /// serving nothing has to be reported before a window opens, not as a blank
    /// one (#67).
    pub fn connect(origin: &str, entrypoint: &str) -> Result<Arc<Self>, JsError> {
        let parsed = Url::parse(origin)
            .map_err(|error| JsError::new(format!("{origin} is not a URL: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(JsError::new(format!(
                "{origin} is not something Blitsen can be pointed at. A directory of built \
                 output, or a dev server over http or https."
            )));
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| JsError::new(format!("could not start an HTTP client: {error}")))?;
        let server = Arc::new(Self {
            origin: parsed,
            client,
            last_error: Mutex::new(None),
        });
        server.wait_for(entrypoint)?;
        Ok(server)
    }

    /// The address the application is served from.
    pub fn origin(&self) -> &Url {
        &self.origin
    }

    /// The URL a path is served at, for messages and for the HMR channel.
    pub fn url_for(&self, path: &str) -> String {
        self.origin
            .join(path)
            .map_or_else(|_| format!("{}{path}", self.origin), |url| url.to_string())
    }

    /// Why the last read failed, if it did.
    pub fn last_error(&self) -> Option<String> {
        crate::dom_bridge::net_lock(&self.last_error).clone()
    }

    /// Waits for the server to answer for `path`, then reports what it found.
    ///
    /// A refused connection is a server that has not started yet and is retried
    /// for [`STARTUP_GRACE`]; a server that answers 404 is running and does not
    /// have that document, which no amount of waiting fixes.
    fn wait_for(&self, path: &str) -> Result<(), JsError> {
        let deadline = std::time::Instant::now() + startup_grace();
        let mut announced = false;
        loop {
            match self.request(path) {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {
                    return Err(JsError::new(format!(
                        "{} is serving, but has no {path}. Point Blitsen at the URL your dev \
                         server prints, including any base path.",
                        self.origin
                    )));
                }
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(JsError::new(format!(
                            "nothing is answering at {}: {error}. Start your dev server first — \
                             `npm run dev` — and give Blitsen the URL it prints.",
                            self.origin
                        )));
                    }
                    if !announced {
                        eprintln!(
                            "blitsen: waiting for a dev server at {} ({error})",
                            self.origin
                        );
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    /// One GET. `Ok(None)` is a server that answered without the file.
    fn request(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let url = self
            .origin
            .join(path)
            .map_err(|error| format!("{path} is not a path this server can serve: {error}"))?;
        let client = self.client.clone();
        let pool = crate::dom_bridge::net_runtime().map_err(|error| error.to_string())?;
        pool.block_on(async move {
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|error| connection_error(&error))?;
            if !response.status().is_success() {
                return Ok(None);
            }
            response
                .bytes()
                .await
                .map(|bytes| Some(bytes.to_vec()))
                .map_err(|error| error.to_string())
        })
    }
}

/// What went wrong, without the URL repeated in every layer of it.
fn connection_error(error: &reqwest::Error) -> String {
    if error.is_connect() {
        "connection refused".to_owned()
    } else if error.is_timeout() {
        "the request timed out".to_owned()
    } else {
        error.to_string()
    }
}

impl AppSource for DevServer {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.request(path) {
            Ok(bytes) => {
                if bytes.is_none() {
                    *crate::dom_bridge::net_lock(&self.last_error) =
                        Some(format!("{} answered 404", self.url_for(path)));
                }
                bytes
            }
            Err(error) => {
                // A dev server restarting mid-session is the ordinary case, not
                // an exceptional one: the read fails, the reason is on stderr
                // once, and the next read after it comes back succeeds. Nothing
                // here takes the window down.
                let message = format!("{}: {error}", self.url_for(path));
                let mut last = crate::dom_bridge::net_lock(&self.last_error);
                if last.as_deref() != Some(message.as_str()) {
                    eprintln!("blitsen: {message}");
                    *last = Some(message);
                }
                None
            }
        }
    }
}
