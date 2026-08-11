//! Recorded input traces and per-frame output digests for deterministic replay.
//!
//! A trace pins everything a replay needs to be reproducible: the viewport, the
//! frame count, the fixed timestep handed to animation callbacks, and the
//! synthetic input delivered at the bridge boundary. Replaying one produces a
//! digest per frame; comparing digest sequences is the regression net.

use serde::{Deserialize, Serialize};

/// Digest length in bytes. 128 bits is far past collision risk for a per-commit
/// sequence and keeps a committed golden file readable.
const DIGEST_BYTES: usize = 16;

/// Rejected trace document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceError(String);

impl TraceError {
    /// Human-readable reason the trace was rejected.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TraceError {}

/// One synthetic input event delivered before a frame's animation callbacks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TraceInput {
    /// Keyboard event routed to the document's focused element.
    Key {
        /// DOM event type, `keydown` or `keyup`.
        #[serde(rename = "type")]
        event_type: String,
        /// `KeyboardEvent.key`.
        key: String,
        /// `KeyboardEvent.code`.
        code: String,
        /// `KeyboardEvent.repeat`.
        #[serde(default)]
        repeat: bool,
    },
    /// Pointer event hit-tested at viewport coordinates, as the window does.
    Pointer {
        /// DOM event type, one of [`POINTER_EVENT_TYPES`].
        #[serde(rename = "type")]
        event_type: String,
        /// Viewport x in CSS pixels.
        x: f64,
        /// Viewport y in CSS pixels.
        y: f64,
    },
}

/// Keyboard event types a trace may dispatch.
pub const KEY_EVENT_TYPES: [&str; 2] = ["keydown", "keyup"];
/// Pointer event types a trace may dispatch.
pub const POINTER_EVENT_TYPES: [&str; 5] = ["mousedown", "mouseup", "click", "mousemove", "wheel"];

/// One input scheduled at a frame index.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceEvent {
    /// One-based frame the input is delivered in.
    pub frame: u32,
    /// The input itself.
    #[serde(flatten)]
    pub input: TraceInput,
}

/// A replayable recording of one application run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputTrace {
    /// Format version. Only `1` is accepted.
    pub version: u32,
    /// Application path relative to the repository root, for diagnostics.
    pub application: String,
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
    /// Number of frames to replay.
    pub frames: u32,
    /// Fixed timestep between animation callbacks, in milliseconds.
    pub frame_duration_ms: f64,
    /// Leading frames excluded from the steady-state timing summary. They pay
    /// for first-use caches — font shaping, style rule matching — that a running
    /// application pays once.
    #[serde(default)]
    pub warmup_frames: u32,
    /// Inputs in delivery order.
    pub events: Vec<TraceEvent>,
}

impl InputTrace {
    /// Parses and validates a serialized trace.
    pub fn from_json(source: &str) -> Result<Self, TraceError> {
        let trace: Self =
            serde_json::from_str(source).map_err(|error| TraceError(error.to_string()))?;
        trace.validate()?;
        Ok(trace)
    }

    /// Serializes the trace in the committed format.
    pub fn to_json(&self) -> Result<String, TraceError> {
        serde_json::to_string_pretty(self).map_err(|error| TraceError(error.to_string()))
    }

    /// The deterministic timestamp handed to frame `frame`, one-based.
    pub fn timestamp_ms(&self, frame: u32) -> f64 {
        f64::from(frame) * self.frame_duration_ms
    }

    /// Inputs scheduled for frame `frame`, in recorded order.
    pub fn inputs_for_frame(&self, frame: u32) -> impl Iterator<Item = &TraceInput> {
        self.events
            .iter()
            .filter(move |event| event.frame == frame)
            .map(|event| &event.input)
    }

    fn validate(&self) -> Result<(), TraceError> {
        if self.version != 1 {
            return Err(TraceError(format!(
                "unsupported input trace version {}",
                self.version
            )));
        }
        if self.frames == 0 || self.frames > 10_000 {
            return Err(TraceError(format!(
                "input trace frame count {} is outside 1..=10000",
                self.frames
            )));
        }
        if !(self.frame_duration_ms.is_finite() && self.frame_duration_ms > 0.0) {
            return Err(TraceError(
                "input trace needs a positive fixed timestep".into(),
            ));
        }
        if self.warmup_frames >= self.frames {
            return Err(TraceError(format!(
                "input trace warm-up of {} frames leaves nothing to measure",
                self.warmup_frames
            )));
        }
        if self.width == 0 || self.height == 0 {
            return Err(TraceError("input trace viewport is empty".into()));
        }
        let mut previous = 0;
        for event in &self.events {
            if event.frame == 0 || event.frame > self.frames {
                return Err(TraceError(format!(
                    "input trace event at frame {} is outside 1..={}",
                    event.frame, self.frames
                )));
            }
            if event.frame < previous {
                return Err(TraceError(
                    "input trace events must be recorded in frame order".into(),
                ));
            }
            previous = event.frame;
            event.input.validate()?;
        }
        Ok(())
    }
}

impl TraceInput {
    /// The DOM event type this input dispatches.
    pub fn event_type(&self) -> &str {
        match self {
            Self::Key { event_type, .. } | Self::Pointer { event_type, .. } => event_type,
        }
    }

