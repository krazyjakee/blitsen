// A todo list, animated the way this runtime animates.
//
// Two mechanisms carry every movement, and which one a thing uses depends on
// whether the value is known ahead of time. A state change with a fixed start
// and end — a row arriving, a tick drawing, a strike crossing a label — is a
// @keyframes animation switched on by a class. A value that follows the data —
// the progress fill, the filter thumb — is tweened here, frame by frame,
// because there is nothing behind a plain declaration that would travel between
// two cascaded values.
//
// One rule shapes the rest: a keyframe animation's end state is not kept once
// the cascade is resolved again, so anything that has to *stay* is declared in
// the stylesheet and the animation only covers the journey to it. The classes
// below are named for that split — `done` is the state, `playing` is the run.

const list = document.getElementById("list");
const entry = document.getElementById("entry");
const ghost = document.getElementById("ghost");
const compose = document.querySelector(".compose");
const addButton = document.getElementById("add");
const filters = document.getElementById("filters");
const thumb = document.getElementById("thumb");
const bar = document.getElementById("bar");
const remaining = document.getElementById("remaining");
const tallyWord = document.getElementById("tally-word");
const finished = document.getElementById("finished");
const empty = document.getElementById("empty");
const emptyText = document.getElementById("empty-text");
const clear = document.getElementById("clear");

const ENTER_MS = 460;
const EXIT_MS = 340;
const FLIP_MS = 480;

const state = {
  filter: "all",
  nextId: 1,
  items: [
    { id: 0, text: "Read the compatibility profile", done: true },
    { id: 0, text: "Draw a checkmark without an SVG", done: false },
    { id: 0, text: "Ship a window, not a browser", done: false },
  ],
};
for (const item of state.items) item.id = state.nextId++;

const nodes = new Map();    // live rows, by item id
const leaving = new Map();  // rows on their way out, by item id
const timers = new Map();   // the removal each one is waiting on
let lastRemaining = null;

/* ── Script-driven tweens ──────────────────────────────────────────────────── */

// One frame callback drives every continuous value. Each spring eases towards a
// target, so a target that moves mid-flight is followed rather than restarted —
// which is what a run of quick clicks across the filters looks like.
const springs = [];
const spring = (rate, apply) => {
  const self = { value: null, target: 0, rate, apply };
  springs.push(self);
  return self;
};

const progress = spring(0.12, value => { bar.style.width = `${value}%`; });
const thumbLeft = spring(0.22, value => { thumb.style.left = `${value}px`; });
const thumbWidth = spring(0.22, value => { thumb.style.width = `${value}px`; });

const frame = () => {
  for (const self of springs) {
    if (self.value === null) self.value = self.target;
    else self.value += (self.target - self.value) * self.rate;
    if (Math.abs(self.target - self.value) < 0.05) self.value = self.target;
    self.apply(self.value);
  }
  requestAnimationFrame(frame);
};

// Restarting an animation means taking the class off, settling the style, and
// putting it back; reading a layout box is what settles it.
const replay = (element, name, ms) => {
  element.classList.remove(name);
  void element.offsetWidth;
  element.classList.add(name);
  setTimeout(() => element.classList.remove(name), ms);
};

/* ── Rows ──────────────────────────────────────────────────────────────────── */

const build = item => {
  const row = document.createElement("li");
  row.className = "item entering";
  row.setAttribute("data-done", String(item.done));
  if (item.done) row.classList.add("done");

  const box = document.createElement("span");
  box.className = "box";
  box.appendChild(Object.assign(document.createElement("span"), { className: "tick" }));

  const label = document.createElement("span");
  label.className = "label";
  const wrap = document.createElement("span");
  wrap.className = "wrap";
  const text = document.createElement("span");
  text.className = "text";
  text.textContent = item.text;
  wrap.appendChild(text);
  wrap.appendChild(Object.assign(document.createElement("span"), { className: "strike" }));
  label.appendChild(wrap);

  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "drop";
  drop.textContent = "×";

  row.appendChild(box);
  row.appendChild(label);
  row.appendChild(drop);

  row.addEventListener("click", event => {
    if (event.target === drop) discard(item.id);
    else toggle(item.id);
  });

  // The entrance class is one-shot: left on, it would replay the arrival every
  // time the row's style is resolved again.
  setTimeout(() => row.classList.remove("entering"), ENTER_MS);
  return row;
};

// The row carries its own idea of `done` so the flip animations run on the
// change rather than on every render.
const paint = (row, item) => {
  const was = row.getAttribute("data-done") === "true";
  if (was === item.done) return;
  row.setAttribute("data-done", String(item.done));
  row.classList.toggle("done", item.done);
  if (item.done) {
    row.classList.remove("undoing");
    replay(row, "playing", FLIP_MS);
  } else {
    row.classList.remove("playing");
    replay(row, "undoing", EXIT_MS);
  }
};

