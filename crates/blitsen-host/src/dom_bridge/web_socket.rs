//! Off-main-thread WebSocket connections for the DOM bridge.
//!
//! A connection is one task on the shared worker pool, never on the thread that
//! owns the DOM. Everything it observes — the handshake, each frame, the close —
//! queues in [`Shared`] and JavaScript drains it at exactly one point in the
//! frame turn, the start of the animation-frame stage, before any
//! `requestAnimationFrame` callback runs. A message therefore cannot arrive
//! part-way through a frame, which is the same contract `fetch` completions and
//! dialog answers are delivered under.
//!
//! Binary payloads stay in Rust until the delivering turn asks for them, keyed by
//! socket and arrival order, so the bytes cross the engine boundary once and in
//! the shape `binaryType` asked for rather than through a string.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use blitsen_js::JsError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Bytes, Message, Utf8Bytes, protocol::CloseFrame};
use url::Url;

use super::net_pool::{lock, runtime as net_runtime};

/// Reported when a connection ends without a close frame from either side.
const ABNORMAL_CLOSURE: u16 = 1006;
/// Reported when the close handshake completed but carried no status.
const NO_STATUS_RECEIVED: u16 = 1005;

/// What the frame turn hands back to a running connection.
enum Command {
    /// The byte count is carried so the buffered total can be released once the
    /// transport has taken the message, rather than when it was queued.
    Send {
        message: Message,
        bytes: usize,
    },
    Close(Option<CloseFrame>),
}

/// State shared with the worker pool.
#[derive(Default)]
struct Shared {
    events: Mutex<Vec<Value>>,
    payloads: Mutex<HashMap<(u64, u64), Vec<u8>>>,
}

impl Shared {
    fn push(&self, event: Value) {
        lock(&self.events).push(event);
    }

    /// Ends a connection the way the spec's "fail the WebSocket connection"
    /// does: an `error` the application can see, then a close it did not choose.
    fn fail(&self, id: u64) {
        self.push(json!({ "id": id, "type": "error" }));
        self.push(json!({
            "id": id, "type": "close",
            "code": ABNORMAL_CLOSURE, "reason": "", "wasClean": false,
        }));
    }
}

/// One live connection, from the frame turn's side.
struct Connection {
    commands: UnboundedSender<Command>,
    buffered: Arc<AtomicUsize>,
    task: tokio::task::AbortHandle,
}

/// The WebSocket executor owned by one JavaScript context.
pub(super) struct WebSocketHost {
    runtime: &'static Runtime,
    next_id: AtomicU64,
    open: Mutex<HashMap<u64, Connection>>,
    shared: Arc<Shared>,
}

