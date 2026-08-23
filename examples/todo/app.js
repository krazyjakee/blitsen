import nativeWindow from "blitsen/window";

// This example starts with a real large collection and keeps the DOM bounded.
// Filtering is O(n) over plain data; scrolling is O(visible rows) and mounts a
// couple of screenfuls at most, whether the collection has 100 or 100,000 rows.

const INITIAL_TASKS = 10_000;
const ROW_HEIGHT = 58;
const OVERSCAN = 6;

const templates = [
  "Review the overnight deployment report",
  "Validate the queue consumer checkpoint",
  "Confirm regional failover capacity",
  "Triage the latest customer feedback",
  "Reconcile the weekly usage export",
  "Update the release readiness notes",
  "Check slow queries against the budget",
  "Verify the archive retention policy",
  "Prepare the operations handoff",
  "Audit notification delivery latency",
  "Resolve the workspace access request",
  "Document the incident follow-up",
];
const groups = ["Platform", "Reliability", "Data", "Customer", "Security", "Release"];
const priorities = ["normal", "normal", "medium", "normal", "high"];

const makeTask = id => {
  const text = `${templates[(id - 1) % templates.length]} · batch ${Math.ceil(id / templates.length)}`;
  return {
    id,
    text,
    search: text.toLowerCase(),
    group: groups[(id - 1) % groups.length],
    priority: priorities[(id - 1) % priorities.length],
    done: id % 4 === 0,
  };
};

const state = {
  filter: "all",
  query: "",
  nextId: INITIAL_TASKS + 1,
  items: Array.from({ length: INITIAL_TASKS }, (_, index) => makeTask(index + 1)),
  shown: [],
  doneCount: Math.floor(INITIAL_TASKS / 4),
};
const byId = new Map(state.items.map(item => [item.id, item]));

const list = document.getElementById("list");
const viewport = document.getElementById("viewport");
const padTop = document.getElementById("pad-top");
const padBottom = document.getElementById("pad-bottom");
const entry = document.getElementById("entry");
const entryGhost = document.getElementById("entry-ghost");
const search = document.getElementById("search");
const searchGhost = document.getElementById("search-ghost");
const clearSearch = document.getElementById("clear-search");
const addButton = document.getElementById("add");
const filters = document.getElementById("filters");
const thumb = document.getElementById("thumb");
const empty = document.getElementById("empty");
const emptyTitle = document.getElementById("empty-title");
const emptyText = document.getElementById("empty-text");
const remaining = document.getElementById("remaining");
const finished = document.getElementById("finished");
const percent = document.getElementById("percent");
const bar = document.getElementById("bar");
const datasetSize = document.getElementById("dataset-size");
const viewLabel = document.getElementById("view-label");
const viewCount = document.getElementById("view-count");
const mountedCount = document.getElementById("mounted-count");
const clearCompleted = document.getElementById("clear-completed");
const maximizeIcon = document.getElementById("maximize-icon");

const format = value => new Intl.NumberFormat().format(value);
const rows = new Map();
let scheduledWindow = 0;
let animateRow = null;

/* ── On-demand motion ────────────────────────────────────────────────────── */

const springs = [];
const spring = (rate, apply) => {
  const value = { current: null, target: 0, rate, apply };
  springs.push(value);
  return value;
};
const progress = spring(0.16, value => { bar.style.width = `${value}%`; });
const thumbLeft = spring(0.24, value => { thumb.style.left = `${value}px`; });
const thumbWidth = spring(0.24, value => { thumb.style.width = `${value}px`; });
let motionFrame = 0;

const move = () => {
  let unsettled = false;
  for (const value of springs) {
    if (value.current === null) value.current = value.target;
    else value.current += (value.target - value.current) * value.rate;
    if (Math.abs(value.target - value.current) < 0.05) value.current = value.target;
    else unsettled = true;
    value.apply(value.current);
  }
  motionFrame = unsettled ? requestAnimationFrame(move) : 0;
};
const animate = () => { if (!motionFrame) motionFrame = requestAnimationFrame(move); };

const replay = (element, className, duration) => {
  element.classList.remove(className);
  void element.offsetWidth;
  element.classList.add(className);
  setTimeout(() => element.classList.remove(className), duration);
};

/* ── Bounded row renderer ────────────────────────────────────────────────── */

