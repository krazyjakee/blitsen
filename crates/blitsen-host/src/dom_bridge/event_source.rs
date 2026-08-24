//! Server-sent event streams for the DOM bridge.
//!
//! One stream is one task on the shared worker pool, never on the thread that
//! owns the DOM. What it observes — the response, every dispatched event, each
//! reconnection — queues in [`Shared`] and JavaScript drains it at exactly one
//! point in the frame turn, the start of the animation-frame stage, before any
//! `requestAnimationFrame` callback runs. That is the contract `fetch`
//! completions and WebSocket frames are delivered under, and an `EventSource`
//! that broke it would be the one transport able to re-enter an application
//! part-way through a frame.
//!
//! Reconnection lives here rather than in the bootstrap because its inputs do:
//! the last event id a server sent, and the interval a `retry:` field asked
//! for, are transport state, and a JavaScript half that owned the retry timer
//! would have to be told both across the boundary on every event. What crosses
//! instead is what the application can observe — a stream that opened, an event
//! that arrived, a connection that dropped and is coming back, and one that
//! failed for good.
//!
//! The parser is byte-level on purpose. A chunk boundary falls wherever the
//! network put it: mid-line, between the CR and LF of one break, or inside a
//! multi-byte character. Lines are therefore assembled from bytes and decoded
//! only once complete, which is also what the specification's decode-then-split
//! order amounts to for well-formed UTF-8.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use blitsen_js::JsError;
use futures_util::StreamExt;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::net_pool::{client, runtime as net_runtime};

/// How long to wait before reconnecting when no server has said otherwise.
///
/// The specification leaves the default to the implementation; three seconds is
/// what browsers settled on, and a stream written against one of them has its
/// server-side keepalive and its client's patience tuned to that number.
const DEFAULT_RETRY: Duration = Duration::from_millis(3_000);

/// The floor under a `retry:` a server asks for.
///
/// The sleep on the reconnection interval is the only pause in the reconnect
/// loop, on the connection-refused path as much as after a stream that ended —
/// so a server sending `retry: 0` and closing would have this task spinning
/// through connections with no delay at all. A second is low enough for any
/// legitimate rapid-retry stream and high enough not to be a busy loop.
const MIN_RETRY: Duration = Duration::from_secs(1);

/// The media type a stream has to arrive as for it to be one.
const EVENT_STREAM: &str = "text/event-stream";

/// State shared with the worker pool.
#[derive(Default)]
struct Shared {
    events: Mutex<Vec<Value>>,
}

impl Shared {
    fn push(&self, event: Value) {
        self.events.lock().push(event);
    }
}

/// The event-stream executor owned by one JavaScript context.
pub(super) struct EventSourceHost {
    runtime: &'static Runtime,
    client: Client,
    next_id: AtomicU64,
    open: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    shared: Arc<Shared>,
}

/// One stream's field buffers, carried across chunks and across reconnections.
///
/// The last event id outlives a dispatch — it is what the next connection sends
/// back in `Last-Event-ID`, and it stays set through an event that does not
/// carry an `id:` of its own — while the event type and data buffers are reset
/// by every dispatch.
#[derive(Default)]
struct Interpreter {
    /// Bytes of a line the previous chunk ended part-way through.
    pending: Vec<u8>,
    /// Whether the previous chunk ended on a CR, so a LF opening the next one
    /// is the second half of one break rather than an empty line of its own.
    after_cr: bool,
    event_type: String,
    data: String,
    last_event_id: String,
}

/// What one parsed chunk asks the connection to do.
enum Interpreted {
    /// An event to deliver, already shaped as the bootstrap receives it.
    Dispatch(Value),
    /// A `retry:` field: the reconnection interval from here on.
    Retry(Duration),
}

impl Interpreter {
    /// Feeds one chunk of the response body through the line parser.
    fn chunk(&mut self, id: u64, bytes: &[u8]) -> Vec<Interpreted> {
        let mut interpreted = Vec::new();
        for &byte in bytes {
            match byte {
                b'\n' if std::mem::take(&mut self.after_cr) => {}
                b'\n' | b'\r' => {
                    self.after_cr = byte == b'\r';
                    let line = std::mem::take(&mut self.pending);
                    self.line(id, &String::from_utf8_lossy(&line), &mut interpreted);
                }
                byte => {
                    self.after_cr = false;
                    self.pending.push(byte);
                }
            }
        }
        interpreted
    }