    fn validate(&self) -> Result<(), TraceError> {
        let (event_type, allowed) = match self {
            Self::Key { event_type, .. } => (event_type, &KEY_EVENT_TYPES[..]),
            Self::Pointer { event_type, .. } => (event_type, &POINTER_EVENT_TYPES[..]),
        };
        if !allowed.contains(&event_type.as_str()) {
            return Err(TraceError(format!(
                "input trace event type {event_type:?} is not one of {allowed:?}"
            )));
        }
        if let Self::Pointer { x, y, .. } = self
            && !(x.is_finite() && y.is_finite())
        {
            return Err(TraceError("pointer input needs finite coordinates".into()));
        }
        Ok(())
    }
}

/// Incremental digest of one frame's observable output.
///
/// Every field is length-prefixed, so no two different frames can hash the same
/// bytes by running their fields together.
#[derive(Clone, Debug)]
pub struct FrameDigest(blake3::Hasher);

impl FrameDigest {
    /// Starts a digest in a domain of its own, so a DOM digest and a pixel
    /// digest of the same bytes never collide.
    pub fn new(domain: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(domain.as_bytes());
        hasher.update(&[0xff]);
        Self(hasher)
    }

    /// Adds a length-prefixed text field.
    pub fn field(&mut self, value: &str) -> &mut Self {
        self.bytes(value.as_bytes())
    }

    /// Adds a length-prefixed byte field.
    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.0.update(&(value.len() as u64).to_le_bytes());
        self.0.update(value);
        self
    }

    /// Adds a float quantized to 1/1024 px, the resolution layout is compared at.
    pub fn number(&mut self, value: f64) -> &mut Self {
        let quantized = if value.is_finite() {
            (value * 1024.0).round() as i64
        } else {
            i64::MIN
        };
        self.0.update(&quantized.to_le_bytes());
        self
    }

    /// Finishes the digest as lowercase hexadecimal.
    pub fn finish(&self) -> String {
        let hash = self.0.finalize();
        hash.as_bytes()[..DIGEST_BYTES].iter().fold(
            String::with_capacity(DIGEST_BYTES * 2),
            |mut text, byte| {
                use std::fmt::Write as _;
                let _ = write!(text, "{byte:02x}");
                text
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(events: &str) -> String {
        format!(
            r#"{{ "version": 1, "application": "examples/pong", "width": 8, "height": 6,
                  "frames": 4, "frameDurationMs": 16.666666666666668, "events": [{events}] }}"#
        )
    }

    #[test]
    fn traces_round_trip_and_schedule_inputs_by_frame() {
        let source = trace(
            r#"{ "frame": 2, "kind": "key", "type": "keydown", "key": " ", "code": "Space" },
               { "frame": 2, "kind": "pointer", "type": "click", "x": 4.0, "y": 3.0 }"#,
        );
        let parsed = InputTrace::from_json(&source).unwrap();
        assert_eq!(parsed.inputs_for_frame(1).count(), 0);
        assert_eq!(
            parsed
                .inputs_for_frame(2)
                .map(TraceInput::event_type)
                .collect::<Vec<_>>(),
            ["keydown", "click"]
        );
        assert_eq!(parsed.timestamp_ms(3), 50.0);
        assert_eq!(
            InputTrace::from_json(&parsed.to_json().unwrap()).unwrap(),
            parsed
        );
    }

    #[test]
    fn invalid_traces_are_rejected_before_anything_is_dispatched() {
        for (source, expected) in [
            (
                trace(
                    r#"{ "frame": 5, "kind": "key", "type": "keydown", "key": "w", "code": "KeyW" }"#,
                ),
                "outside 1..=4",
            ),
            (
                trace(
                    r#"{ "frame": 2, "kind": "key", "type": "keypress", "key": "w", "code": "KeyW" }"#,
                ),
                "not one of",
            ),
            (
                trace(
                    r#"{ "frame": 3, "kind": "pointer", "type": "click", "x": 1.0, "y": 1.0 },
                       { "frame": 1, "kind": "pointer", "type": "click", "x": 1.0, "y": 1.0 }"#,
                ),
                "frame order",
            ),
            (
                trace("").replace("\"version\": 1", "\"version\": 2"),
                "unsupported input trace version",
            ),
            (
                trace("").replace("\"frames\": 4", "\"frames\": 0"),
                "outside 1..=10000",
            ),
        ] {
            let error = InputTrace::from_json(&source).unwrap_err();
            assert!(error.message().contains(expected), "{error}");
        }
    }

    #[test]
    fn digest_fields_cannot_run_together() {
        let digest = |left: &str, right: &str| {
            let mut digest = FrameDigest::new("dom");
            digest.field(left).field(right);
            digest.finish()
        };
        assert_ne!(digest("ab", "c"), digest("a", "bc"));
        assert_eq!(digest("ab", "c"), digest("ab", "c"));

        let mut domain = FrameDigest::new("pixels");
        domain.field("ab").field("c");
        assert_ne!(domain.finish(), digest("ab", "c"));

        let mut quantized = FrameDigest::new("dom");
        quantized.number(1.0);
        let mut under_resolution = FrameDigest::new("dom");
        under_resolution.number(1.000_1);
        assert_eq!(quantized.finish(), under_resolution.finish());
        let mut visible = FrameDigest::new("dom");
        visible.number(1.01);
        assert_ne!(quantized.finish(), visible.finish());
    }
}
