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

use std::sync::Arc;
use std::time::Duration;

use blitsen_js::JsError;
use parking_lot::Mutex;
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
        let origin = parsed.origin();
        let origin_name = origin.ascii_serialization();
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.url().origin() == origin {
                    attempt.follow()
                } else {
                    let target = attempt.url().to_string();
                    attempt.error(format!(
                        "redirect to {target} refused: this server only requests from {origin_name}"
                    ))
                }
            }))
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
        self.resolve(path)
            .map_or_else(|_| format!("{}{path}", self.origin), |url| url.to_string())
    }

    /// Resolves a path against the origin, refusing a result that leaves it.
    ///
    /// `Url::join` follows the URL rules, under which a path is not always a
    /// path: `//evil.example/x` is protocol-relative and an absolute URL
    /// replaces the base outright, both landing on another origin. Paths here
    /// come from the application's own source, but the contract of this type
    /// is "a request to the configured server", so a join that resolves
    /// elsewhere is refused rather than requested.
    fn resolve(&self, path: &str) -> Result<Url, String> {
        let url = self
            .origin
            .join(path)
            .map_err(|error| format!("{path} is not a path this server can serve: {error}"))?;
        if url.origin() != self.origin.origin() {
            return Err(format!(
                "{path} resolves outside {}, which is the only place this server requests from",
                self.origin
            ));
        }
        Ok(url)
    }

    /// Why the last read failed, if it did.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
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
        let url = self.resolve(path)?;
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
        let mut message = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(error) = source {
            message.push_str(": ");
            message.push_str(&error.to_string());
            source = error.source();
        }
        message
    }
}

impl AppSource for DevServer {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.request(path) {
            Ok(bytes) => {
                if bytes.is_none() {
                    *self.last_error.lock() = Some(format!("{} answered 404", self.url_for(path)));
                }
                bytes
            }
            Err(error) => {
                // A dev server restarting mid-session is the ordinary case, not
                // an exceptional one: the read fails, the reason is on stderr
                // once, and the next read after it comes back succeeds. Nothing
                // here takes the window down.
                let message = format!("{}: {error}", self.url_for(path));
                let mut last = self.last_error.lock();
                if last.as_deref() != Some(message.as_str()) {
                    eprintln!("blitsen: {message}");
                    *last = Some(message);
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    use super::*;

    fn server() -> DevServer {
        DevServer {
            origin: Url::parse("http://localhost:5173/").unwrap(),
            client: reqwest::Client::new(),
            last_error: Mutex::new(None),
        }
    }

    #[test]
    fn paths_resolving_to_another_origin_are_refused() {
        let server = server();
        for path in ["/src/main.jsx", "src/main.jsx?t=1738", "./assets/logo.svg"] {
            assert!(server.resolve(path).is_ok(), "{path} is an ordinary path");
        }
        for path in [
            "//evil.example/x",
            "http://evil.example/x",
            "https://localhost:5173/x",
        ] {
            let error = server.resolve(path).unwrap_err();
            assert!(error.contains("resolves outside"), "{path}: {error}");
        }
        assert_eq!(
            server.url_for("//evil.example/x"),
            "http://localhost:5173///evil.example/x",
            "a message about a refused path still names it, on the origin"
        );
    }

    #[test]
    fn a_first_module_load_costs_one_request() {
        use crate::modules::{ModuleRegistry, url_of};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        // Exactly two connections are served: the connection probe and the
        // one load. A stray third request has nobody answering it, which the
        // final assertion would report as a failed read.
        let requests_served = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let target = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .and_then(|line| line.split_ascii_whitespace().nth(1))
                    .unwrap()
                    .to_owned();
                requests.push(target);
                let body = b"// module";
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                stream.write_all(body).unwrap();
                stream.flush().unwrap();
            }
            requests
        });

        let server = DevServer::connect(&origin, "/index.html").unwrap();
        let registry = ModuleRegistry::new(server);
        let url = registry.resolve(&url_of("index.html"), "./mod.js").unwrap();
        assert_eq!(*registry.source(&url).unwrap(), "// module");
        // Resolution's GET is the load (#360): one request for the probe, one
        // for the module, and none to ask whether the module was there first.
        assert_eq!(requests_served.join().unwrap(), ["/index.html", "/mod.js"]);
    }

    #[test]
    fn redirects_stay_on_the_configured_origin() {
        let origin_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", origin_listener.local_addr().unwrap());
        let outside_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let outside = format!("http://{}", outside_listener.local_addr().unwrap());
        let origin_server = std::thread::spawn(move || {
            for response in [
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nindex".to_owned(),
                "HTTP/1.1 302 Found\r\nLocation: /entrypoint\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nentry".to_owned(),
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {outside}/payload\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            ] {
                let (mut stream, _) = origin_listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });

        let server = DevServer::connect(&origin, "/index.html").unwrap();
        assert_eq!(server.read("/entrypoint"), Some(b"entry".to_vec()));
        assert_eq!(server.read("/cross-origin"), None);
        let error = server.last_error().unwrap();
        assert!(error.contains("redirect"), "{error}");
        assert!(error.contains("only requests from"), "{error}");
        origin_server.join().unwrap();

        outside_listener.set_nonblocking(true).unwrap();
        assert!(
            outside_listener.accept().is_err(),
            "the redirect target must not receive a request"
        );
    }
}
