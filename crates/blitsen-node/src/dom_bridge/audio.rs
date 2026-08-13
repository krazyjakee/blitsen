//! Audio playback for the DOM bridge (issue #81).
//!
//! Backed by `web-audio-api`, which is the Web Audio API rather than a playback
//! library the graph would have to be rebuilt on: the nodes below are that
//! crate's nodes, so what a caller schedules is what the specification says it
//! scheduled. The bridge's job is only to name them from JavaScript.
//!
//! Three decisions are worth reading before the code.
//!
//! **The context is lazy.** Constructing an `AudioContext` opens an output
//! device and starts a render thread, so nothing here runs until an application
//! asks for one. A Blitsen application that never plays a sound never touches
//! the sound card.
//!
//! **A machine with no output device still runs.** `try_new` failing is not
//! fatal: the context falls back to the crate's `"none"` sink, which is a real
//! context that renders to nothing. An application on a headless build server
//! behaves like one whose user has muted the speakers, which is the same thing
//! as far as its own code can tell.
//!
//! **Decoding never touches the audio context.** It runs on the shared worker
//! pool against a throwaway `OfflineAudioContext`, which has no device and no
//! thread of its own; the decoded buffer is then played by whichever context
//! wants it. That keeps `decodeAudioData` off the main thread without sharing
//! the live context across one, and it is why the harness can decode with no
//! device at all.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use blitsen_js::JsError;
use serde_json::{Value, json};
use web_audio_api::AudioBuffer;
use web_audio_api::context::{
    AudioContext, AudioContextOptions, AudioContextState, BaseAudioContext, OfflineAudioContext,
};
use web_audio_api::node::{
    AudioBufferSourceNode, AudioNode, AudioScheduledSourceNode, GainNode, StereoPannerNode,
};

use super::worker::{lock, runtime as worker_runtime};

/// How the bridge is rendering.
///
/// `Offline` exists so audio can be asserted on rendered samples rather than on
/// the calls that were made — the same reason the renderer's tests read painted
/// pixels. It is selected by `BLITSEN_AUDIO_OFFLINE` and is not something an
/// application can reach.
// Boxed because an `OfflineAudioContext` is several times the size of a live
// one, and this enum is stored once per JavaScript context either way.
#[allow(clippy::large_enum_variant)]
enum Backend {
    Live(AudioContext),
    /// Taken by `render`, because rendering consumes the context.
    Offline(Option<Box<OfflineAudioContext>>),
}

/// The nodes this bridge exposes.
///
/// Deliberately four, not everything the crate implements: a context, gain,
/// stereo panning and a buffer source are what the issue asks for, and every
/// name here becomes a published claim in the API manifest. What is absent is
/// listed in COMPATIBILITY.md rather than half-built.
enum Node {
    Gain(GainNode),
    Panner(StereoPannerNode),
    Source(Box<AudioBufferSourceNode>),
}

impl Node {
    fn as_audio_node(&self) -> &dyn AudioNode {
        match self {
            Node::Gain(node) => node,
            Node::Panner(node) => node,
            Node::Source(node) => node.as_ref(),
        }
    }

    /// Resolves one of the node's `AudioParam`s by the name JavaScript used.
    fn param(&self, name: &str) -> Result<&web_audio_api::AudioParam, JsError> {
        match (self, name) {
            (Node::Gain(node), "gain") => Ok(node.gain()),
            (Node::Panner(node), "pan") => Ok(node.pan()),
            (Node::Source(node), "playbackRate") => Ok(node.playback_rate()),
            (Node::Source(node), "detune") => Ok(node.detune()),
            _ => Err(JsError::new(format!("no audio parameter named {name}"))),
        }
    }
}

/// A decode that has finished, waiting for the frame turn to deliver it.
struct Decoded {
    id: u64,
    result: Result<AudioBuffer, String>,
}

#[derive(Default)]
struct Shared {
    decoded: Mutex<Vec<Decoded>>,
}

pub(super) struct AudioHost {
    backend: Mutex<Option<Backend>>,
    nodes: Mutex<HashMap<u64, Node>>,
    buffers: Mutex<HashMap<u64, AudioBuffer>>,
    next_id: AtomicU64,
    pending: AtomicU64,
    shared: Arc<Shared>,
    offline: bool,
}