/// Reads the subprotocol the server agreed to, which is "" when it agreed none.
fn negotiated_protocol(headers: &tokio_tungstenite::tungstenite::http::HeaderMap) -> String {
    headers
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Runs one connection to its close, queueing everything it observes.
async fn run(
    id: u64,
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
    shared: Arc<Shared>,
    buffered: Arc<AtomicUsize>,
    mut commands: UnboundedReceiver<Command>,
) {
    let connected = tokio::select! {
        result = connect_async(request) => result,
        // Only a close can arrive before the socket is open — `send` is refused
        // while the connection is opening — and the spec answers it by failing
        // the connection rather than by completing a handshake it will discard.
        _ = commands.recv() => {
            shared.fail(id);
            return;
        }
    };
    let (socket, response) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            // The message is queued rather than thrown: a handshake that fails
            // is a `close` the application listens for, not a constructor error.
            shared.push(json!({ "id": id, "type": "error", "message": error.to_string() }));
            shared.push(json!({
                "id": id, "type": "close",
                "code": ABNORMAL_CLOSURE, "reason": "", "wasClean": false,
            }));
            return;
        }
    };
    shared.push(json!({
        "id": id, "type": "open", "protocol": negotiated_protocol(response.headers()),
    }));

    let (mut sink, mut stream) = socket.split();
    let mut sequence = 0_u64;
    let mut closing = false;
    let mut received: Option<CloseFrame> = None;
    let clean = loop {
        tokio::select! {
            // Biased so a queued frame is written before the next read is
            // awaited: a send issued in frame N must not sit behind a peer that
            // has gone quiet.
            biased;
            command = commands.recv(), if !closing => match command {
                Some(Command::Send { message, bytes }) => {
                    let sent = sink.send(message).await;
                    buffered.fetch_sub(bytes, Ordering::Relaxed);
                    if sent.is_err() { break false; }
                }
                // `None` is the host being disposed of, which drops the sender.
                command => {
                    let frame = match command {
                        Some(Command::Close(frame)) => frame,
                        _ => None,
                    };
                    closing = true;
                    if sink.send(Message::Close(frame)).await.is_err() { break false; }
                }
            },
            message = stream.next() => match message {
                // The stream ends once the close handshake has been echoed both
                // ways, which is the one exit that is a clean close.
                None => break true,
                Some(Err(_)) => break false,
                Some(Ok(Message::Text(text))) => shared.push(json!({
                    "id": id, "type": "message", "text": text.as_str(),
                })),
                Some(Ok(Message::Binary(bytes))) => {
                    sequence += 1;
                    lock(&shared.payloads).insert((id, sequence), bytes.to_vec());
                    shared.push(json!({ "id": id, "type": "message", "binary": sequence }));
                }
                // The peer closed first. Reading on lets the protocol send the
                // reply, after which the stream ends and the close is clean.
                Some(Ok(Message::Close(frame))) => {
                    received = frame;
                    closing = true;
                }
                // Ping, pong and raw frames are the protocol's own business:
                // tungstenite answers a ping itself and nothing is observable.
                Some(Ok(_)) => {}
            },
        }
    };
    if !clean {
        shared.push(json!({ "id": id, "type": "error" }));
    }
    let (code, reason) = match received {
        Some(frame) => (u16::from(frame.code), frame.reason.as_str().to_owned()),
        None if clean => (NO_STATUS_RECEIVED, String::new()),
        None => (ABNORMAL_CLOSURE, String::new()),
    };
    shared.push(json!({
        "id": id, "type": "close", "code": code, "reason": reason, "wasClean": clean,
    }));
}

impl WebSocketHost {
    /// Creates a host bound to the shared worker pool.
    pub(super) fn new() -> Result<Self, JsError> {
        Ok(Self {
            runtime: net_runtime()?,
            next_id: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
            shared: Arc::default(),
        })
    }

    /// Opens a connection on the worker pool and returns its identifier.
    ///
    /// Only the address is refused here. Everything else a handshake can go
    /// wrong about is reported as `error` then `close`, because that is what the
    /// application has a listener for.
    pub(super) fn open(&self, url: &str, protocols: &[String]) -> Result<u64, JsError> {
        let parsed = Url::parse(url)
            .map_err(|error| JsError::new(format!("invalid WebSocket URL {url}: {error}")))?;
        if !matches!(parsed.scheme(), "ws" | "wss") {
            return Err(JsError::new(format!(
                "a WebSocket address is ws: or wss:; {}: is not",
                parsed.scheme()
            )));
        }
        let mut request = parsed
            .as_str()
            .into_client_request()
            .map_err(|error| JsError::new(format!("invalid WebSocket URL {url}: {error}")))?;
        if !protocols.is_empty() {
            let value = HeaderValue::from_str(&protocols.join(", "))
                .map_err(|_| JsError::new("a WebSocket subprotocol must be a header-safe token"))?;
            request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, value);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (commands, receiver) = mpsc::unbounded_channel();
        let buffered = Arc::new(AtomicUsize::new(0));
        let shared = Arc::clone(&self.shared);
        let task = self
            .runtime
            .spawn(run(id, request, shared, Arc::clone(&buffered), receiver));
        lock(&self.open).insert(
            id,
            Connection {
                commands,
                buffered,
                task: task.abort_handle(),
            },
        );
        Ok(id)
    }

    /// Queues a message, counting its bytes against the buffered total.
    ///
    /// A socket that has already gone is not an error: JavaScript refuses a send
    /// on a closed socket itself, and the close it has not drained yet is one the
    /// spec discards the message for.
    fn queue(&self, id: u64, message: Message, bytes: usize) {
        let open = lock(&self.open);
        let Some(connection) = open.get(&id) else {
            return;
        };
        connection.buffered.fetch_add(bytes, Ordering::Relaxed);
        if connection
            .commands
            .send(Command::Send { message, bytes })
            .is_err()
        {
            connection.buffered.fetch_sub(bytes, Ordering::Relaxed);
        }
    }

