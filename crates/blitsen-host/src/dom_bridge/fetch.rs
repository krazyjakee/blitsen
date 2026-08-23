//! Off-main-thread `fetch` execution for the DOM bridge.
//!
//! Requests are issued on a shared tokio worker pool, never on the thread that
//! owns the DOM. Completions queue in [`Shared`] and JavaScript drains them at
//! exactly one point in the frame turn — the start of the animation-frame stage,
//! before any `requestAnimationFrame` callback runs — so a response can never
//! land in the middle of one.
//!
//! Bodies stay in Rust until the application reads them, keyed by request id, so
//! the bytes cross the engine boundary once and only in the shape that was asked
//! for. The bootstrap releases an unread body when its `Response` is collected.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use blitsen_js::JsError;
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::net_pool::runtime as net_runtime;

/// A `fetch` call as the bootstrap describes it, with the body passed
/// separately so binary payloads never round-trip through a string.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestSpec {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
}

/// State shared with the worker pool.
#[derive(Default)]
struct Shared {
    completed: Mutex<Vec<Value>>,
    bodies: Mutex<HashMap<u64, Vec<u8>>>,
}

/// The `fetch` executor owned by one JavaScript context.
pub(super) struct FetchHost {
    runtime: &'static Runtime,
    client: Client,
    next_id: AtomicU64,
    inflight: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    shared: Arc<Shared>,
    /// How a URL naming a file the application shipped is read (issue #125).
    /// Absent in the bare harness, which has no application behind it.
    reader: Option<crate::app::AppReader>,
}

/// Builds the header map, rejecting names or values HTTP cannot carry.
fn header_map(headers: &[(String, String)]) -> Result<HeaderMap, JsError> {
    let mut map = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| JsError::new(format!("invalid header name: {name}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| JsError::new(format!("invalid header value for {name}")))?;
        map.append(name, value);
    }
    Ok(map)
}

/// Builds the completion record for a request that never produced a response.
///
/// The whole source chain is reported: `reqwest`'s own message stops at "error
/// sending request", which names nothing an application author can act on.
fn failure(id: u64, error: &reqwest::Error) -> Value {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    let name = if error.is_timeout() {
        "TimeoutError"
    } else {
        "TypeError"
    };
    json!({ "id": id, "error": { "name": name, "message": message } })
}

/// Builds the completion record for a file the application shipped.
///
/// Shaped exactly like a network completion, because an application should not
/// be able to tell which one it got: the same fields, read at the same point in
/// the frame turn, differing only in that nothing was sent.
fn local_completion(id: u64, url: &Url, bytes: Vec<u8>, shared: &Shared) -> Value {
    let length = bytes.len();
    let content_type = content_type(url.path());
    shared.bodies.lock().insert(id, bytes);
    json!({
        "id": id,
        "ok": true,
        "status": 200,
        "statusText": "OK",
        "url": url.to_string(),
        "redirected": false,
        "headers": [
            ["content-type", content_type],
            ["content-length", length.to_string()],
        ],
    })
}

/// Explains why a URL cannot be served by either fetch transport.
fn outside_application(url: &Url) -> String {
    format!(
        "fetch reaches http, https, and the files this application shipped; \
         {url} is none of them"
    )
}

/// What a path's extension says its bytes are.
///
/// Enough to answer `response.json()`, an `ArrayBuffer` for `decodeAudioData`,
/// and a `Blob` with a useful `type`. An extension not named here is answered as
/// bytes rather than guessed at, which is what the application asked for anyway.
fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "json" => "application/json",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "html" | "htm" => "text/html",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/vnd.microsoft.icon",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" | "aac" => "audio/aac",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Reads the response, storing its bytes for the eventual body read.
async fn completion(
    id: u64,
    requested: &Url,
    response: reqwest::Response,
    shared: &Shared,
) -> Value {
    let status = response.status();
    let redirected = response.url() != requested;
    let url = response.url().to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            json!([
                name.as_str(),
                value.to_str().unwrap_or_default().to_string()
            ])
        })
        .collect::<Vec<_>>();
    match response.bytes().await {
        Err(error) => failure(id, &error),
        Ok(bytes) => {
            shared.bodies.lock().insert(id, bytes.to_vec());
            json!({
                "id": id,
                "ok": status.is_success(),
                "status": status.as_u16(),
                "statusText": status.canonical_reason().unwrap_or(""),
                "url": url,
                "redirected": redirected,
                "headers": headers,
            })
        }
    }
}

