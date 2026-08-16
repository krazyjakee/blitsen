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

use super::net_pool::{lock, runtime as net_runtime};

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
    /// Sources that have finished playing, waiting for the frame turn.
    ///
    /// The crate calls back from the render thread, so this is the only place
    /// the two threads meet: nothing is dispatched from there, it is queued and
    /// delivered where every other off-thread result is.
    ended: Mutex<Vec<u64>>,
}

/// Which context the host will open when something first asks for one.
///
/// Three, because they answer different questions. `Device` is what an
/// application gets. `Silent` is a real context with a real clock and no output
/// device — the only one in which a sound can actually *finish*, so it is the
/// only one `ended` can be tested in. `Offline` renders to sample buffers on
/// demand and has no clock at all, which is what makes it the one that can be
/// asserted on.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Device,
    Silent,
    Offline,
}

pub(super) struct AudioHost {
    backend: Mutex<Option<Backend>>,
    nodes: Mutex<HashMap<u64, Node>>,
    buffers: Mutex<HashMap<u64, AudioBuffer>>,
    next_id: AtomicU64,
    pending: AtomicU64,
    /// Sources started and not yet ended, so the host keeps turning while
    /// something is actually playing and stops when nothing is.
    playing: AtomicU64,
    shared: Arc<Shared>,
    mode: Mutex<Mode>,
    /// How a source that names a file the application shipped is read
    /// (issue #125). Absent in the bare harness, which has no application.
    reader: Option<crate::app::AppReader>,
}

/// The offline render's shape, which only the harness asks for.
const OFFLINE_CHANNELS: usize = 2;
const OFFLINE_SAMPLE_RATE: f32 = 48_000.0;
const OFFLINE_FRAMES: usize = 48_000;

/// The string arguments supplied by the JavaScript bridge.
///
/// Keeping the positional protocol behind named readers makes each operation's
/// wire shape explicit without changing that protocol or its error messages.
#[derive(Clone, Copy)]
struct AudioArguments<'a>(&'a [String]);

impl<'a> AudioArguments<'a> {
    fn number(self, index: usize) -> Result<f64, JsError> {
        self.0
            .get(index)
            .ok_or_else(|| JsError::new(format!("missing audio argument {index}")))?
            .parse::<f64>()
            .map_err(|_| JsError::new(format!("invalid audio argument {index}")))
    }

    fn id(self, index: usize) -> Result<u64, JsError> {
        Ok(self.number(index)? as u64)
    }

    fn text(self, index: usize) -> Result<&'a str, JsError> {
        self.0
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| JsError::new(format!("missing audio argument {index}")))
    }
}

/// A group's answer: `None` when the operation belongs to another group.
type DispatchAnswer = Result<Option<Value>, JsError>;
type DispatchGroup = for<'a> fn(&AudioHost, &str, AudioArguments<'a>) -> DispatchAnswer;

const DISPATCH_GROUPS: [DispatchGroup; 5] = [
    AudioHost::dispatch_context,
    AudioHost::dispatch_nodes,
    AudioHost::dispatch_params,
    AudioHost::dispatch_sources,
    AudioHost::dispatch_buffers,
];