const buildRow = item => {
  const row = document.createElement("li");
  row.className = "item";
  row.setAttribute("data-id", String(item.id));

  const check = document.createElement("span");
  check.className = "check";
  check.appendChild(Object.assign(document.createElement("span"), { className: "tick" }));

  const copy = document.createElement("span");
  copy.className = "task-copy";
  const title = document.createElement("span");
  title.className = "task-title";
  const meta = document.createElement("span");
  meta.className = "task-meta";
  copy.append(title, meta);

  const id = document.createElement("span");
  id.className = "task-id";
  const priority = document.createElement("span");
  priority.className = "priority";
  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "drop";
  drop.setAttribute("aria-label", "Delete task");
  drop.textContent = "×";

  row.append(check, copy, id, priority, drop);
  row._title = title;
  row._meta = meta;
  row._taskId = id;
  row._priority = priority;
  return row;
};

const paintRow = (row, item) => {
  row.classList.toggle("done", item.done);
  row._title.textContent = item.text;
  row._meta.textContent = `${item.group} · queued work`;
  row._taskId.textContent = `#${String(item.id).padStart(5, "0")}`;
  row._priority.className = `priority ${item.priority}`;
  row._priority.textContent = item.priority;
  if (animateRow === item.id) replay(row, "changed", 280);
};

const renderWindow = () => {
  scheduledWindow = 0;
  const height = viewport.clientHeight || 420;
  const scrollTop = viewport.scrollTop;
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const last = Math.min(state.shown.length,
    first + Math.ceil(height / ROW_HEIGHT) + OVERSCAN * 2);
  const wanted = new Set();

  for (let index = first; index < last; index += 1) {
    const item = state.shown[index];
    wanted.add(item.id);
    let row = rows.get(item.id);
    if (!row) {
      row = buildRow(item);
      rows.set(item.id, row);
    }
    paintRow(row, item);
    list.insertBefore(row, padBottom);
  }
  for (const [id, row] of rows) {
    if (wanted.has(id)) continue;
    rows.delete(id);
    row.remove();
  }

  padTop.style.height = `${first * ROW_HEIGHT}px`;
  padBottom.style.height = `${Math.max(0, (state.shown.length - last) * ROW_HEIGHT)}px`;
  mountedCount.textContent = String(rows.size);
  animateRow = null;
};

const scheduleWindow = () => {
  if (!scheduledWindow) scheduledWindow = requestAnimationFrame(renderWindow);
};

/* ── Collection view ─────────────────────────────────────────────────────── */

const measureFilter = () => {
  const filterBox = filters.getBoundingClientRect();
  for (const button of filters.querySelectorAll("button")) {
    const selected = button.getAttribute("data-filter") === state.filter;
    button.classList.toggle("on", selected);
    if (!selected) continue;
    const buttonBox = button.getBoundingClientRect();
    thumbLeft.target = buttonBox.x - filterBox.x;
    thumbWidth.target = buttonBox.width;
  }
  animate();
};

const renderSummary = () => {
  const total = state.items.length;
  const open = total - state.doneCount;
  const completion = total === 0 ? 0 : state.doneCount / total * 100;
  remaining.textContent = format(open);
  finished.textContent = format(state.doneCount);
  datasetSize.textContent = format(total);
  percent.textContent = `${Math.round(completion)}%`;
  progress.target = completion;
  viewLabel.textContent = state.query ? `Results for “${state.query}”`
    : state.filter === "active" ? "Active tasks"
    : state.filter === "done" ? "Completed tasks" : "All tasks";
  viewCount.textContent = `${format(state.shown.length)} ${state.shown.length === 1 ? "result" : "results"}`;
  clearCompleted.disabled = state.doneCount === 0;
  animate();
};

const refreshView = ({ keepScroll = false } = {}) => {
  const query = state.query;
  state.shown = state.items.filter(item => {
    if (state.filter === "active" && item.done) return false;
    if (state.filter === "done" && !item.done) return false;
    return !query || item.search.includes(query);
  });
  if (!keepScroll) viewport.scrollTop = 0;
  empty.classList.toggle("visible", state.shown.length === 0);
  if (state.shown.length === 0) {
    emptyTitle.textContent = state.query ? "No matching tasks"
      : state.filter === "done" ? "Nothing completed yet" : "Queue is clear";
    emptyText.textContent = state.query ? "Try a shorter search phrase."
      : "Choose another filter or add a new task.";
  }
  renderSummary();
  renderWindow();
};