const leave = (id, row) => {
  row.classList.remove("entering");
  row.classList.add("leaving");
  leaving.set(id, row);
  timers.set(id, setTimeout(() => {
    leaving.delete(id);
    timers.delete(id);
    if (row.parentNode) row.parentNode.removeChild(row);
  }, EXIT_MS));
};

// A row asked for again while it is still leaving is the same row: cancelling
// the removal keeps its identity rather than stacking a second copy over it.
const revive = id => {
  const row = leaving.get(id);
  if (!row) return null;
  clearTimeout(timers.get(id));
  leaving.delete(id);
  timers.delete(id);
  row.classList.remove("leaving");
  replay(row, "entering", ENTER_MS);
  return row;
};

/* ── Render ────────────────────────────────────────────────────────────────── */

const visible = () => state.items.filter(item =>
  state.filter === "all" || (state.filter === "done") === item.done);

const render = () => {
  const shown = visible();
  const wanted = new Set(shown.map(item => item.id));

  for (const [id, row] of nodes) {
    if (wanted.has(id)) continue;
    nodes.delete(id);
    leave(id, row);
  }

  // Placed back to front, so the row each one goes in front of is already
  // there. A row that is only being reordered is left where it is: moving a
  // node re-resolves its style, and its arrival would play a second time.
  let anchor = null;
  for (let index = shown.length - 1; index >= 0; index--) {
    const item = shown[index];
    let row = nodes.get(item.id) ?? revive(item.id) ?? build(item);
    nodes.set(item.id, row);
    if (row.parentNode !== list) list.insertBefore(row, anchor);
    paint(row, item);
    anchor = row;
  }

  const left = state.items.filter(item => !item.done).length;
  const complete = state.items.length - left;
  remaining.textContent = String(left);
  tallyWord.textContent = left === 1 ? "task left" : "tasks left";
  finished.textContent = `${complete} done`;
  if (lastRemaining !== null && lastRemaining !== left) replay(remaining, "bump", 400);
  lastRemaining = left;

  progress.target = state.items.length === 0 ? 0 : (complete / state.items.length) * 100;
  clear.classList.toggle("live", complete > 0);

  const bare = shown.length === 0;
  empty.classList.toggle("shown", bare);
  if (bare) {
    emptyText.textContent = state.filter === "done" ? "Nothing finished yet."
      : state.filter === "active" ? "All clear. Every task is done."
      : "Nothing here yet.";
  }
};

/* ── Actions ───────────────────────────────────────────────────────────────── */

const arm = () => {
  const typed = entry.value.trim().length > 0;
  compose.classList.toggle("armed", typed);
  ghost.classList.toggle("gone", typed);
};

const add = () => {
  const text = entry.value.trim();
  if (!text) return;
  state.items.push({ id: state.nextId++, text, done: false });
  entry.value = "";
  arm();
  replay(addButton, "sent", 440);
  // Adding while looking at what is finished would put the new row somewhere
  // it cannot be seen, so the view follows the work.
  if (state.filter === "done") select("all");
  else render();
};

const toggle = id => {
  const item = state.items.find(candidate => candidate.id === id);
  if (!item) return;
  item.done = !item.done;
  render();
};

const discard = id => {
  state.items = state.items.filter(item => item.id !== id);
  render();
};

function select(name) {
  state.filter = name;
  // Where the thumb has to be is measured, not computed: the pills share the
  // row's width, so their geometry is whatever the flex line resolved to. The
  // offset is taken as a difference between two rectangles rather than from
  // `offsetLeft`, which this runtime does not answer.
  const row = filters.getBoundingClientRect();
  for (const button of filters.querySelectorAll("button")) {
    const on = button.getAttribute("data-filter") === name;
    button.classList.toggle("on", on);
    if (!on) continue;
    const box = button.getBoundingClientRect();
    thumbLeft.target = box.x - row.x;
    thumbWidth.target = box.width;
  }
  render();
}

entry.addEventListener("input", arm);
entry.addEventListener("keydown", event => { if (event.key === "Enter") add(); });
addButton.addEventListener("click", add);
filters.addEventListener("click", event => {
  const name = event.target.getAttribute("data-filter");
  if (name) select(name);
});
clear.addEventListener("click", () => {
  state.items = state.items.filter(item => !item.done);
  render();
});
// The prompt sits over the field rather than inside it, so a click anywhere in
// the row still lands on the control that takes the typing.
compose.addEventListener("click", () => entry.focus());

select("all");
requestAnimationFrame(frame);

// The shell arrives scaled, so the first measurement of the pills is taken
// through that transform. Re-measuring once it has landed, and again whenever
// the window changes size, keeps the thumb the width of the pill it is under.
const remeasure = () => select(state.filter);
setTimeout(remeasure, 800);
window.addEventListener("resize", remeasure);