impl AudioHost {
    pub(super) fn new(offline: bool, reader: Option<crate::app::AppReader>) -> Self {
        let mode = if offline { Mode::Offline } else { Mode::Device };
        Self {
            reader,
            backend: Mutex::new(None),
            nodes: Mutex::new(HashMap::new()),
            buffers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            pending: AtomicU64::new(0),
            playing: AtomicU64::new(0),
            shared: Arc::new(Shared::default()),
            mode: Mutex::new(mode),
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
            *slot = Some(match *lock(&self.mode) {
                Mode::Offline => Backend::Offline(Some(Box::new(OfflineAudioContext::new(
                    OFFLINE_CHANNELS,
                    OFFLINE_FRAMES,
                    OFFLINE_SAMPLE_RATE,
                )))),
                Mode::Silent => Backend::Live(silent_context()?),
                Mode::Device => match AudioContext::try_new(AudioContextOptions::default()) {
                    Ok(context) => Backend::Live(context),
                    Err(error) => {
                        eprintln!(
                            "blitsen: no audio output device ({error}); \
                             audio will run silently"
                        );
                        Backend::Live(silent_context()?)
                    }
                },
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
                let context = context
                    .as_ref()
                    .expect("offline context is live until rendered");
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
                let context = context
                    .as_ref()
                    .expect("offline context is live until rendered");
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

    /// Mutates a buffer source while retaining the operation-specific error
    /// used when the id is absent or names another kind of node.
    fn with_source<T>(
        &self,
        id: u64,
        wrong_kind: &'static str,
        run: impl FnOnce(&mut AudioBufferSourceNode) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let mut nodes = lock(&self.nodes);
        match nodes.get_mut(&id) {
            Some(Node::Source(source)) => run(source),
            _ => Err(JsError::new(wrong_kind)),
        }
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
                    let context = context
                        .as_ref()
                        .expect("offline context is live until rendered");
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
        net_runtime()?.spawn_blocking(move || {
            let result = decode_bytes(bytes, sample_rate);
            lock(&shared.decoded).push(Decoded { id, result });
        });
        Ok(id)
    }

    /// Drains finished decodes and finished sources.
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
        let ended = std::mem::take(&mut *lock(&self.shared.ended));
        for _ in &ended {
            // Saturating: a source can only end once, but a disposed host has
            // already zeroed the count and must not wrap under it.
            let _ = self
                .playing
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |playing| {
                    Some(playing.saturating_sub(1))
                });
        }
        // A source that has ended is finished with: it cannot be started again,
        // so nothing will reach it after this and holding it would be a leak
        // for an application firing one-shots.
        {
            let mut nodes = lock(&self.nodes);
            for node in &ended {
                nodes.remove(node);
            }
        }
        json!({ "decoded": delivered, "ended": ended })
    }

    /// Whether anything is owed, so the host keeps turning until it lands.
    pub(super) fn pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0 || self.playing.load(Ordering::Relaxed) > 0
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
            return Err(JsError::new(
                "only an offline audio context can be rendered",
            ));
        };
        let mut context = context
            .take()
            .ok_or_else(|| JsError::new("the offline audio context has already been rendered"))?;
        let rendered = context.start_rendering_sync();
        let channels = (0..rendered.number_of_channels())
            .map(|channel| {
                let samples = rendered.get_channel_data(channel);
                let peak = samples
                    .iter()
                    .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                // The sum of squares is what tells silence from a signal that
                // merely never peaks, which a peak alone cannot.
                let energy = samples
                    .iter()
                    .map(|sample| f64::from(*sample) * f64::from(*sample))
                    .sum::<f64>();
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
        lock(&self.shared.ended).clear();
        self.pending.store(0, Ordering::Relaxed);
        self.playing.store(0, Ordering::Relaxed);
        if let Some(Backend::Live(context)) = lock(&self.backend).take() {
            context.close_sync();
        }
    }

    /// Every operation the bootstrap can name, dispatched by domain.
    pub(super) fn dispatch(&self, operation: &str, arguments: &[String]) -> Result<Value, JsError> {
        let arguments = AudioArguments(arguments);
        for group in DISPATCH_GROUPS {
            if let Some(value) = group(self, operation, arguments)? {
                return Ok(value);
            }
        }
        Err(JsError::new(format!(
            "unknown audio operation: {operation}"
        )))
    }

    fn dispatch_context(&self, operation: &str, arguments: AudioArguments<'_>) -> DispatchAnswer {
        match operation {
            "context" => Ok(Some(self.context_state()?)),
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
                Ok(Some(self.context_state()?))
            }
            "render" => Ok(Some(self.render()?)),
            // Harness only, and refused once a context exists: the mode decides
            // what gets opened, so changing it afterwards would describe
            // something other than what is playing.
            "mode" => {
                if lock(&self.backend).is_some() {
                    return Err(JsError::new("the audio context is already open"));
                }
                *lock(&self.mode) = match arguments.text(0)? {
                    "offline" => Mode::Offline,
                    "silent" => Mode::Silent,
                    "device" => Mode::Device,
                    other => return Err(JsError::new(format!("unknown audio mode: {other}"))),
                };
                Ok(Some(Value::Null))
            }
            _ => Ok(None),
        }
    }

    fn dispatch_nodes(&self, operation: &str, arguments: AudioArguments<'_>) -> DispatchAnswer {
        match operation {
            "create" => Ok(Some(json!(self.create(arguments.text(0)?)?))),
            "release" => {
                lock(&self.nodes).remove(&arguments.id(0)?);
                Ok(Some(Value::Null))
            }
            "connect" => {
                self.connect(arguments.id(0)?, arguments.id(1)?)?;
                Ok(Some(Value::Null))
            }
            "disconnect" => {
                self.with_node(arguments.id(0)?, |node| node.as_audio_node().disconnect())?;
                Ok(Some(Value::Null))
            }
            _ => Ok(None),
        }
    }

    fn dispatch_params(&self, operation: &str, arguments: AudioArguments<'_>) -> DispatchAnswer {
        match operation {
            "paramValue" => Ok(Some(self.with_node(arguments.id(0)?, |node| {
                node.param(arguments.text(1)?)
                    .map(|param| json!(param.value()))
            })??)),
            "paramSet" => {
                let value = arguments.number(2)? as f32;
                Ok(Some(self.with_node(arguments.id(0)?, |node| {
                    node.param(arguments.text(1)?).map(|param| {
                        param.set_value(value);
                        Value::Null
                    })
                })??))
            }
            // The scheduling half of AudioParam. Ramps are what a game uses for
            // a fade, and are the reason `volume = x` is not the whole surface.
            "paramSchedule" => Ok(Some(self.with_node(arguments.id(0)?, |node| {
                let param = node.param(arguments.text(1)?)?;
                let value = arguments.number(3)? as f32;
                let when = arguments.number(4)?;
                match arguments.text(2)? {
                    "setValueAtTime" => {
                        param.set_value_at_time(value, when);
                    }
                    "linearRampToValueAtTime" => {
                        param.linear_ramp_to_value_at_time(value, when);
                    }
                    "exponentialRampToValueAtTime" => {
                        param.exponential_ramp_to_value_at_time(value, when);
                    }
                    "setTargetAtTime" => {
                        param.set_target_at_time(value, when, arguments.number(5)?);
                    }
                    "cancelScheduledValues" => {
                        param.cancel_scheduled_values(when);
                    }
                    other => {
                        return Err(JsError::new(format!("unknown parameter schedule: {other}")));
                    }
                }
                Ok(Value::Null)
            })??)),
            _ => Ok(None),
        }
    }

    fn dispatch_sources(&self, operation: &str, arguments: AudioArguments<'_>) -> DispatchAnswer {
        match operation {
            "sourceBuffer" => {
                let buffer = self.buffer(arguments.id(1)?)?;
                self.with_node(arguments.id(0)?, |node| match node {
                    Node::Source(_) => Ok(()),
                    _ => Err(JsError::new("only a buffer source has a buffer")),
                })??;
                let source = arguments.id(0)?;
                Ok(Some(self.with_source(
                    source,
                    "only a buffer source has a buffer",
                    |source| {
                        source.set_buffer(buffer);
                        Ok(Value::Null)
                    },
                )?))
            }
            "sourceLoop" => {
                let source = arguments.id(0)?;
                Ok(Some(self.with_source(
                    source,
                    "only a buffer source loops",
                    |source| {
                        source.set_loop(arguments.number(1)? != 0.0);
                        Ok(Value::Null)
                    },
                )?))
            }
            // `start` takes ownership of the schedule in this crate, so a source
            // that has been started cannot be started again — which is also what
            // the specification says about an `AudioBufferSourceNode`.
            "sourceStart" => {
                let node = arguments.id(0)?;
                let shared = Arc::clone(&self.shared);
                Ok(Some(self.with_source(
                    node,
                    "only a buffer source can be started",
                    |source| {
                        let when = arguments.number(1)?;
                        let offset = arguments.number(2)?;
                        // Registered before the start, so a source short enough
                        // to finish immediately cannot end before anything is
                        // listening for it.
                        source.set_onended(move |_| lock(&shared.ended).push(node));
                        source.start_at_with_offset(when, offset);
                        self.playing.fetch_add(1, Ordering::Relaxed);
                        Ok(Value::Null)
                    },
                )?))
            }
            "sourceStop" => {
                let source = arguments.id(0)?;
                Ok(Some(self.with_source(
                    source,
                    "only a buffer source can be stopped",
                    |source| {
                        source.stop_at(arguments.number(1)?);
                        Ok(Value::Null)
                    },
                )?))
            }
            _ => Ok(None),
        }
    }

    fn dispatch_buffers(&self, operation: &str, arguments: AudioArguments<'_>) -> DispatchAnswer {
        match operation {
            "bufferInfo" => {
                let buffer = self.buffer(arguments.id(0)?)?;
                Ok(Some(buffer_record(arguments.id(0)?, &buffer)))
            }
            "releaseBuffer" => {
                lock(&self.buffers).remove(&arguments.id(0)?);
                Ok(Some(Value::Null))
            }
            _ => Ok(None),
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
        let runtime = net_runtime()?;
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
            // An exported application's own sounds are addressed by the
            // application origin rather than by a path, because inside a shipped
            // executable there is no directory to name (issue #125). Without this
            // arm `<audio src="thud.wav">` worked while a directory was being run
            // and stopped working the moment it was exported, which is the one
            // divergence between the two shapes that must not exist.
            _ if self.reader.is_some() => {
                let reader = self.reader.clone().expect("checked");
                runtime.spawn_blocking(move || {
                    let result = match reader.read_url(&parsed) {
                        Ok(bytes) => decode_bytes(bytes, sample_rate),
                        Err(crate::app::NotRead::Missing(path)) => {
                            Err(format!("the application ships no {path}"))
                        }
                        Err(crate::app::NotRead::Outside) => Err(format!(
                            "an audio source is a file this application shipped, \
                             or an http or https URL, not {parsed}"
                        )),
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

/// A real context with a real clock and no output device.
fn silent_context() -> Result<AudioContext, JsError> {
    AudioContext::try_new(AudioContextOptions {
        sink_id: "none".into(),
        ..AudioContextOptions::default()
    })
    .map_err(|error| JsError::new(format!("could not start an audio context: {error}")))
}

fn state_name(state: AudioContextState) -> &'static str {
    match state {
        AudioContextState::Suspended => "suspended",
        AudioContextState::Running => "running",
        AudioContextState::Closed => "closed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(host: &AudioHost, operation: &str, arguments: &[&str]) -> Result<Value, JsError> {
        let arguments = arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        host.dispatch(operation, &arguments)
    }

    fn id(value: &Value) -> String {
        value.as_u64().expect("node id").to_string()
    }

    fn error(host: &AudioHost, operation: &str, arguments: &[&str]) -> String {
        dispatch(host, operation, arguments)
            .expect_err("operation should fail")
            .message()
            .to_owned()
    }

    #[test]
    fn dispatch_routes_each_audio_operation_family() {
        let host = AudioHost::new(true, None);
        assert_eq!(dispatch(&host, "mode", &["offline"]), Ok(Value::Null));

        let gain = id(&dispatch(&host, "create", &["gain"]).unwrap());
        let panner = id(&dispatch(&host, "create", &["panner"]).unwrap());
        let source = id(&dispatch(&host, "create", &["source"]).unwrap());
        assert_eq!(
            dispatch(&host, "connect", &[&gain, &panner]),
            Ok(Value::Null)
        );

        assert_eq!(
            dispatch(&host, "paramSet", &[&gain, "gain", "0.25"]),
            Ok(Value::Null)
        );
        assert_eq!(
            dispatch(&host, "paramValue", &[&gain, "gain"]),
            Ok(json!(0.25))
        );
        for arguments in [
            vec![&gain, "gain", "setValueAtTime", "0.5", "0"],
            vec![&gain, "gain", "linearRampToValueAtTime", "0.75", "0.1"],
            vec![&gain, "gain", "exponentialRampToValueAtTime", "0.5", "0.2"],
            vec![&gain, "gain", "setTargetAtTime", "0.25", "0.3", "0.1"],
            vec![&gain, "gain", "cancelScheduledValues", "0", "0.4"],
        ] {
            assert_eq!(
                dispatch(&host, "paramSchedule", &arguments),
                Ok(Value::Null)
            );
        }

        let buffer_id = 500;
        lock(&host.buffers).insert(
            buffer_id,
            AudioBuffer::from(vec![vec![0.0f32; 16]], OFFLINE_SAMPLE_RATE),
        );
        let buffer_id = buffer_id.to_string();
        assert_eq!(
            dispatch(&host, "sourceBuffer", &[&source, &buffer_id]),
            Ok(Value::Null)
        );
        assert_eq!(
            dispatch(&host, "sourceLoop", &[&source, "1"]),
            Ok(Value::Null)
        );
        assert_eq!(
            dispatch(&host, "sourceStart", &[&source, "0", "0"]),
            Ok(Value::Null)
        );
        assert_eq!(
            dispatch(&host, "sourceStop", &[&source, "0.2"]),
            Ok(Value::Null)
        );

        let record = dispatch(&host, "bufferInfo", &[&buffer_id]).unwrap();
        assert_eq!(record["id"], json!(500));
        assert_eq!(record["length"], json!(16));
        assert_eq!(
            dispatch(&host, "releaseBuffer", &[&buffer_id]),
            Ok(Value::Null)
        );
        assert_eq!(dispatch(&host, "disconnect", &[&gain]), Ok(Value::Null));
        assert_eq!(dispatch(&host, "release", &[&panner]), Ok(Value::Null));
    }

    #[test]
    fn dispatch_preserves_context_and_protocol_errors() {
        let host = AudioHost::new(true, None);
        assert_eq!(
            error(&host, "notAnAudioOperation", &[]),
            "unknown audio operation: notAnAudioOperation"
        );
        assert_eq!(error(&host, "create", &[]), "missing audio argument 0");
        assert_eq!(
            error(&host, "release", &["not-a-number"]),
            "invalid audio argument 0"
        );
        assert_eq!(
            error(&host, "mode", &["impossible"]),
            "unknown audio mode: impossible"
        );
        assert_eq!(
            error(&host, "create", &["oscillator"]),
            "unknown audio node: oscillator"
        );

        dispatch(&host, "context", &[]).unwrap();
        assert_eq!(
            error(&host, "mode", &[]),
            "the audio context is already open"
        );
    }

    #[test]
    fn dispatch_preserves_param_source_and_buffer_error_order() {
        let host = AudioHost::new(true, None);
        let gain = id(&dispatch(&host, "create", &["gain"]).unwrap());

        // `paramSet` validates the assigned value before looking up the node.
        assert_eq!(
            error(&host, "paramSet", &["bad-id", "gain", "bad-value"]),
            "invalid audio argument 2"
        );
        assert_eq!(
            error(&host, "paramValue", &[&gain, "frequency"]),
            "no audio parameter named frequency"
        );
        assert_eq!(
            error(
                &host,
                "paramSchedule",
                &[&gain, "gain", "unknownSchedule", "1", "0"],
            ),
            "unknown parameter schedule: unknownSchedule"
        );
        assert_eq!(
            error(
                &host,
                "paramSchedule",
                &[&gain, "gain", "setTargetAtTime", "1", "0"],
            ),
            "missing audio argument 5"
        );

        // A wrong node kind is diagnosed before source-only arguments are read.
        assert_eq!(
            error(&host, "sourceLoop", &[&gain, "bad-loop"]),
            "only a buffer source loops"
        );
        assert_eq!(
            error(&host, "sourceStart", &[&gain, "bad-when", "bad-offset"]),
            "only a buffer source can be started"
        );
        // Buffer lookup precedes the source-kind check for assignment.
        assert_eq!(
            error(&host, "sourceBuffer", &[&gain, "999"]),
            "the audio buffer has been released"
        );
        assert_eq!(
            error(&host, "bufferInfo", &["999"]),
            "the audio buffer has been released"
        );
    }
}