    pub(super) fn send_text(&self, id: u64, text: String) {
        let bytes = text.len();
        self.queue(id, Message::Text(Utf8Bytes::from(text)), bytes);
    }

    pub(super) fn send_binary(&self, id: u64, payload: Vec<u8>) {
        let bytes = payload.len();
        self.queue(id, Message::Binary(Bytes::from(payload)), bytes);
    }

    /// Bytes queued and not yet handed to the transport.
    pub(super) fn buffered(&self, id: u64) -> usize {
        lock(&self.open)
            .get(&id)
            .map_or(0, |connection| connection.buffered.load(Ordering::Relaxed))
    }

    /// Starts the close handshake. The `close` event follows from the worker.
    pub(super) fn close(&self, id: u64, code: Option<u16>, reason: &str) {
        let open = lock(&self.open);
        let Some(connection) = open.get(&id) else {
            return;
        };
        let frame = code.map(|code| CloseFrame {
            code: CloseCode::from(code),
            reason: Utf8Bytes::from(reason.to_owned()),
        });
        let _ = connection.commands.send(Command::Close(frame));
    }

    /// Drains everything the connections observed since the previous frame turn.
    pub(super) fn poll(&self) -> Value {
        let events = std::mem::take(&mut *lock(&self.shared.events));
        let mut open = lock(&self.open);
        for event in &events {
            if event["type"] == "close"
                && let Some(id) = event["id"].as_u64()
            {
                open.remove(&id);
            }
        }
        Value::Array(events)
    }

    /// Takes a binary message's bytes, which are handed over exactly once.
    pub(super) fn take_binary(&self, id: u64, sequence: u64) -> Result<Vec<u8>, JsError> {
        lock(&self.shared.payloads)
            .remove(&(id, sequence))
            .ok_or_else(|| JsError::new("the WebSocket message is no longer available"))
    }