    /// Applies one complete line.
    fn line(&mut self, id: u64, line: &str, interpreted: &mut Vec<Interpreted>) {
        if line.is_empty() {
            if let Some(event) = self.dispatch(id) {
                interpreted.push(Interpreted::Dispatch(event));
            }
            return;
        }
        // A line opening with a colon is a comment, which is how a server keeps
        // an idle connection alive without delivering anything.
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            // The space after the colon is part of the syntax, not the value,
            // and only the first one is.
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => {
                self.event_type.clear();
                self.event_type.push_str(value);
            }
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            // A NUL is the one value refused outright: the id travels back in a
            // header, and the specification will not let one carry a NUL.
            "id" if !value.contains('\0') => {
                self.last_event_id.clear();
                self.last_event_id.push_str(value);
            }
            "retry" if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
                if let Ok(milliseconds) = value.parse::<u64>() {
                    // Clamped to [`MIN_RETRY`]: applied as parsed, `retry: 0`
                    // would make the reconnect loop's only pause a zero-length
                    // sleep — a busy loop.
                    interpreted.push(Interpreted::Retry(
                        Duration::from_millis(milliseconds).max(MIN_RETRY),
                    ));
                }
            }
            // Anything else — including an `id:` carrying a NUL and a `retry:`
            // that is not a number — is ignored, which is what the parser is
            // required to do with a field it does not know.
            _ => {}
        }
    }

    /// Ends an event, or discards the buffers when there is nothing to deliver.
    fn dispatch(&mut self, id: u64) -> Option<Value> {
        let event_type = std::mem::take(&mut self.event_type);
        let mut data = std::mem::take(&mut self.data);
        if data.is_empty() {
            // A block with no `data:` at all dispatches nothing, but it has
            // still set the last event id if it carried one.
            return None;
        }
        // The newline every `data:` appended is a separator, so the last one is
        // not part of the value.
        data.pop();
        Some(json!({
            "id": id,
            "type": "message",
            "event": if event_type.is_empty() { "message" } else { &event_type },
            "data": data,
            "lastEventId": self.last_event_id,
        }))
    }
}

/// Reads the media type, ignoring the parameters that may follow it.
fn is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(EVENT_STREAM)
        })
}

/// Runs one stream, reconnecting until the application closes it.
///
/// The two ways a connection can end are not the same event. A response that is
/// not a `200 text/event-stream` is the server saying the stream is not there,
/// and the specification fails the connection for good — an application that
/// kept retrying a 404 would hammer it forever. Everything else — a refused
/// connection, a socket that dropped, a body that ended — is the transport, and
/// the connection comes back after the reconnection interval.
async fn run(id: u64, client: Client, url: Url, shared: Arc<Shared>) {
    let mut retry = DEFAULT_RETRY;
    let mut interpreter = Interpreter::default();
    loop {
        let mut request = client
            .get(url.clone())
            .header(ACCEPT, EVENT_STREAM)
            .header(CACHE_CONTROL, "no-store");
        // Sent only once a server has given one. `from_str` refuses an id no
        // header can carry, and dropping it is better than dropping the
        // reconnection: the stream resumes from the start rather than not at all.
        if !interpreter.last_event_id.is_empty()
            && let Ok(value) = HeaderValue::from_str(&interpreter.last_event_id)
        {
            request = request.header("last-event-id", value);
        }
        match request.send().await {
            Err(error) => {
                shared.push(json!({
                    "id": id, "type": "error", "fatal": false, "message": error.to_string(),
                }));
            }
            Ok(response) if !response.status().is_success() => {
                shared.push(json!({
                    "id": id, "type": "error", "fatal": true,
                    "message": format!("the server answered {}", response.status()),
                }));
                return;
            }
            Ok(response) if !is_event_stream(response.headers()) => {
                let served = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("no content type")
                    .to_owned();
                shared.push(json!({
                    "id": id, "type": "error", "fatal": true,
                    "message": format!("the server answered {served}, not {EVENT_STREAM}"),
                }));
                return;
            }
            Ok(response) => {
                shared.push(json!({ "id": id, "type": "open" }));
                let mut body = response.bytes_stream();
                let mut dropped = None;
                while let Some(chunk) = body.next().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            dropped = Some(error.to_string());
                            break;
                        }
                    };
                    for interpreted in interpreter.chunk(id, &chunk) {
                        match interpreted {
                            Interpreted::Dispatch(event) => shared.push(event),
                            Interpreted::Retry(interval) => retry = interval,
                        }
                    }
                }
                // A stream that ends is a reconnection, not a close: the
                // application asked for a feed, and the server hanging up is
                // how a feed says "come back", not how it says "we are done".
                shared.push(json!({
                    "id": id, "type": "error", "fatal": false, "message": dropped,
                }));
                // A half-read block does not survive the connection. The last
                // event id does, and it is what the next request carries.
                interpreter.pending.clear();
                interpreter.after_cr = false;
                interpreter.event_type.clear();
                interpreter.data.clear();
            }
        }
        tokio::time::sleep(retry).await;
    }
}