/* ── Actions ─────────────────────────────────────────────────────────────── */

const syncEntry = () => entryGhost.classList.toggle("hidden", entry.value.length > 0);
const syncSearch = () => {
  const typed = search.value.trim();
  searchGhost.classList.toggle("hidden", typed.length > 0);
  clearSearch.classList.toggle("visible", typed.length > 0);
  state.query = typed.toLowerCase();
  refreshView();
};

const add = () => {
  const text = entry.value.trim();
  if (!text) return;
  const item = {
    id: state.nextId++, text, search: text.toLowerCase(), group: "Inbox", priority: "normal", done: false,
  };
  state.items.unshift(item);
  byId.set(item.id, item);
  entry.value = "";
  syncEntry();
  state.filter = "all";
  state.query = "";
  search.value = "";
  syncSearch();
  animateRow = item.id;
  replay(addButton, "sent", 280);
  measureFilter();
};

const toggle = id => {
  const item = byId.get(id);
  if (!item) return;
  item.done = !item.done;
  state.doneCount += item.done ? 1 : -1;
  animateRow = id;
  refreshView({ keepScroll: state.filter === "all" && !state.query });
};

const discard = id => {
  const item = byId.get(id);
  if (!item) return;
  if (item.done) state.doneCount -= 1;
  byId.delete(id);
  state.items = state.items.filter(candidate => candidate.id !== id);
  refreshView({ keepScroll: true });
};

entry.addEventListener("input", syncEntry);
entry.addEventListener("keydown", event => { if (event.key === "Enter") add(); });
addButton.addEventListener("click", add);
search.addEventListener("input", syncSearch);
clearSearch.addEventListener("click", () => { search.value = ""; syncSearch(); search.focus(); });
filters.addEventListener("click", event => {
  const filter = event.target.getAttribute("data-filter");
  if (!filter || filter === state.filter) return;
  state.filter = filter;
  measureFilter();
  refreshView();
});
list.addEventListener("click", event => {
  const row = event.target.closest(".item");
  if (!row) return;
  const id = Number(row.getAttribute("data-id"));
  if (event.target.classList.contains("drop")) discard(id);
  else toggle(id);
});
clearCompleted.addEventListener("click", () => {
  if (state.doneCount === 0) return;
  state.items = state.items.filter(item => {
    if (!item.done) return true;
    byId.delete(item.id);
    return false;
  });
  state.doneCount = 0;
  refreshView();
});

// The native scroller applies wheel/keyboard defaults after dispatch. Rendering
// on the next frame reads the settled offset and swaps only the bounded window.
viewport.addEventListener("wheel", scheduleWindow);
viewport.addEventListener("scroll", scheduleWindow);
viewport.addEventListener("keydown", scheduleWindow);

/* ── Application-drawn window chrome ─────────────────────────────────────── */

const syncMaximize = () => {
  const maximized = nativeWindow.isMaximized?.() ?? false;
  maximizeIcon.classList.toggle("restore", maximized);
  document.getElementById("window-max").setAttribute("aria-label", maximized ? "Restore" : "Maximize");
};

document.getElementById("drag-region").addEventListener("pointerdown", event => {
  if (event.button === 0) nativeWindow.startDrag?.();
});
document.getElementById("drag-region").addEventListener("dblclick", () => {
  nativeWindow.setMaximized?.(!(nativeWindow.isMaximized?.() ?? false));
  syncMaximize();
});
document.getElementById("window-min").addEventListener("click", () => nativeWindow.setMinimized?.(true));
document.getElementById("window-max").addEventListener("click", () => {
  nativeWindow.setMaximized?.(!(nativeWindow.isMaximized?.() ?? false));
  syncMaximize();
});
document.getElementById("window-close").addEventListener("click", () => nativeWindow.close?.());

window.addEventListener("load", () => {
  // Runs before the hidden startup surface is revealed, avoiding a decorated
  // frame flashing ahead of the custom chrome.
  nativeWindow.setDecorations?.(false);
  syncMaximize();
  measureFilter();
  renderWindow();
});
window.addEventListener("resize", () => {
  syncMaximize();
  measureFilter();
  scheduleWindow();
});

refreshView();
