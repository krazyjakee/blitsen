  // Audio (issue #81). The graph is `web-audio-api`'s; these are the names for
  // it. What is here is what the issue asked for — a context, gain, stereo
  // panning, and buffer sources for one-shot playback — plus the `Audio`
  // element built on those. Everything else in Web Audio stays absent rather
  // than half-built, and COMPATIBILITY.md lists it.
  const audio = (operation, ...args) =>
    JSON.parse(__blitsenAudioCall(operation, ...args.map(value => String(value))));

  const audioStates = new WeakMap();
  const audioStateFor = object => {
    const state = audioStates.get(object);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };
  // The destination is addressed as node 0: it belongs to the context rather
  // than to the node registry, it is never created and never released, and
  // there is exactly one of it.
  const DESTINATION = 0;

  const audioNumber = (value, what) => {
    const number = Number(value);
    if (!Number.isFinite(number)) throw new TypeError(`${what} must be a finite number`);
    return number;
  };

  // A parameter is addressed by its node and its name rather than held as an
  // object of its own, so a value read back is the graph's and not a copy that
  // could have drifted from it.
  class AudioParam {
    constructor() { throw new TypeError("Illegal constructor"); }
    get value() { const { node, name } = audioStateFor(this); return audio("paramValue", node, name); }
    set value(value) {
      const { node, name } = audioStateFor(this);
      audio("paramSet", node, name, audioNumber(value, "an audio parameter"));
    }
    setValueAtTime(value, when) { return this._schedule("setValueAtTime", value, when); }
    linearRampToValueAtTime(value, when) { return this._schedule("linearRampToValueAtTime", value, when); }
    exponentialRampToValueAtTime(value, when) {
      return this._schedule("exponentialRampToValueAtTime", value, when);
    }
    setTargetAtTime(target, when, timeConstant) {
      return this._schedule("setTargetAtTime", target, when, timeConstant);
    }
    cancelScheduledValues(when) { return this._schedule("cancelScheduledValues", 0, when); }
    _schedule(kind, value, when, extra = 0) {
      const { node, name } = audioStateFor(this);
      audio("paramSchedule", node, name, kind,
        audioNumber(value, "an audio parameter"), audioNumber(when, "a schedule time"),
        audioNumber(extra, "a time constant"));
      return this;
    }
  }

  const audioParam = (node, name) => {
    const param = Object.create(AudioParam.prototype);
    audioStates.set(param, { node, name });
    return param;
  };

  class AudioNode {
    constructor() { throw new TypeError("Illegal constructor"); }
    get context() { return audioStateFor(this).context; }
    // Returns the destination, as the specification does, so connections chain.
    connect(destination) {
      if (!(destination instanceof AudioNode)) {
        throw new TypeError("an audio node connects to another audio node");
      }
      audio("connect", audioStateFor(this).id, audioStateFor(destination).id);
      return destination;
    }
    disconnect() { audio("disconnect", audioStateFor(this).id); }
  }

  class AudioDestinationNode extends AudioNode {}

  class GainNode extends AudioNode {
    get gain() { return audioStateFor(this).gain; }
  }

  class StereoPannerNode extends AudioNode {
    get pan() { return audioStateFor(this).pan; }
  }

  // A decoded buffer. The samples stay in Rust until asked for: `getChannelData`
  // copies one channel across the boundary, which is the only shape a caller
  // reads them in.
  class AudioBuffer {
    constructor() { throw new TypeError("Illegal constructor"); }
    get sampleRate() { return audioStateFor(this).sampleRate; }
    get length() { return audioStateFor(this).length; }
    get duration() { return audioStateFor(this).duration; }
    get numberOfChannels() { return audioStateFor(this).numberOfChannels; }
    getChannelData(channel) {
      const { id, numberOfChannels } = audioStateFor(this);
      const index = Number(channel) | 0;
      if (index < 0 || index >= numberOfChannels) {
        throw new DOMException("the audio buffer has no such channel", "IndexSizeError");
      }
      return __blitsenAudioChannel(String(id), String(index));
    }
  }

  const audioBuffer = record => {
    const buffer = Object.create(AudioBuffer.prototype);
    audioStates.set(buffer, record);
    return buffer;
  };

  // One-shot by construction, which is what the specification says and what a
  // game wants: a source plays once and is thrown away, so overlapping sounds
  // are separate sources over one decoded buffer rather than one source
  // restarted.
  class AudioBufferSourceNode extends AudioNode {
    get buffer() { return audioStateFor(this).buffer ?? null; }
    set buffer(value) {
      const state = audioStateFor(this);
      if (value === null) return;
      if (!(value instanceof AudioBuffer)) throw new TypeError("buffer must be an AudioBuffer");
      state.buffer = value;
      audio("sourceBuffer", state.id, audioStateFor(value).id);
    }
    get loop() { return audioStateFor(this).loop; }
    set loop(value) {
      const state = audioStateFor(this);
      state.loop = Boolean(value);
      audio("sourceLoop", state.id, state.loop ? 1 : 0);
    }
    get playbackRate() { return audioStateFor(this).playbackRate; }
    get detune() { return audioStateFor(this).detune; }
    get onended() { return audioStateFor(this).onended ?? null; }
    set onended(callback) {
      const state = audioStateFor(this);
      if (state.onended) this.removeEventListener("ended", state.onended);
      state.onended = typeof callback === "function" ? callback : null;
      if (state.onended) this.addEventListener("ended", state.onended);
    }
    start(when = 0, offset = 0) {
      const state = audioStateFor(this);
      if (state.started) {
        throw new DOMException("this source has already been started", "InvalidStateError");
      }
      state.started = true;
      liveSources.set(state.id, this);
      audio("sourceStart", state.id,
        audioNumber(when, "a start time"), audioNumber(offset, "an offset"));
    }
    stop(when = 0) {
      const state = audioStateFor(this);
      if (!state.started) {
        throw new DOMException("this source has not been started", "InvalidStateError");
      }
      audio("sourceStop", state.id, audioNumber(when, "a stop time"));
    }
  }

  // `ended` is an EventTarget event, so a source is one. The base class cannot
  // be EventTarget for every node — only a source fires anything — so the
  // listener surface is mixed in here.
  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"]) {
    Object.defineProperty(AudioBufferSourceNode.prototype, method, {
      value: EventTarget.prototype[method], writable: true, configurable: true,
    });
  }

  const decodeJobs = new Map();
  // Started sources, by the id the runtime knows them under, so an `ended`
  // arriving from the render thread can find the wrapper it belongs to. An
  // entry is dropped when it ends: a source plays once, so nothing reaches it
  // afterwards and keeping it would leak for an application firing one-shots.
  const liveSources = new Map();

  class AudioContext {
    constructor(options = {}) {
      const state = audio("context");
      audioStates.set(this, state);
      if (options.sampleRate !== undefined && Number(options.sampleRate) !== state.sampleRate) {
        // The device decides the rate here; a context cannot be opened at an
        // arbitrary one, so asking is refused rather than silently ignored.
        throw new DOMException(
          `this audio device runs at ${state.sampleRate} Hz, not ${options.sampleRate}`,
          "NotSupportedError");
      }
    }
    get sampleRate() { return audioStateFor(this).sampleRate; }
    // Read through rather than cached: the clock moves whether or not anything
    // asked it to.
    get currentTime() { return audio("context").currentTime; }
    get state() { return audio("context").state; }
    get destination() {
      const state = audioStateFor(this);
      if (!state.destination) {
        state.destination = Object.create(AudioDestinationNode.prototype);
        audioStates.set(state.destination, { id: DESTINATION, context: this });
      }
      return state.destination;
    }
    createGain() {
      const id = audio("create", "gain");
      const node = Object.create(GainNode.prototype);
      audioStates.set(node, { id, context: this, gain: audioParam(id, "gain") });
      return node;
    }
    createStereoPanner() {
      const id = audio("create", "panner");
      const node = Object.create(StereoPannerNode.prototype);
      audioStates.set(node, { id, context: this, pan: audioParam(id, "pan") });
      return node;
    }
    createBufferSource() {
      const id = audio("create", "source");
      const node = Object.create(AudioBufferSourceNode.prototype);
      audioStates.set(node, { id, context: this, buffer: null, loop: false, started: false,
        playbackRate: audioParam(id, "playbackRate"), detune: audioParam(id, "detune") });
      return node;
    }
    // Decoding runs on the worker pool and lands at the frame turn, exactly
    // where a fetch result does. The callback forms are the pre-promise
    // spelling, still what a good deal of shipped audio code uses.
    decodeAudioData(data, onSuccess, onError) {
      const promise = new Promise((resolve, reject) => {
        let bytes;
        try {
          bytes = data instanceof ArrayBuffer ? new Uint8Array(data)
            : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
            : null;
          if (!bytes) throw new TypeError("decodeAudioData takes an ArrayBuffer");
        } catch (error) { reject(error); return; }
        decodeJobs.set(__blitsenAudioDecode(bytes), { resolve, reject });
      });
      if (typeof onSuccess === "function" || typeof onError === "function") {
        promise.then(
          buffer => { if (typeof onSuccess === "function") onSuccess(buffer); },
          error => { if (typeof onError === "function") onError(error); });
      }
      return promise;
    }
    resume() { return Promise.resolve(audio("resume")).then(() => undefined); }
    suspend() { return Promise.resolve(audio("suspend")).then(() => undefined); }
    close() { return Promise.resolve(audio("close")).then(() => undefined); }
  }

  // The one handoff point for audio work, beside the network's. A source that
  // finished playing is announced from the render thread and dispatched here,
  // so an `ended` handler runs where every other off-thread result does rather
  // than on a thread that does not own the DOM.
  const settleAudio = () => {
    if (decodeJobs.size === 0 && liveSources.size === 0) return;
    const polled = JSON.parse(__blitsenAudioPoll());
    for (const record of polled.decoded) {
      const job = decodeJobs.get(record.id);
      if (!job) continue;
      decodeJobs.delete(record.id);
      if (record.error) {
        job.reject(new DOMException(`could not decode the audio data: ${record.error}`,
          "EncodingError"));
      } else job.resolve(audioBuffer(record.buffer));
    }
    for (const id of polled.ended) {
      const source = liveSources.get(id);
      if (!source) continue;
      liveSources.delete(id);
      source.dispatchEvent(new Event("ended"));
    }
  };
  // The runtime counts what is playing, not this map: a source is only in the
  // map so an `ended` can find its wrapper, and a context that has been
  // replaced must not go on asking for frames on behalf of sounds nothing can
  // hear any more.
  const audioPending = () => decodeJobs.size > 0 || __blitsenAudioPending();

  // `Audio` and `<audio>`: the element surface, built on the graph above rather
  // than beside it, so one mixer governs everything the application plays.
  //
  // What it is not is a streaming media element. The source is fetched whole and
  // decoded whole before playback starts, which is right for the sounds a
  // desktop application has — effects, cues, a short loop — and wrong for an
  // hour of audio. `buffered`, `seekable`, `readyState` and the rest of the
  // streaming surface are absent rather than answered with a fiction.
  const mediaContext = () => {
    if (!mediaContext.instance) mediaContext.instance = new AudioContext();
    return mediaContext.instance;
  };

  class HTMLAudioElement extends Element {
    static [Symbol.hasInstance](value) {
      return value instanceof Element && elementTag(value) === "audio";
    }
  }

  const mediaElementStates = new WeakMap();
  const mediaElementState = element => {
    let state = mediaElementStates.get(element);
    if (!state) {
      state = { buffer: null, source: null, gain: null, startedAt: 0, offset: 0,
        paused: true, ended: false, volume: 1, muted: false, loop: false, loading: null };
      mediaElementStates.set(element, state);
    }
    return state;
  };

  const mediaGain = state => {
    if (!state.gain) {
      state.gain = mediaContext().createGain();
      state.gain.connect(mediaContext().destination);
    }
    return state.gain;
  };

  // Where a media source actually resolves against.
  //
  // Not `location.href`: JavaScript sees `blitsen://app/`, while the files the
  // application shipped sit in the directory the document was loaded from —
  // which is the base the renderer already resolves images and fonts against.
  // A source is loaded by the runtime rather than by `fetch` for the same
  // reason: `fetch` is http(s) only and says so, and an application's sounds
  // are files it shipped.
  const mediaSourceUrl = source => {
    const base = call("documentBase");
    if (!base) return resolveAgainstDocument(source).href;
    return call("resolveUrl", base, String(source)).href;
  };

  // Loads and decodes the element's source once, and hands back the same
  // promise for every later play of the same address.
  const loadMedia = element => {
    const state = mediaElementState(element);
    const source = element.getAttribute("src");
    if (!source) {
      return Promise.reject(new DOMException("the element has no source", "NotSupportedError"));
    }
    const resolved = mediaSourceUrl(source);
    if (state.loading && state.loaded === resolved) return state.loading;
    state.loaded = resolved;
    state.loading = new Promise((resolve, reject) => {
      let id;
      try { id = __blitsenAudioLoad(resolved); }
      catch (error) { reject(error); return; }
      decodeJobs.set(id, { resolve, reject });
    })
      .then(buffer => {
        state.buffer = buffer;
        element.dispatchEvent(new Event("loadedmetadata"));
        element.dispatchEvent(new Event("canplaythrough"));
        return buffer;
      })
      .catch(error => {
        state.loading = null;
        element.dispatchEvent(new Event("error"));
        throw error;
      });
    return state.loading;
  };

  // A source plays once, so every resume builds a new one over the same decoded
  // buffer — which is also what makes overlapping playback of one file free.
  const startMedia = element => {
    const state = mediaElementState(element);
    const context = mediaContext();
    const source = context.createBufferSource();
    source.buffer = state.buffer;
    source.loop = state.loop;
    source.connect(mediaGain(state));
    mediaGain(state).gain.value = state.muted ? 0 : state.volume;
    source.addEventListener("ended", () => {
      if (state.source !== source || state.paused) return;
      state.ended = true;
      state.paused = true;
      state.offset = 0;
      element.dispatchEvent(new Event("ended"));
    });
    state.source = source;
    state.startedAt = context.currentTime;
    state.paused = false;
    state.ended = false;
    source.start(0, state.offset);
  };

  const mediaCurrentTime = state => {
    if (state.paused || !state.source) return state.offset;
    const elapsed = mediaContext().currentTime - state.startedAt + state.offset;
    if (!state.buffer) return elapsed;
    return state.loop ? elapsed % state.buffer.duration : Math.min(elapsed, state.buffer.duration);
  };

  const audioElementSurface = {
    play() {
      const state = mediaElementState(this);
      if (!state.paused) return Promise.resolve();
      return loadMedia(this).then(() => {
        startMedia(this);
        this.dispatchEvent(new Event("play"));
        this.dispatchEvent(new Event("playing"));
      });
    },
    pause() {
      const state = mediaElementState(this);
      if (state.paused) return;
      state.offset = mediaCurrentTime(state);
      state.paused = true;
      try { state.source?.stop(0); } catch {}
      state.source = null;
      this.dispatchEvent(new Event("pause"));
    },
    load() { mediaElementStates.delete(this); },
    // Only the codecs Symphonia decodes, and only as a definite answer: the
    // specification allows "maybe", which tells a caller nothing.
    canPlayType(type) {
      return /^audio\/(mpeg|mp3|ogg|vorbis|wav|wave|x-wav|flac|x-flac|aac|mp4|webm)\b/i
        .test(String(type)) ? "probably" : "";
    },
  };

  const audioElementProperties = {
    src: {
      get() {
        const value = this.getAttribute("src");
        return value === null ? "" : resolveAgainstDocument(value).href;
      },
      set(value) { this.setAttribute("src", value); mediaElementStates.delete(this); },
    },
    currentTime: {
      get() { return mediaCurrentTime(mediaElementState(this)); },
      set(value) {
        const state = mediaElementState(this);
        const seconds = Math.max(0, Number(value) || 0);
        if (state.paused) { state.offset = seconds; return; }
        this.pause();
        state.offset = seconds;
        this.play();
      },
    },
    duration: {
      get() { const { buffer } = mediaElementState(this); return buffer ? buffer.duration : NaN; },
    },
    paused: { get() { return mediaElementState(this).paused; } },
    ended: { get() { return mediaElementState(this).ended; } },
    volume: {
      get() { return mediaElementState(this).volume; },
      set(value) {
        const volume = Number(value);
        if (!(volume >= 0 && volume <= 1)) {
          throw new DOMException("volume is between 0 and 1", "IndexSizeError");
        }
        const state = mediaElementState(this);
        state.volume = volume;
        if (state.gain) state.gain.gain.value = state.muted ? 0 : volume;
        this.dispatchEvent(new Event("volumechange"));
      },
    },
    muted: {
      get() { return mediaElementState(this).muted; },
      set(value) {
        const state = mediaElementState(this);
        state.muted = Boolean(value);
        if (state.gain) state.gain.gain.value = state.muted ? 0 : state.volume;
        this.dispatchEvent(new Event("volumechange"));
      },
    },
    loop: {
      get() { return mediaElementState(this).loop; },
      set(value) {
        const state = mediaElementState(this);
        state.loop = Boolean(value);
        if (state.source) state.source.loop = state.loop;
      },
    },
    autoplay: {
      get() { return this.hasAttribute("autoplay"); },
      set(value) { this.toggleAttribute("autoplay", Boolean(value)); },
    },
  };

  // Installed on Element rather than on a subclass because the wrapper for an
  // `<audio>` element is retyped by tag, and these must exist on whichever
  // interface that lands on. Guarded by tag so a `<div>` does not answer them.
  const installAudioElementSurface = target => {
    for (const [name, method] of Object.entries(audioElementSurface)) {
      Object.defineProperty(target, name, { value: method, writable: true, configurable: true });
    }
    for (const [name, descriptor] of Object.entries(audioElementProperties)) {
      Object.defineProperty(target, name, { ...descriptor, configurable: true });
    }
  };

  class BlitsenAudioElement extends Element {}
  installAudioElementSurface(BlitsenAudioElement.prototype);

  class Audio {
    constructor(source) {
      const element = document.createElement("audio");
      if (source !== undefined) element.setAttribute("src", String(source));
      return element;
    }
    static [Symbol.hasInstance](value) { return value instanceof HTMLAudioElement; }
  }

  // Registered after the fact rather than in the literal: `TAG_INTERFACES` is
  // built with the document, before this fragment has declared its class. The
  // wrapper reads the table when it retypes a node, so a late entry still takes
  // effect for every `<audio>` element — including those already in the tree.
  TAG_INTERFACES.audio = BlitsenAudioElement;