    /// Drops every connection and everything they had queued.
    pub(super) fn dispose(&self) {
        for (_, connection) in lock(&self.open).drain() {
            connection.task.abort();
        }
        lock(&self.shared.events).clear();
        lock(&self.shared.payloads).clear();
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// Accepts one connection and runs the exchange the test asked for.
    ///
    /// The server is tungstenite's own accept side on a thread of its own, so
    /// what the host is measured against is a real handshake and real framing.
    ///
    /// The handshake callback's error type is tungstenite's `ErrorResponse`, an
    /// `http::Response` the `Callback` signature fixes, so the large-`Err` lint
    /// has nothing here to act on.
    #[allow(clippy::result_large_err)]
    fn server(
        exchange: impl FnOnce(&mut tokio_tungstenite::tungstenite::WebSocket<std::net::TcpStream>)
        + Send
        + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/socket", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tokio_tungstenite::tungstenite::accept_hdr(
                stream,
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    // Echo the first subprotocol offered, which is what a server
                    // that supports one of them does.
                    if let Some(offered) = request.headers().get(SEC_WEBSOCKET_PROTOCOL)
                        && let Ok(offered) = offered.to_str()
                        && let Some(first) = offered.split(',').next()
                    {
                        response.headers_mut().insert(
                            SEC_WEBSOCKET_PROTOCOL,
                            HeaderValue::from_str(first.trim()).unwrap(),
                        );
                    }
                    Ok(response)
                },
            )
            .unwrap();
            exchange(&mut socket);
        });
        (url, handle)
    }

    /// Turns the frame loop's delivery point by hand until the events land.
    fn drain(host: &WebSocketHost, until: &str) -> Vec<Value> {
        let mut events = Vec::new();
        for _ in 0..600 {
            if let Value::Array(polled) = host.poll() {
                events.extend(polled);
            }
            if events.iter().any(|event| event["type"] == until) {
                return events;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no {until} event arrived: {events:?}");
    }

    #[test]
    fn a_handshake_frames_and_a_clean_close_arrive_on_the_queue_in_order() {
        let (url, server) = server(|socket| {
            let text = socket.read().unwrap();
            socket.send(text).unwrap();
            socket
                .send(Message::Binary(Bytes::from_static(&[7, 8, 9])))
                .unwrap();
            // Reads until the client's close, which tungstenite answers itself.
            while socket.read().is_ok() {}
        });
        let host = WebSocketHost::new().unwrap();
        let id = host.open(&url, &["chat.v1".to_owned()]).unwrap();
        let opened = drain(&host, "open");
        assert_eq!(opened[0]["type"], "open");
        assert_eq!(opened[0]["protocol"], "chat.v1");

        host.send_text(id, "hello".to_owned());
        let mut events = drain(&host, "message");
        while events
            .iter()
            .filter(|event| event["type"] == "message")
            .count()
            < 2
        {
            events.extend(drain(&host, "message"));
        }
        let messages = events
            .iter()
            .filter(|event| event["type"] == "message")
            .collect::<Vec<_>>();
        assert_eq!(messages[0]["text"], "hello");
        let sequence = messages[1]["binary"].as_u64().unwrap();
        assert_eq!(host.take_binary(id, sequence).unwrap(), vec![7, 8, 9]);
        assert!(
            host.take_binary(id, sequence).is_err(),
            "a payload is handed over once"
        );

        host.close(id, Some(1000), "done");
        let closed = drain(&host, "close");
        let close = closed
            .iter()
            .find(|event| event["type"] == "close")
            .unwrap();
        assert_eq!(close["code"], 1000);
        assert_eq!(close["reason"], "done");
        assert_eq!(close["wasClean"], Value::Bool(true));
        assert!(
            !closed.iter().any(|event| event["type"] == "error"),
            "a close the application asked for is not an error: {closed:?}"
        );
        assert_eq!(host.buffered(id), 0, "a closed socket buffers nothing");
        server.join().unwrap();
    }

    #[test]
    fn a_peer_that_closes_first_reports_its_own_code_and_reason() {
        let (url, server) = server(|socket| {
            socket
                .close(Some(CloseFrame {
                    code: CloseCode::from(4001),
                    reason: Utf8Bytes::from_static("go away"),
                }))
                .unwrap();
            while socket.read().is_ok() {}
        });
        let host = WebSocketHost::new().unwrap();
        host.open(&url, &[]).unwrap();
        let events = drain(&host, "close");
        let close = events
            .iter()
            .find(|event| event["type"] == "close")
            .unwrap();
        assert_eq!(close["code"], 4001);
        assert_eq!(close["reason"], "go away");
        assert_eq!(close["wasClean"], Value::Bool(true));
        server.join().unwrap();
    }

    #[test]
    fn a_handshake_that_never_completes_fails_rather_than_throwing_at_the_call() {
        let host = WebSocketHost::new().unwrap();
        // Port 1 is unroutable, so the connection fails without a server.
        let id = host.open("ws://127.0.0.1:1/socket", &[]).unwrap();
        let events = drain(&host, "close");
        assert_eq!(events[0]["type"], "error");
        assert!(events[0]["message"].is_string());
        assert_eq!(events[1]["code"], ABNORMAL_CLOSURE);
        assert_eq!(events[1]["wasClean"], Value::Bool(false));
        assert!(
            host.poll().as_array().unwrap().is_empty(),
            "a drained close forgets the connection"
        );
        assert_eq!(host.buffered(id), 0);
    }

    #[test]
    fn addresses_without_a_socket_behind_them_are_refused_at_the_call() {
        let host = WebSocketHost::new().unwrap();
        for (url, expected) in [
            ("https://example.com/socket", "is not"),
            ("blitsen://app/socket", "is not"),
            ("/relative", "invalid WebSocket URL"),
        ] {
            let error = host.open(url, &[]).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }
        let error = host
            .open("ws://127.0.0.1:1/", &["bad\nprotocol".to_owned()])
            .unwrap_err();
        assert!(
            error.message().contains("header-safe"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn disposing_a_context_abandons_every_connection_it_opened() {
        let (url, server) = server(|socket| while socket.read().is_ok() {});
        let host = WebSocketHost::new().unwrap();
        let id = host.open(&url, &[]).unwrap();
        drain(&host, "open");
        host.dispose();
        assert!(host.poll().as_array().unwrap().is_empty());
        assert_eq!(host.buffered(id), 0);
        // The send lands nowhere rather than panicking on a connection that is
        // gone, which is the shape `__blitsenDisposeContext` leaves behind.
        host.send_text(id, "ignored".to_owned());
        host.close(id, Some(1000), "");
        server.join().unwrap();
    }
}