/// The offline render's shape, which only the harness asks for.
const OFFLINE_CHANNELS: usize = 2;
const OFFLINE_SAMPLE_RATE: f32 = 48_000.0;
const OFFLINE_FRAMES: usize = 48_000;

impl AudioHost {
    pub(super) fn new(offline: bool) -> Self {
        Self {
            backend: Mutex::new(None),
            nodes: Mutex::new(HashMap::new()),
            buffers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            pending: AtomicU64::new(0),
            shared: Arc::new(Shared::default()),
            offline,
        }
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Opens the context if it is not open yet.
    ///
    /// The fallback to a silent sink is deliberate and reported once: an
    /// application that cannot make a sound should still run, and a thrown
    /// constructor would take down every page that plays a click on hover.
    fn with_backend<T>(&self, run: impl FnOnce(&Backend) -> T) -> Result<T, JsError> {
        let mut slot = lock(&self.backend);
        if slot.is_none() {
            *slot = Some(if self.offline {
                Backend::Offline(Some(Box::new(OfflineAudioContext::new(
                    OFFLINE_CHANNELS,
                    OFFLINE_FRAMES,
                    OFFLINE_SAMPLE_RATE,
                ))))
            } else {
                match AudioContext::try_new(AudioContextOptions::default()) {
                    Ok(context) => Backend::Live(context),
                    Err(error) => {
                        eprintln!(
                            "blitsen: no audio output device ({error}); \
                             audio will run silently"
                        );
                        Backend::Live(AudioContext::try_new(AudioContextOptions {
                            sink_id: "none".into(),
                            ..AudioContextOptions::default()
                        }).map_err(|error| {
                            JsError::new(format!("could not start an audio context: {error}"))
                        })?)
                    }
                }
            });
        }
        Ok(run(slot.as_ref().expect("context was just opened")))
    }

    fn context_state(&self) -> Result<Value, JsError> {
        self.with_backend(|backend| match backend {
            Backend::Live(context) => json!({
                "sampleRate": context.sample_rate(),
                "currentTime": context.current_time(),
                "state": state_name(context.state()),
                "offline": false,
            }),
            Backend::Offline(context) => {
                let context = context.as_ref().expect("offline context is live until rendered");
                json!({
                    "sampleRate": context.sample_rate(),
                    "currentTime": context.current_time(),
                    "state": "suspended",
                    "offline": true,
                })
            }
        })
    }

    fn create(&self, kind: &str) -> Result<u64, JsError> {
        let node = self.with_backend(|backend| match (backend, kind) {
            (Backend::Live(context), "gain") => Some(Node::Gain(context.create_gain())),
            (Backend::Live(context), "panner") => {
                Some(Node::Panner(context.create_stereo_panner()))
            }
            (Backend::Live(context), "source") => {
                Some(Node::Source(Box::new(context.create_buffer_source())))
            }
            (Backend::Offline(context), kind) => {
                let context = context.as_ref().expect("offline context is live until rendered");
                match kind {
                    "gain" => Some(Node::Gain(context.create_gain())),
                    "panner" => Some(Node::Panner(context.create_stereo_panner())),
                    "source" => Some(Node::Source(Box::new(context.create_buffer_source()))),
                    _ => None,
                }
            }
            _ => None,
        })?;
        let node = node.ok_or_else(|| JsError::new(format!("unknown audio node: {kind}")))?;
        let id = self.id();
        lock(&self.nodes).insert(id, node);
        Ok(id)
    }

    fn with_node<T>(&self, id: u64, run: impl FnOnce(&Node) -> T) -> Result<T, JsError> {
        let nodes = lock(&self.nodes);
        let node = nodes
            .get(&id)
            .ok_or_else(|| JsError::new("the audio node has been released"))?;
        Ok(run(node))
    }

    /// Connects one node to another, or to the destination when `to` is zero.
    ///
    /// Zero is the destination rather than a node id because the destination is
    /// the context's, not the registry's: it is not created, cannot be released,
    /// and there is exactly one.
    fn connect(&self, from: u64, to: u64) -> Result<(), JsError> {
        let nodes = lock(&self.nodes);
        let source = nodes
            .get(&from)
            .ok_or_else(|| JsError::new("the audio node has been released"))?;
        if to == 0 {
            return self.with_backend(|backend| match backend {
                Backend::Live(context) => {
                    source.as_audio_node().connect(&context.destination());
                }
                Backend::Offline(context) => {
                    let context = context.as_ref().expect("offline context is live until rendered");
                    source.as_audio_node().connect(&context.destination());
                }
            });
        }
        let destination = nodes
            .get(&to)
            .ok_or_else(|| JsError::new("the audio node has been released"))?;
        source.as_audio_node().connect(destination.as_audio_node());
        Ok(())
    }

    /// Starts a decode on the worker pool.
    ///
    /// The throwaway context is built inside the task: it has no device and no
    /// render thread, so it costs nothing to make and nothing is shared with
    /// the context that will eventually play the buffer. A decoded `AudioBuffer`
    /// is plain sample data and plays on any context at the same rate.
    fn decode(&self, bytes: Vec<u8>, sample_rate: f32) -> Result<u64, JsError> {
        let id = self.id();
        let shared = Arc::clone(&self.shared);
        self.pending.fetch_add(1, Ordering::Relaxed);
        worker_runtime()?.spawn_blocking(move || {
            let result = decode_bytes(bytes, sample_rate);
            lock(&shared.decoded).push(Decoded { id, result });
        });
        Ok(id)
    }

    /// Drains finished decodes, registering the buffers they produced.
    pub(super) fn poll(&self) -> Value {
        let finished = std::mem::take(&mut *lock(&self.shared.decoded));
        let mut delivered = Vec::with_capacity(finished.len());
        for entry in finished {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            delivered.push(match entry.result {
                Ok(buffer) => {
                    let id = self.id();
                    let record = buffer_record(id, &buffer);
                    lock(&self.buffers).insert(id, buffer);
                    json!({ "id": entry.id, "buffer": record })
                }
                Err(message) => json!({ "id": entry.id, "error": message }),
            });
        }
        Value::Array(delivered)
    }

    /// Whether anything is owed, so the host keeps turning until it lands.
    pub(super) fn pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0
    }