impl FetchHost {
    /// Creates a host bound to the shared worker pool.
    pub(super) fn new(reader: Option<crate::app::AppReader>) -> Result<Self, JsError> {
        let runtime = net_runtime()?;
        // The connection pool spawns its idle reaper on construction, so the
        // client has to be built inside the runtime it will run on.
        let guard = runtime.enter();
        let client = Client::builder()
            .build()
            .map_err(|error| JsError::new(format!("could not start the HTTP client: {error}")))?;
        drop(guard);
        Ok(Self {
            runtime,
            client,
            next_id: AtomicU64::new(1),
            inflight: Mutex::new(HashMap::new()),
            shared: Arc::default(),
            reader,
        })
    }

    /// Issues a request on the worker pool and returns its identifier.
    pub(super) fn start(&self, spec: &RequestSpec, body: Option<Vec<u8>>) -> Result<u64, JsError> {
        let method = Method::from_bytes(spec.method.as_bytes())
            .map_err(|_| JsError::new(format!("invalid HTTP method: {}", spec.method)))?;
        let url = Url::parse(&spec.url)
            .map_err(|error| JsError::new(format!("invalid fetch URL {}: {error}", spec.url)))?;
        if !matches!(url.scheme(), "http" | "https") {
            return self.start_local(&url);
        }
        let mut builder = self
            .client
            .request(method, url)
            .headers(header_map(&spec.headers)?);
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let request = builder
            .build()
            .map_err(|error| JsError::new(format!("could not build the request: {error}")))?;
        let id = self.id();
        let client = self.client.clone();
        let shared = Arc::clone(&self.shared);
        // The URL after `build`, so a redirect is measured against what was
        // actually sent rather than against what the caller typed.
        let requested = request.url().clone();
        let task = self.runtime.spawn(async move {
            let record = match client.execute(request).await {
                Ok(response) => completion(id, &requested, response, &shared).await,
                Err(error) => failure(id, &error),
            };
            shared.completed.lock().push(record);
        });
        self.inflight.lock().insert(id, task.abort_handle());
        Ok(id)
    }

    /// Answers a URL that names a file the application shipped (issue #125).
    ///
    /// There is no server behind an exported application, and there is still no
    /// origin to ask for permission — but `new URL('./data.json', import.meta.url)`
    /// names a file the export carries, and refusing to read it left an
    /// application unable to load its own assets at all. So a URL that resolves
    /// inside the application is read from the same source the module resolver
    /// and the renderer read, and everything else keeps the old answer.
    ///
    /// Read on the pool and delivered through the ordinary completion queue, so
    /// a large file does not stall the frame it was asked for on, and so what an
    /// application observes — a promise settling at the animation-frame drain —
    /// is the same for a file as for a response off the network.
    fn start_local(&self, url: &Url) -> Result<u64, JsError> {
        let no_server = || JsError::new(outside_application(url));
        let reader = self.reader.clone().ok_or_else(no_server)?;
        // A file has no verbs. Answering 405 would be the server-shaped reply,
        // and there is no server: posting to a bundled file is a mistake in the
        // application rather than a request it should have to check the status of.
        let id = self.id();
        let url = url.clone();
        let shared = Arc::clone(&self.shared);
        let task = self.runtime.spawn_blocking(move || {
            let record = match reader.read_url(&url) {
                Ok(bytes) => local_completion(id, &url, bytes, &shared),
                // The web's answer for a file that is not there, so an
                // application that checks `response.ok` and falls back keeps
                // working. `doctor` is where a path that names nothing shipped
                // is meant to be caught, and it is caught at build time.
                Err(crate::app::NotRead::Missing(_)) => {
                    // An empty body rather than none: `take_body` treats a
                    // missing entry as a body already read, and a 404 whose
                    // `.text()` threw would be a different bug to chase.
                    shared.bodies.lock().insert(id, Vec::new());
                    json!({
                        "id": id,
                        "ok": false,
                        "status": 404,
                        "statusText": "Not Found",
                        "url": url.to_string(),
                        "redirected": false,
                        "headers": [["content-length", "0"]],
                    })
                }
                Err(crate::app::NotRead::Outside) => json!({
                    "id": id,
                    "error": {
                        "name": "TypeError",
                        "message": outside_application(&url),
                    },
                }),
            };
            shared.completed.lock().push(record);
        });
        self.inflight.lock().insert(id, task.abort_handle());
        Ok(id)
    }