impl EventSourceHost {
    /// Creates a host bound to the shared worker pool.
    pub(super) fn new() -> Result<Self, JsError> {
        let runtime = net_runtime()?;
        Ok(Self {
            runtime,
            client: client(runtime)?,
            next_id: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
            shared: Arc::default(),
        })
    }

    /// Opens a stream on the worker pool and returns its identifier.
    ///
    /// Only an address no request can be made from is refused here. A server
    /// that is not there, or answers with something that is not a stream, is
    /// reported as an `error` event, because that is what an application has a
    /// listener for.
    pub(super) fn open(&self, url: &str) -> Result<u64, JsError> {
        let parsed = Url::parse(url)
            .map_err(|error| JsError::new(format!("invalid EventSource URL {url}: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(JsError::new(format!(
                "an event stream is http: or https:; {}: is not",
                parsed.scheme()
            )));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let task = self.runtime.spawn(run(
            id,
            self.client.clone(),
            parsed,
            Arc::clone(&self.shared),
        ));
        self.open.lock().insert(id, task.abort_handle());
        Ok(id)
    }

    /// Ends a stream and stops it reconnecting.
    ///
    /// Nothing is queued in answer: `close()` is synchronous in the application
    /// and the specification fires no event for it, so the only observable is
    /// the `readyState` the bootstrap has already set.
    pub(super) fn close(&self, id: u64) {
        if let Some(task) = self.open.lock().remove(&id) {
            task.abort();
        }
    }

    /// Drains everything the streams observed since the previous frame turn.
    pub(super) fn poll(&self) -> Value {
        let events = std::mem::take(&mut *self.shared.events.lock());
        let mut open = self.open.lock();
        for event in &events {
            if event["fatal"] == Value::Bool(true)
                && let Some(id) = event["id"].as_u64()
            {
                open.remove(&id);
            }
        }
        Value::Array(events)
    }

    /// Drops every stream and everything they had queued.
    pub(super) fn dispose(&self) {
        for (_, task) in self.open.lock().drain() {
            task.abort();
        }
        self.shared.events.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    use super::*;

    /// Serves one canned response per connection and records what was asked for.
    ///
    /// Raw HTTP rather than a server crate: what is under test is a parser over
    /// a byte stream, and the tests need to choose where the chunk boundaries
    /// fall — which is exactly what a framework would take away.
    fn server(
        responses: Vec<Vec<&'static str>>,
    ) -> (String, std::sync::mpsc::Receiver<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let _ = sender.send(request_headers(&stream));
                let mut stream = stream;
                for part in response {
                    if stream.write_all(part.as_bytes()).is_err() {
                        break;
                    }
                    let _ = stream.flush();
                    // Long enough that the client reads each part as its own
                    // chunk, which is what puts the boundaries under test.
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Accept and drop anything that arrives after the script, so a
            // reconnection the test did not plan for fails rather than hangs.
            while listener.accept().is_ok() {}
        });
        (url, receiver)
    }