    fn buffer(&self, id: u64) -> Result<AudioBuffer, JsError> {
        lock(&self.buffers)
            .get(&id)
            .cloned()
            .ok_or_else(|| JsError::new("the audio buffer has been released"))
    }

    /// Renders the offline graph and reports what it produced.
    ///
    /// Harness only. Rendering consumes the context, which is why it is taken:
    /// a second render would have nothing to render.
    fn render(&self) -> Result<Value, JsError> {
        let mut slot = lock(&self.backend);
        let Some(Backend::Offline(context)) = slot.as_mut() else {
            return Err(JsError::new("only an offline audio context can be rendered"));
        };
        let mut context = context
            .take()
            .ok_or_else(|| JsError::new("the offline audio context has already been rendered"))?;
        let rendered = context.start_rendering_sync();
        let channels = (0..rendered.number_of_channels())
            .map(|channel| {
                let samples = rendered.get_channel_data(channel);
                let peak = samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                // The sum of squares is what tells silence from a signal that
                // merely never peaks, which a peak alone cannot.
                let energy = samples.iter().map(|sample| f64::from(*sample) * f64::from(*sample)).sum::<f64>();
                json!({ "peak": peak, "energy": energy })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "length": rendered.length(),
            "sampleRate": rendered.sample_rate(),
            "channels": channels,
        }))
    }

    pub(super) fn dispose(&self) {
        lock(&self.nodes).clear();
        lock(&self.buffers).clear();
        lock(&self.shared.decoded).clear();
        self.pending.store(0, Ordering::Relaxed);
        if let Some(Backend::Live(context)) = lock(&self.backend).take() {
            context.close_sync();
        }
    }

    /// Every operation the bootstrap can name, dispatched by string.
    pub(super) fn dispatch(&self, operation: &str, arguments: &[String]) -> Result<Value, JsError> {
        let number = |index: usize| -> Result<f64, JsError> {
            arguments
                .get(index)
                .ok_or_else(|| JsError::new(format!("missing audio argument {index}")))?
                .parse::<f64>()
                .map_err(|_| JsError::new(format!("invalid audio argument {index}")))
        };
        let id = |index: usize| -> Result<u64, JsError> { Ok(number(index)? as u64) };
        let text = |index: usize| -> Result<&str, JsError> {
            arguments
                .get(index)
                .map(String::as_str)
                .ok_or_else(|| JsError::new(format!("missing audio argument {index}")))
        };
        match operation {
            "context" => self.context_state(),
            "create" => Ok(json!(self.create(text(0)?)?)),
            "release" => {
                lock(&self.nodes).remove(&id(0)?);
                Ok(Value::Null)
            }
            "connect" => {
                self.connect(id(0)?, id(1)?)?;
                Ok(Value::Null)
            }
            "disconnect" => {
                self.with_node(id(0)?, |node| node.as_audio_node().disconnect())?;
                Ok(Value::Null)
            }
            "paramValue" => self.with_node(id(0)?, |node| {
                node.param(text(1)?).map(|param| json!(param.value()))
            })?,
            "paramSet" => {
                let value = number(2)? as f32;
                self.with_node(id(0)?, |node| {
                    node.param(text(1)?).map(|param| {
                        param.set_value(value);
                        Value::Null
                    })
                })?
            }
            // The scheduling half of AudioParam. Ramps are what a game uses for
            // a fade, and are the reason `volume = x` is not the whole surface.
            "paramSchedule" => self.with_node(id(0)?, |node| {
                let param = node.param(text(1)?)?;
                let value = number(3)? as f32;
                let when = number(4)?;
                match text(2)? {
                    "setValueAtTime" => { param.set_value_at_time(value, when); }
                    "linearRampToValueAtTime" => { param.linear_ramp_to_value_at_time(value, when); }
                    "exponentialRampToValueAtTime" => {
                        param.exponential_ramp_to_value_at_time(value, when);
                    }
                    "setTargetAtTime" => { param.set_target_at_time(value, when, number(5)?); }
                    "cancelScheduledValues" => { param.cancel_scheduled_values(when); }
                    other => return Err(JsError::new(format!("unknown parameter schedule: {other}"))),
                }
                Ok(Value::Null)
            })?,
            "sourceBuffer" => {
                let buffer = self.buffer(id(1)?)?;
                self.with_node(id(0)?, |node| match node {
                    Node::Source(_) => Ok(()),
                    _ => Err(JsError::new("only a buffer source has a buffer")),
                })??;
                let mut nodes = lock(&self.nodes);
                match nodes.get_mut(&id(0)?) {
                    Some(Node::Source(source)) => {
                        source.set_buffer(buffer);
                        Ok(Value::Null)
                    }
                    _ => Err(JsError::new("only a buffer source has a buffer")),
                }
            }
            "sourceLoop" => {
                let mut nodes = lock(&self.nodes);
                match nodes.get_mut(&id(0)?) {
                    Some(Node::Source(source)) => {
                        source.set_loop(number(1)? != 0.0);
                        Ok(Value::Null)
                    }
                    _ => Err(JsError::new("only a buffer source loops")),
                }
            }
            // `start` takes ownership of the schedule in this crate, so a source
            // that has been started cannot be started again — which is also what
            // the specification says about an `AudioBufferSourceNode`.
            "sourceStart" => {
                let mut nodes = lock(&self.nodes);
                match nodes.get_mut(&id(0)?) {
                    Some(Node::Source(source)) => {
                        let when = number(1)?;
                        let offset = number(2)?;
                        source.start_at_with_offset(when, offset);
                        Ok(Value::Null)
                    }
                    _ => Err(JsError::new("only a buffer source can be started")),
                }
            }
            "sourceStop" => {
                let mut nodes = lock(&self.nodes);
                match nodes.get_mut(&id(0)?) {
                    Some(Node::Source(source)) => {
                        source.stop_at(number(1)?);
                        Ok(Value::Null)
                    }
                    _ => Err(JsError::new("only a buffer source can be stopped")),
                }
            }
            "bufferInfo" => {
                let buffer = self.buffer(id(0)?)?;
                Ok(buffer_record(id(0)?, &buffer))
            }
            "releaseBuffer" => {
                lock(&self.buffers).remove(&id(0)?);
                Ok(Value::Null)
            }
            "resume" | "suspend" | "close" => {
                self.with_backend(|backend| {
                    if let Backend::Live(context) = backend {
                        match operation {
                            "resume" => context.resume_sync(),
                            "suspend" => context.suspend_sync(),
                            _ => context.close_sync(),
                        }
                    }
                })?;
                self.context_state()
            }
            "render" => self.render(),
            other => Err(JsError::new(format!("unknown audio operation: {other}"))),
        }
    }

    /// Loads a URL and decodes it, both on the worker pool.
    ///
    /// This exists because `fetch` cannot read a local file — it is http(s) only
    /// and says so — and a desktop application's sounds are files it shipped.
    /// The renderer already reads subresources off disk for images and fonts;
    /// this is the same capability for audio, reached the same way: a URL
    /// already resolved against the document's real base.
    pub(super) fn start_load(&self, url: &str) -> Result<u64, JsError> {
        let parsed = url::Url::parse(url)
            .map_err(|error| JsError::new(format!("invalid audio source {url}: {error}")))?;
        let id = self.id();
        let shared = Arc::clone(&self.shared);
        let sample_rate = self.sample_rate()?;
        self.pending.fetch_add(1, Ordering::Relaxed);
        let runtime = worker_runtime()?;
        match parsed.scheme() {
            "file" => {
                let path = parsed
                    .to_file_path()
                    .map_err(|()| JsError::new(format!("{url} is not a readable path")))?;
                runtime.spawn_blocking(move || {
                    let result = std::fs::read(&path)
                        .map_err(|error| format!("{}: {error}", path.display()))
                        .and_then(|bytes| decode_bytes(bytes, sample_rate));
                    lock(&shared.decoded).push(Decoded { id, result });
                });
            }
            "http" | "https" => {
                runtime.spawn(async move {
                    let result = match reqwest::get(parsed).await {
                        Err(error) => Err(error.to_string()),
                        Ok(response) if !response.status().is_success() => {
                            Err(format!("{} for the audio source", response.status()))
                        }
                        Ok(response) => match response.bytes().await {
                            Err(error) => Err(error.to_string()),
                            Ok(bytes) => decode_bytes(bytes.to_vec(), sample_rate),
                        },
                    };
                    lock(&shared.decoded).push(Decoded { id, result });
                });
            }
            other => {
                self.pending.fetch_sub(1, Ordering::Relaxed);
                return Err(JsError::new(format!(
                    "an audio source is a file, http or https URL, not {other}:"
                )));
            }
        }
        Ok(id)
    }

    fn sample_rate(&self) -> Result<f32, JsError> {
        self.with_backend(|backend| match backend {
            Backend::Live(context) => context.sample_rate(),
            Backend::Offline(context) => context
                .as_ref()
                .expect("offline context is live until rendered")
                .sample_rate(),
        })
    }

    /// Registers decoded bytes, used by the harness and by `createBuffer`.
    pub(super) fn start_decode(&self, bytes: Vec<u8>) -> Result<u64, JsError> {
        let rate = self.sample_rate()?;
        self.decode(bytes, rate)
    }

    /// Copies one channel of a decoded buffer out for `getChannelData`.
    pub(super) fn channel_data(&self, buffer: u64, channel: usize) -> Result<Vec<f32>, JsError> {
        let buffer = self.buffer(buffer)?;
        if channel >= buffer.number_of_channels() {
            return Err(JsError::new("the audio buffer has no such channel"));
        }
        Ok(buffer.get_channel_data(channel).to_vec())
    }
}

/// Decodes encoded audio to a buffer at `sample_rate`.
///
/// On a throwaway offline context, which has neither a device nor a thread of
/// its own: a decoded buffer is plain sample data and plays on any context at
/// the same rate, so nothing needs the live one.
fn decode_bytes(bytes: Vec<u8>, sample_rate: f32) -> Result<AudioBuffer, String> {
    OfflineAudioContext::new(1, 1, sample_rate)
        .decode_audio_data_sync(Cursor::new(bytes))
        .map_err(|error| error.to_string())
}

fn buffer_record(id: u64, buffer: &AudioBuffer) -> Value {
    json!({
        "id": id,
        "numberOfChannels": buffer.number_of_channels(),
        "length": buffer.length(),
        "sampleRate": buffer.sample_rate(),
        "duration": buffer.duration(),
    })
}

fn state_name(state: AudioContextState) -> &'static str {
    match state {
        AudioContextState::Suspended => "suspended",
        AudioContextState::Running => "running",
        AudioContextState::Closed => "closed",
    }
}
