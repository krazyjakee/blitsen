// Audio (issue #81), asserted on rendered samples.
//
// The whole point of the offline backend is that this file can say what came
// out, not merely what was called. A graph that was built correctly and
// rendered silence would pass any check that only read properties back — which
// is the same reason the renderer's tests read painted pixels rather than DOM
// state.
//
// `BLITSEN_AUDIO_OFFLINE=1` is set by the harness runner, so no test here opens
// an output device. A machine running these has its sound card left alone.
import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

/** A 16-bit PCM mono WAV of a constant sample value, which peaks predictably. */
const wav = (amplitude, frames, sampleRate = 48_000) => {
  const bytes = new Uint8Array(44 + frames * 2);
  const view = new DataView(bytes.buffer);
  const ascii = (offset, text) => [...text].forEach((c, i) => bytes[offset + i] = c.charCodeAt(0));
  ascii(0, "RIFF");
  view.setUint32(4, 36 + frames * 2, true);
  ascii(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  ascii(36, "data");
  view.setUint32(40, frames * 2, true);
  for (let frame = 0; frame < frames; frame += 1) {
    view.setInt16(44 + frame * 2, Math.round(amplitude * 32767), true);
  }
  return bytes;
};

// Half amplitude, a quarter of the offline render's one second.
const SAMPLE = wav(0.5, 12_000);
const encoded = `new Uint8Array([${SAMPLE.join(",")}])`;

// The surface, and the refusals. Nothing is rendered here — that is the next
// block — so this is the half that can be checked without a render.
const surface = JSON.parse(native.runBridgeHarness(`<div id="audio"></div>`, `{
  const expect = (actual, wanted, what) => {
    const seen = JSON.stringify(actual), meant = JSON.stringify(wanted);
    if (seen !== meant) throw new Error(what + ": " + seen + " is not " + meant);
  };
  const context = new AudioContext();
  expect(typeof context.sampleRate, "number", "a context reports its sample rate");
  expect(context.destination instanceof AudioDestinationNode, true, "the destination is a node");
  expect(context.destination === context.destination, true, "and is the same object each time");

  const gain = context.createGain();
  expect(gain.gain.value, 1, "gain starts at unity");
  gain.gain.value = 0.25;
  expect(gain.gain.value, 0.25, "a parameter reads back what the graph holds");
  expect(gain instanceof GainNode && gain instanceof AudioNode, true, "node interfaces");
  expect(gain.gain instanceof AudioParam, true, "AudioParam");

  const panner = context.createStereoPanner();
  panner.pan.value = -1;
  expect(panner.pan.value, -1, "a panner pans");

  // connect returns its destination, so a chain reads as one expression.
  expect(gain.connect(panner) === panner, true, "connect returns the destination");
  panner.connect(context.destination);

  const source = context.createBufferSource();
  expect(source.loop, false, "a source does not loop by default");
  source.loop = true;
  expect(source.loop, true, "until it is told to");
  source.loop = false;

  // A source plays once: the specification says so, and a game relies on it —
  // overlapping sounds are separate sources over one buffer.
  let restarted = null;
  source.start();
  try { source.start(); } catch (error) { restarted = error.name; }
  expect(restarted, "InvalidStateError", "a source cannot be started twice");

  let unstarted = null;
  try { context.createBufferSource().stop(); } catch (error) { unstarted = error.name; }
  expect(unstarted, "InvalidStateError", "an unstarted source cannot be stopped");

  let badConnect = null;
  try { gain.connect({}); } catch (error) { badConnect = error.constructor.name; }
  expect(badConnect, "TypeError", "a node connects to a node");

  let illegal = null;
  try { new AudioNode(); } catch (error) { illegal = error.constructor.name; }
  expect(illegal, "TypeError", "nodes are not constructible");

  // The element surface, which is the graph above with a tag on it.
  const element = new Audio("cue.wav");
  expect(element instanceof Audio && element instanceof HTMLAudioElement, true, "Audio is an element");
  expect([element.tagName, element.paused, element.volume, element.loop, element.muted],
    ["AUDIO", true, 1, false, false], "an element starts paused at full volume");
  expect(String(element.duration), "NaN", "duration is unknown until something is decoded");
  expect(document.createElement("audio") instanceof HTMLAudioElement, true,
    "a parsed <audio> gets the same interface");
  element.volume = 0.5;
  expect(element.volume, 0.5, "volume reads back");
  let range = null;
  try { element.volume = 2; } catch (error) { range = error.name; }
  expect(range, "IndexSizeError", "volume outside 0..1 is refused");
  expect([element.canPlayType("audio/wav"), element.canPlayType("audio/mpeg"),
    element.canPlayType("video/mp4")], ["probably", "probably", ""],
    "canPlayType answers definitely, for the codecs Symphonia decodes");

  document.getElementById("audio").setAttribute("data-audio", "ok"); }`, 320, 180));
assert.equal(
  surface.nodes.find(node => node.attributes.id === "audio").attributes["data-audio"], "ok");

// Decoding lands at the frame turn, like every other off-thread result, and the
// graph it feeds is then rendered and measured.
const rendered = JSON.parse(native.runBridgeHarness(`<div id="render"></div>`, `{
  const results = globalThis.__blitsenAudio = {};
  const context = new AudioContext();
  context.decodeAudioData(${encoded}.buffer).then(buffer => {
    results.decoded = {
      channels: buffer.numberOfChannels,
      length: buffer.length,
      sampleRate: buffer.sampleRate,
      duration: buffer.duration,
      peak: [...buffer.getChannelData(0)].reduce((peak, s) => Math.max(peak, Math.abs(s)), 0),
      isFloat: buffer.getChannelData(0) instanceof Float32Array,
    };
    // Through a gain of 0.5, so what comes out is a quarter of full scale.
    const gain = context.createGain();
    gain.gain.value = 0.5;
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(gain);
    gain.connect(context.destination);
    source.start();
    results.ready = true;
  }, error => { results.error = error.name + ": " + error.message; });
}`, 320, 180));
assert.ok(rendered);

await Bun.sleep(50);
assert.equal(globalThis.__blitsenAudio.decoded, undefined,
  "a decode waits for the frame turn rather than arriving between them");
assert.equal(globalThis.__blitsenAnimationFramesPending(), true,
  "a decode in flight keeps the host turning so its result can land");

for (let turn = 0; turn < 400 && !globalThis.__blitsenAudio.ready; turn++) {
  globalThis.__blitsenAnimationFrameTick(0);
  await Bun.sleep(5);
}
const { decoded, error, ready } = globalThis.__blitsenAudio;
assert.equal(error, undefined, `decoding failed: ${error}`);
assert.equal(ready, true, "the decode settled within the turns allowed");
assert.equal(decoded.channels, 1, "a mono WAV decodes to one channel");
assert.equal(decoded.sampleRate, 48_000);
assert.equal(decoded.isFloat, true, "channel data is Float32");
assert.ok(Math.abs(decoded.peak - 0.5) < 0.01,
  `the decoded peak is the amplitude that was encoded (got ${decoded.peak})`);
assert.ok(Math.abs(decoded.duration - 0.25) < 0.01,
  `12000 frames at 48 kHz is a quarter second (got ${decoded.duration})`);

// The evidence: render the graph and read the samples it produced.
//
// From out here rather than from another `runBridgeHarness` call, because each
// call installs a fresh bridge with its own audio host — a second one would
// render an empty graph and prove nothing. The harness installs its globals
// into this realm, so this is the same host the graph above was built in.
const measured = JSON.parse(globalThis.__blitsenAudioCall("render"));
assert.equal(measured.sampleRate, 48_000);
assert.equal(measured.length, 48_000, "the offline context renders one second");
const [left, right] = measured.channels;
assert.ok(Math.abs(left.peak - 0.25) < 0.01,
  `0.5 through a gain of 0.5 peaks at 0.25 (got ${left.peak})`);
assert.ok(Math.abs(right.peak - 0.25) < 0.01,
  `and the same on the right channel (got ${right.peak})`);
// Energy separates a real signal from one that merely never peaked: a quarter
// second of 0.25 into 48000 frames is 12000 * 0.0625 = 750.
assert.ok(left.energy > 700 && left.energy < 800,
  `the rendered energy is a quarter second of signal, not a click (got ${left.energy})`);
delete globalThis.__blitsenAudio;

// Refusing to decode something that is not audio, rather than producing silence
// that would be indistinguishable from a working file the user cannot hear.
const refused = JSON.parse(native.runBridgeHarness(`<div id="bad"></div>`, `{
  const results = globalThis.__blitsenBadAudio = {};
  new AudioContext().decodeAudioData(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]).buffer)
    .then(() => { results.outcome = "resolved"; },
      error => { results.outcome = error.name; results.message = error.message; });
}`, 320, 180));
assert.ok(refused);
for (let turn = 0; turn < 400 && !globalThis.__blitsenBadAudio.outcome; turn++) {
  globalThis.__blitsenAnimationFrameTick(0);
  await Bun.sleep(5);
}
assert.equal(globalThis.__blitsenBadAudio.outcome, "EncodingError",
  "bytes that are not audio are refused, not silently decoded to silence");
assert.match(globalThis.__blitsenBadAudio.message, /could not decode/);
delete globalThis.__blitsenBadAudio;

// The path an application actually takes: a `<audio src>` naming a file the
// export shipped, loaded from disk.
//
// This is not a variation on the tests above — it is a different loader. `fetch`
// is http(s) only and refuses a local file, so an element whose source is a
// bundled asset would have been dead on arrival if it went through `fetch`, and
// it did until this was checked. That is why the document below is a real one
// on disk rather than a bridge-harness string: only a document loaded from a
// directory has a base for a relative source to resolve against.
{
  const { mkdtemp, writeFile, rm } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  const directory = await mkdtemp(join(tmpdir(), "blitsen-audio-"));
  try {
    await writeFile(join(directory, "cue.wav"), wav(0.5, 6_000));
    await writeFile(join(directory, "index.html"), `<!DOCTYPE html><html><body>
      <div id="probe"></div>
      <script type="module">
        // A real context with a real clock and no output device. The harness
        // renders offline everywhere else, and an offline context has no clock
        // at all — so it is the one place a sound cannot actually finish, and the
        // end of one could not be observed in it.
        __blitsenAudioCall("mode", "silent");
        globalThis.__blitsenMedia = { events: [] };
        const results = globalThis.__blitsenMedia;
        const element = new Audio("cue.wav");
        for (const type of ["loadedmetadata", "canplaythrough", "play", "error", "ended"])
          element.addEventListener(type, () => results.events.push(type));
        // What the element looks like once the sound has finished, which is the
        // half that was missing: the end is announced from the render thread,
        // and without it an element stays "playing" forever and refuses to be
        // played again.
        element.addEventListener("ended", () => {
          results.afterEnd = { paused: element.paused, ended: element.ended,
            currentTime: element.currentTime };
          if (!results.replayed) element.play().then(() => { results.replayed = true; });
          else { element.pause(); results.quiet = true; }
        });
        element.volume = 0.4;
        element.play().then(() => {
          results.duration = element.duration;
          results.paused = element.paused;
          results.currentTimeMoved = element.currentTime >= 0;
          element.pause();
          results.pausedAfter = element.paused;
          results.done = true;
          // Play it through to the end, so there is an ending to announce.
          element.currentTime = 0;
          element.play();
        }, error => { results.error = error.name + ": " + error.message; results.done = true; });

        // A source that is not there must fail, not hang: an application waiting
        // forever on a missing cue is worse than one told it is missing.
        const missing = new Audio("absent.wav");
        missing.play().then(() => { results.missing = "resolved"; },
          error => { results.missing = error.name; });
      </script></body></html>`);

    native.runDocumentScriptsHarness(join(directory, "index.html"), 320, 180);
    for (let turn = 0; turn < 600 && !globalThis.__blitsenMedia?.done; turn++) {
      globalThis.__blitsenAnimationFrameTick(0);
      await Bun.sleep(5);
    }
    const media = globalThis.__blitsenMedia;
    assert.equal(media.error, undefined, `the element failed to load: ${media.error}`);
    assert.ok(Math.abs(media.duration - 0.125) < 0.01,
      `6000 frames at 48 kHz is an eighth of a second (got ${media.duration})`);
    assert.equal(media.paused, false, "the element is playing once play() resolves");
    assert.equal(media.pausedAfter, true, "and paused once it is paused");
    assert.deepEqual(media.events.slice(0, 3), ["loadedmetadata", "canplaythrough", "play"],
      "the element reports what it did, in order");

    // A source plays once and says so. Nothing dispatched `ended` at first —
    // the render thread announces it and the frame turn delivers it — so an
    // element stayed "playing" forever and would not play a second time.
    for (let turn = 0; turn < 600 && !media.replayed; turn++) {
      globalThis.__blitsenAnimationFrameTick(0);
      await Bun.sleep(5);
    }
    assert.ok(media.events.includes("ended"), "a sound that finished says so");
    assert.deepEqual(media.afterEnd, { paused: true, ended: true, currentTime: 0 },
      "and leaves the element paused, ended and rewound");
    assert.equal(media.replayed, true, "so the same element can be played again");

    // Leave nothing playing. The harness is one linear pass over one realm, and
    // a sound still going is a host that never stops asking for frames — which
    // the next section reads as its own work outstanding.
    globalThis.__blitsenMedia.stop = true;
    for (let turn = 0; turn < 600 && globalThis.__blitsenAudioPending(); turn++) {
      globalThis.__blitsenAnimationFrameTick(0);
      await Bun.sleep(5);
    }
    assert.equal(globalThis.__blitsenAudioPending(), false,
      "a finished sound stops asking for frames");

    for (let turn = 0; turn < 600 && !media.missing; turn++) {
      globalThis.__blitsenAnimationFrameTick(0);
      await Bun.sleep(5);
    }
    assert.equal(media.missing, "EncodingError",
      "a source that is not there rejects rather than hanging");
    delete globalThis.__blitsenMedia;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