    fn request_headers(stream: &TcpStream) -> Vec<String> {
        let mut reader = BufReader::new(stream);
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end().to_owned();
            if line.is_empty() {
                break;
            }
            headers.push(line);
        }
        headers
    }

    const STREAM_HEADERS: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    /// Turns the frame loop's delivery point by hand until the events land.
    fn drain(host: &EventSourceHost, wanted: usize) -> Vec<Value> {
        let mut events = Vec::new();
        for _ in 0..600 {
            if let Value::Array(polled) = host.poll() {
                events.extend(polled);
            }
            if events.len() >= wanted {
                return events;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "only {} of {wanted} events arrived: {events:?}",
            events.len()
        );
    }

    #[test]
    fn fields_split_across_chunks_become_whole_events() {
        let (url, _requests) = server(vec![vec![
            STREAM_HEADERS,
            ": a comment keeps the connection warm\n\ndata: fir",
            "st\n\nevent: quote\ndata: line one\ndata: line two\nid: 7\n\n",
            "data\n\n",
        ]]);
        let host = EventSourceHost::new().unwrap();
        host.open(&url).unwrap();
        let events = drain(&host, 4);
        assert_eq!(events[0]["type"], "open");
        assert_eq!(events[1]["event"], "message");
        assert_eq!(events[1]["data"], "first");
        assert_eq!(events[1]["lastEventId"], "");
        assert_eq!(events[2]["event"], "quote");
        assert_eq!(
            events[2]["data"], "line one\nline two",
            "each data field is a line of one value"
        );
        assert_eq!(events[2]["lastEventId"], "7");
        assert_eq!(
            events[3]["data"], "",
            "a bare `data` field is an empty value, not an absent one"
        );
        assert_eq!(
            events[3]["lastEventId"], "7",
            "the last event id outlives the event that set it"
        );
    }

    #[test]
    fn a_stream_that_ends_reconnects_with_the_id_it_reached() {
        let (url, requests) = server(vec![
            vec![STREAM_HEADERS, "retry: 10\nid: 42\ndata: before\n\n"],
            vec![STREAM_HEADERS, "data: after\n\n"],
        ]);
        let host = EventSourceHost::new().unwrap();
        host.open(&url).unwrap();
        let events = drain(&host, 5);
        assert_eq!(events[1]["data"], "before");
        assert_eq!(
            events[2]["fatal"],
            Value::Bool(false),
            "a body that ended is a reconnection, not a failure: {events:?}"
        );
        assert_eq!(events[3]["type"], "open");
        assert_eq!(events[4]["data"], "after");
        assert_eq!(events[4]["lastEventId"], "42");

        let first = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            first
                .iter()
                .any(|line| line.eq_ignore_ascii_case("accept: text/event-stream")),
            "{first:?}"
        );
        assert!(
            !first
                .iter()
                .any(|line| line.to_ascii_lowercase().starts_with("last-event-id")),
            "nothing has been received yet, so there is no id to resume from: {first:?}"
        );
        let second = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            second
                .iter()
                .any(|line| line.eq_ignore_ascii_case("last-event-id: 42")),
            "the reconnection resumes from the last id the server sent: {second:?}"
        );
    }

    #[test]
    fn a_response_that_is_not_a_stream_fails_for_good() {
        for (response, expected) in [
            (
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
                "404 Not Found",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
                "not text/event-stream",
            ),
        ] {
            let (url, _requests) = server(vec![vec![response]]);
            let host = EventSourceHost::new().unwrap();
            host.open(&url).unwrap();
            let events = drain(&host, 1);
            assert_eq!(events[0]["type"], "error");
            assert_eq!(events[0]["fatal"], Value::Bool(true));
            assert!(
                events[0]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(expected),
                "{events:?}"
            );
            assert!(
                host.poll().as_array().unwrap().is_empty(),
                "a failed stream is not retried"
            );
        }
    }

    #[test]
    fn a_closed_stream_stops_reconnecting() {
        let (url, _requests) = server(vec![
            vec![STREAM_HEADERS, "retry: 10\ndata: one\n\n"],
            vec![STREAM_HEADERS, "data: two\n\n"],
        ]);
        let host = EventSourceHost::new().unwrap();
        let id = host.open(&url).unwrap();
        let events = drain(&host, 2);
        assert_eq!(events[1]["data"], "one");
        host.close(id);
        // Long enough for the reconnection — `retry: 10` clamped to the 1s
        // floor — to have happened.
        std::thread::sleep(Duration::from_millis(1_100));
        let after: Vec<Value> = host.poll().as_array().cloned().unwrap_or_default();
        assert!(
            after.iter().all(|event| event["type"] != "open"),
            "a closed stream does not reconnect: {after:?}"
        );
    }

    #[test]
    fn a_retry_of_zero_is_clamped_to_the_floor() {
        let mut interpreter = Interpreter::default();
        let interpreted = interpreter.chunk(1, b"retry: 0\nretry: 250\nretry: 5000\n");
        let intervals: Vec<Duration> = interpreted
            .iter()
            .map(|interpreted| match interpreted {
                Interpreted::Retry(interval) => *interval,
                Interpreted::Dispatch(event) => panic!("not a retry: {event}"),
            })
            .collect();
        assert_eq!(
            intervals,
            [MIN_RETRY, MIN_RETRY, Duration::from_millis(5_000)],
            "a retry below the floor is clamped to it; one above passes through"
        );
    }

    #[test]
    fn addresses_no_request_can_be_made_from_are_refused_at_the_call() {
        let host = EventSourceHost::new().unwrap();
        for (url, expected) in [
            ("ws://example.com/stream", "is not"),
            ("blitsen://app/stream", "is not"),
            ("/relative", "invalid EventSource URL"),
        ] {
            let error = host.open(url).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }
    }

    #[test]
    fn disposing_a_context_abandons_every_stream_it_opened() {
        let (url, _requests) = server(vec![vec![STREAM_HEADERS, "retry: 10\ndata: one\n\n"]]);
        let host = EventSourceHost::new().unwrap();
        let id = host.open(&url).unwrap();
        drain(&host, 2);
        host.dispose();
        assert!(host.poll().as_array().unwrap().is_empty());
        // The close lands nowhere rather than panicking on a stream that is
        // gone, which is the shape `__blitsenDisposeContext` leaves behind.
        host.close(id);
    }
}