    /// The next request identifier.
    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Drains everything that finished since the previous frame turn.
    pub(super) fn poll(&self) -> Value {
        let completed = std::mem::take(&mut *self.shared.completed.lock());
        let mut inflight = self.inflight.lock();
        for record in &completed {
            if let Some(id) = record["id"].as_u64() {
                inflight.remove(&id);
            }
        }
        json!({ "pending": inflight.len(), "completed": completed })
    }

    /// Cancels a request and forgets anything it already produced.
    ///
    /// Also the release path for a `Response` whose body was never read.
    pub(super) fn cancel(&self, id: u64) {
        if let Some(task) = self.inflight.lock().remove(&id) {
            task.abort();
        }
        self.shared.bodies.lock().remove(&id);
        self.shared
            .completed
            .lock()
            .retain(|record| record["id"].as_u64() != Some(id));
    }

    /// Takes the response body, which may be read exactly once.
    pub(super) fn take_body(&self, id: u64) -> Result<Vec<u8>, JsError> {
        self.shared
            .bodies
            .lock()
            .remove(&id)
            .ok_or_else(|| JsError::new("the response body is no longer available"))
    }

    /// Cancels every request and drops every unread body.
    pub(super) fn dispose(&self) {
        for (_, task) in self.inflight.lock().drain() {
            task.abort();
        }
        self.shared.bodies.lock().clear();
        self.shared.completed.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    use super::*;

    fn spec(url: &str, method: &str, headers: &[(&str, &str)]) -> RequestSpec {
        RequestSpec {
            url: url.to_string(),
            method: method.to_string(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    /// Serves one canned response and hands back what the client actually sent.
    fn one_shot_server(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/probe", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
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
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    /// Turns the frame loop's delivery point by hand until something lands.
    fn drain(host: &FetchHost) -> Value {
        for _ in 0..600 {
            let polled = host.poll();
            if !polled["completed"].as_array().unwrap().is_empty() {
                return polled;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no fetch completion arrived");
    }

    #[test]
    fn a_response_lands_on_the_queue_with_its_body_held_for_one_read() {
        let (url, server) = one_shot_server(
            "HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello",
        );
        let host = FetchHost::new(None).unwrap();
        let id = host
            .start(
                &spec(&url, "POST", &[("x-probe", "yes")]),
                Some(b"payload".to_vec()),
            )
            .unwrap();
        let polled = drain(&host);
        let record = &polled["completed"][0];
        assert_eq!(record["id"].as_u64(), Some(id));
        assert_eq!(record["status"], 201);
        assert_eq!(record["statusText"], "Created");
        assert_eq!(record["ok"], Value::Bool(true));
        assert_eq!(record["redirected"], Value::Bool(false));
        assert_eq!(polled["pending"], 0);
        let headers = record["headers"].as_array().unwrap();
        assert!(
            headers
                .iter()
                .any(|header| header[0] == "content-type" && header[1] == "text/plain")
        );
        assert_eq!(host.take_body(id).unwrap(), b"hello");
        assert!(host.take_body(id).is_err(), "a body is readable once");

        let sent = server.join().unwrap();
        assert!(sent.starts_with("POST /probe HTTP/1.1"), "{sent}");
        assert!(sent.to_ascii_lowercase().contains("x-probe: yes"), "{sent}");
        assert!(sent.ends_with("payload"), "{sent}");
    }

    #[test]
    fn a_transport_failure_arrives_as_a_completion_rather_than_a_start_error() {
        let host = FetchHost::new(None).unwrap();
        // Port 0 is unroutable, so the connection fails without a server.
        host.start(&spec("http://127.0.0.1:1/gone", "GET", &[]), None)
            .unwrap();
        let record = drain(&host)["completed"][0].clone();
        assert_eq!(record["error"]["name"], "TypeError");
        assert!(record["error"]["message"].is_string());
    }

    #[test]
    fn cancelling_forgets_the_request_its_body_and_its_completion() {
        let (url, server) = one_shot_server("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let host = FetchHost::new(None).unwrap();
        let id = host.start(&spec(&url, "GET", &[]), None).unwrap();
        drain(&host);
        host.cancel(id);
        assert!(host.take_body(id).is_err());
        assert!(host.poll()["completed"].as_array().unwrap().is_empty());
        server.join().unwrap();
    }

    #[test]
    fn schemes_without_a_server_and_malformed_requests_are_refused_at_the_call() {
        let host = FetchHost::new(None).unwrap();
        for (url, expected) in [
            // No application behind this host, so nothing addresses one.
            ("blitsen://app/data.json", "is none of them"),
            ("file:///etc/hosts", "is none of them"),
            ("/relative.json", "invalid fetch URL"),
        ] {
            let error = host.start(&spec(url, "GET", &[]), None).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }
        let error = host
            .start(&spec("http://127.0.0.1:1/", "GE T", &[]), None)
            .unwrap_err();
        assert!(
            error.message().contains("invalid HTTP method"),
            "{}",
            error.message()
        );
        let error = host
            .start(
                &spec("http://127.0.0.1:1/", "GET", &[("bad name", "v")]),
                None,
            )
            .unwrap_err();
        assert!(
            error.message().contains("invalid header name"),
            "{}",
            error.message()
        );
    }

    /// A directory of application files, and a host that can read them.
    fn application(name: &str, files: &[(&str, &[u8])]) -> (std::path::PathBuf, FetchHost) {
        let root =
            std::env::temp_dir().join(format!("blitsen-fetch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (path, bytes) in files {
            let target = root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, bytes).unwrap();
        }
        let files = crate::app::AppFiles::directory(root.join("index.html")).unwrap();
        (root, FetchHost::new(Some(files.reader())).unwrap())
    }

    #[test]
    fn outside_application_diagnostic_is_identical_with_or_without_a_reader() {
        let url = "blitsen://other/data.json";
        let bare = FetchHost::new(None).unwrap();
        let synchronous = bare
            .start(&spec(url, "GET", &[]), None)
            .unwrap_err()
            .message()
            .to_owned();

        let (root, application) = application("outside-message", &[("index.html", b"<p>hi")]);
        application.start(&spec(url, "GET", &[]), None).unwrap();
        let asynchronous = drain(&application)["completed"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .to_owned();

        assert_eq!(synchronous, asynchronous);
        assert_eq!(
            synchronous,
            "fetch reaches http, https, and the files this application shipped; \
             blitsen://other/data.json is none of them"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_the_application_shipped_is_read_like_a_response() {
        let (root, host) = application(
            "shipped",
            &[
                ("index.html", b"<p>hi"),
                ("blip.wav", b"RIFFbytes"),
                ("data.json", b"{\"ok\":true}"),
            ],
        );
        // The spelling an application actually writes:
        // `new URL('./blip.wav', import.meta.url).href`.
        let url = format!("file://{}/blip.wav", root.to_string_lossy());
        let id = host.start(&spec(&url, "GET", &[]), None).unwrap();
        let completed = drain(&host);
        let record = &completed["completed"][0];
        assert_eq!(record["status"], 200);
        assert_eq!(record["ok"], true);
        assert_eq!(host.take_body(id).unwrap(), b"RIFFbytes");
        let headers = record["headers"].as_array().unwrap();
        assert!(
            headers.contains(&json!(["content-type", "audio/wav"])),
            "{headers:?}"
        );

        // And the same file addressed the way a shipped executable addresses it,
        // so the two shapes cannot answer differently (issue #90).
        let id = host
            .start(&spec("blitsen://app/data.json", "GET", &[]), None)
            .unwrap();
        drain(&host);
        assert_eq!(host.take_body(id).unwrap(), b"{\"ok\":true}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_path_the_application_does_not_ship_is_a_404_and_one_outside_it_is_refused() {
        let (root, host) = application("missing", &[("index.html", b"<p>hi")]);
        let id = host
            .start(&spec("blitsen://app/nope.json", "GET", &[]), None)
            .unwrap();
        let completed = drain(&host);
        assert_eq!(completed["completed"][0]["status"], 404);
        assert_eq!(completed["completed"][0]["ok"], false);
        // Readable, and empty: a 404 whose body threw would be a different bug.
        assert!(host.take_body(id).unwrap().is_empty());

        // An application reading its own files is not an application reading the
        // disk, so a path outside it is refused however it is spelled.
        host.start(&spec("file:///etc/hosts", "GET", &[]), None)
            .unwrap();
        let completed = drain(&host);
        let message = completed["completed"][0]["error"]["message"]
            .as_str()
            .unwrap();
        assert!(message.contains("is none of them"), "{message}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disposing_a_context_abandons_everything_it_started() {
        let (url, server) = one_shot_server("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let host = FetchHost::new(None).unwrap();
        let id = host.start(&spec(&url, "GET", &[]), None).unwrap();
        drain(&host);
        host.dispose();
        assert!(host.take_body(id).is_err());
        assert_eq!(host.poll()["pending"], 0);
        server.join().unwrap();
    }
}
