import nativeWindow from "blitsen/window";

// The list stays windowed so it remains responsive if a real task collection
// grows large, while localStorage gives the example ordinary app persistence.

const STORAGE_KEY = "blitsen.todo.tasks.v1";
const ROW_HEIGHT = 58;
const OVERSCAN = 6;
const PRIORITIES = ["normal", "medium", "high"];

const exampleTask = () => ({
  id: 1,
  text: "Try changing this task's priority",
  search: "try changing this task's priority",
  group: "Example",
  priority: "medium",
  done: false,
});

let loadWarning = "";
let initializeStorage = false;
const loadTasks = () => {
  const fallback = { items: [exampleTask()], nextId: 2 };
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === null) {
      initializeStorage = true;
      return fallback;
    }
    const parsed = JSON.parse(stored);
    if (parsed?.version !== 1 || !Array.isArray(parsed.items)) throw new Error("unsupported data");
    const usedIds = new Set();
    let nextId = 1;
    const items = [];
    for (const candidate of parsed.items) {
      const text = typeof candidate?.text === "string" ? candidate.text.trim() : "";
      if (!text) continue;
      let id = Number(candidate.id);
      if (!Number.isSafeInteger(id) || id < 1 || usedIds.has(id)) {
        while (usedIds.has(nextId)) nextId += 1;
        id = nextId;
      }
      usedIds.add(id);
      nextId = Math.max(nextId, id + 1);
      items.push({
        id,
        text,
        search: text.toLowerCase(),
        group: typeof candidate.group === "string" && candidate.group.trim()
          ? candidate.group.trim() : "Inbox",
        priority: PRIORITIES.includes(candidate.priority) ? candidate.priority : "normal",
        done: candidate.done === true,
      });
    }
    return { items, nextId };
  } catch {
    loadWarning = "Saved tasks could not be read, so a fresh list was opened.";
    return fallback;
  }
};

const initial = loadTasks();

const state = {
  filter: "all",
  query: "",
  nextId: initial.nextId,
  items: initial.items,
  shown: [],
  doneCount: initial.items.filter(item => item.done).length,
};
const byId = new Map(state.items.map(item => [item.id, item]));

const list = document.getElementById("list");
const viewport = document.getElementById("viewport");
const appRoot = document.querySelector(".app");
const compose = document.getElementById("compose");
const padTop = document.getElementById("pad-top");
const padBottom = document.getElementById("pad-bottom");
const entry = document.getElementById("entry");
const entryGhost = document.getElementById("entry-ghost");
const entryPriority = document.getElementById("entry-priority");
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
const viewLabel = document.getElementById("view-label");
const viewCount = document.getElementById("view-count");
const taskCount = document.getElementById("task-count");
const taskLabel = document.getElementById("task-label");
const clearCompleted = document.getElementById("clear-completed");
const maximizeIcon = document.getElementById("maximize-icon");
const saveBadge = document.getElementById("save-badge");
const saveState = document.getElementById("save-state");
const toast = document.getElementById("toast");
const toastMessage = document.getElementById("toast-message");
const toastAction = document.getElementById("toast-action");
const priorityMenu = document.getElementById("priority-menu");

const format = value => new Intl.NumberFormat().format(value);
const rows = new Map();
const reduceMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
let scheduledWindow = 0;
let animateRow = null;
let toastTimer = 0;
let toastCallback = null;

const persist = () => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      version: 1,
      items: state.items.map(({ id, text, group, priority, done }) => (
        { id, text, group, priority, done }
      )),
    }));
    saveState.textContent = "Saved locally";
    saveBadge.classList.remove("save-failed");
    return true;
  } catch {
    saveState.textContent = "Could not save";
    saveBadge.classList.add("save-failed");
    return false;
  }
};

/* ── On-demand motion ────────────────────────────────────────────────────── */

const springs = [];
const spring = (rate, apply) => {
  const value = { current: null, target: 0, rate, apply };
  springs.push(value);
  return value;
};
const progress = spring(reduceMotion ? 1 : 0.16, value => { bar.style.width = `${value}%`; });
const thumbLeft = spring(reduceMotion ? 1 : 0.24, value => { thumb.style.left = `${value}px`; });
const thumbWidth = spring(reduceMotion ? 1 : 0.24, value => { thumb.style.width = `${value}px`; });
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
  if (reduceMotion) return;
  element.classList.remove(className);
  void element.offsetWidth;
  element.classList.add(className);
  setTimeout(() => element.classList.remove(className), duration);
};

/* ── Priority menu ───────────────────────────────────────────────────────── */

let priorityTarget = null;
const priorityLabel = value => value[0].toUpperCase() + value.slice(1);

const setPriorityTrigger = (trigger, value, label) => {
  trigger.setAttribute("data-priority", value);
  trigger.textContent = priorityLabel(value);
  for (const priority of PRIORITIES) trigger.classList.toggle(priority, priority === value);
  trigger.setAttribute("aria-label", `${label}: ${priorityLabel(value)}`);
};

const closePriorityMenu = ({ restoreFocus = false } = {}) => {
  if (!priorityTarget) return;
  const { trigger } = priorityTarget;
  priorityTarget = null;
  priorityMenu.classList.remove("visible");
  trigger.setAttribute("aria-expanded", "false");
  if (restoreFocus) trigger.focus();
};

const openPriorityMenu = (trigger, taskId = null) => {
  if (priorityTarget?.trigger === trigger) {
    closePriorityMenu({ restoreFocus: true });
    return;
  }
  closePriorityMenu();
  priorityTarget = { trigger, taskId };
  const selectedPriority = trigger.getAttribute("data-priority") ?? "normal";
  let selectedButton = null;
  for (const button of priorityMenu.querySelectorAll("button")) {
    const selected = button.getAttribute("data-priority") === selectedPriority;
    button.setAttribute("aria-checked", String(selected));
    button.classList.toggle("selected", selected);
    if (selected) selectedButton = button;
  }
  trigger.setAttribute("aria-expanded", "true");
  priorityMenu.classList.add("visible");

  const appBox = appRoot.getBoundingClientRect();
  const triggerBox = trigger.getBoundingClientRect();
  const menuWidth = priorityMenu.offsetWidth || 144;
  const menuHeight = priorityMenu.offsetHeight || 112;
  let left = triggerBox.right - appBox.left - menuWidth;
  let top = triggerBox.bottom - appBox.top + 5;
  left = Math.max(8, Math.min(left, appBox.width - menuWidth - 8));
  if (top + menuHeight > appBox.height - 8) {
    top = triggerBox.top - appBox.top - menuHeight - 5;
  }
  priorityMenu.style.left = `${left}px`;
  priorityMenu.style.top = `${Math.max(8, top)}px`;
  selectedButton?.focus();
};

entryPriority.addEventListener("click", () => openPriorityMenu(entryPriority));
priorityMenu.addEventListener("click", event => {
  const option = event.target.closest("button");
  const value = option?.getAttribute("data-priority");
  if (!priorityTarget || !PRIORITIES.includes(value)) return;
  const { trigger, taskId } = priorityTarget;
  if (taskId === null) {
    setPriorityTrigger(trigger, value, "Priority for new task");
  } else {
    const item = byId.get(taskId);
    if (item) {
      item.priority = value;
      setPriorityTrigger(trigger, value, `Priority for ${item.text}`);
      persist();
    }
  }
  closePriorityMenu({ restoreFocus: true });
});
priorityMenu.addEventListener("keydown", event => {
  const options = [...priorityMenu.querySelectorAll("button")];
  const current = Math.max(0, options.indexOf(document.activeElement));
  let next = null;
  if (event.key === "ArrowDown") next = (current + 1) % options.length;
  else if (event.key === "ArrowUp") next = (current + options.length - 1) % options.length;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = options.length - 1;
  else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closePriorityMenu({ restoreFocus: true });
    return;
  }
  if (next !== null) {
    event.preventDefault();
    options[next].focus();
  }
});
document.addEventListener("pointerdown", event => {
  if (!priorityTarget || event.target.closest(".priority-menu")
    || event.target.closest(".priority-trigger")) return;
  closePriorityMenu();
});

/* ── Bounded row renderer ────────────────────────────────────────────────── */

const buildRow = () => {
  const row = document.createElement("li");
  row.className = "item";

  const check = document.createElement("button");
  check.type = "button";
  check.className = "check";
  check.setAttribute("role", "checkbox");
  const tick = Object.assign(document.createElement("span"), { className: "tick" });
  tick.setAttribute("aria-hidden", "true");
  check.appendChild(tick);

  const copy = document.createElement("span");
  copy.className = "task-copy";
  const title = document.createElement("span");
  title.className = "task-title";
  const meta = document.createElement("span");
  meta.className = "task-meta";
  copy.append(title, meta);

  const priority = document.createElement("button");
  priority.type = "button";
  priority.className = "priority priority-trigger task-priority";
  priority.setAttribute("aria-haspopup", "menu");
  priority.setAttribute("aria-expanded", "false");
  priority.setAttribute("aria-controls", "priority-menu");
  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "drop";
  drop.setAttribute("aria-label", "Delete task");
  drop.textContent = "×";

  row.append(check, copy, priority, drop);
  row._check = check;
  row._title = title;
  row._meta = meta;
  row._priority = priority;
  row._drop = drop;
  return row;
};

const paintRow = (row, item) => {
  row.setAttribute("data-id", String(item.id));
  row.classList.toggle("done", item.done);
  row._title.textContent = item.text;
  row._title.setAttribute("title", item.text);
  row._meta.textContent = item.group;
  row._priority.className = `priority priority-trigger task-priority ${item.priority}`;
  row._priority.setAttribute("data-priority", item.priority);
  row._priority.textContent = item.priority[0].toUpperCase() + item.priority.slice(1);
  row._priority.setAttribute("aria-label", `Priority for ${item.text}`);
  row._check.setAttribute("aria-checked", String(item.done));
  row._check.setAttribute("aria-label", `${item.done ? "Mark active" : "Mark complete"}: ${item.text}`);
  row._drop.setAttribute("aria-label", `Delete task: ${item.text}`);
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
      row = buildRow();
      rows.set(item.id, row);
    }
    row.setAttribute("aria-posinset", String(index + 1));
    row.setAttribute("aria-setsize", String(state.shown.length));
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
    button.setAttribute("aria-pressed", String(selected));
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
  taskCount.textContent = format(total);
  taskLabel.textContent = `${total === 1 ? "task" : "tasks"} total`;
  searchGhost.textContent = `Search ${format(total)} ${total === 1 ? "task" : "tasks"}`;
  percent.textContent = `${Math.round(completion)}%`;
  progress.target = completion;
  const visibleQuery = search.value.trim();
  viewLabel.textContent = state.query ? `Results for “${visibleQuery}”`
    : state.filter === "active" ? "Active tasks"
    : state.filter === "done" ? "Completed tasks" : "All tasks";
  viewCount.textContent = `${format(state.shown.length)} ${state.shown.length === 1 ? "result" : "results"}`;
  clearCompleted.disabled = state.doneCount === 0;
  clearCompleted.setAttribute("aria-label", state.doneCount === 0
    ? "No completed tasks to clear"
    : `Clear ${format(state.doneCount)} completed ${state.doneCount === 1 ? "task" : "tasks"}`);
  animate();
};

const refreshView = ({ keepScroll = false } = {}) => {
  closePriorityMenu();
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
      : state.filter === "done" ? "Nothing completed yet"
      : state.filter === "active" ? "All caught up" : "No tasks yet";
    emptyText.textContent = state.query ? "Try a shorter search phrase."
      : state.filter === "done" ? "Completed tasks will appear here."
      : state.filter === "active" ? "There are no active tasks in the queue."
      : "Add a task above to start a new queue.";
  }
  renderSummary();
  renderWindow();
};

/* ── Actions ─────────────────────────────────────────────────────────────── */

const syncEntry = () => {
  entryGhost.classList.toggle("hidden", entry.value.length > 0);
  addButton.disabled = entry.value.trim().length === 0;
};
const syncSearch = () => {
  const typed = search.value.trim();
  searchGhost.classList.toggle("hidden", typed.length > 0);
  clearSearch.classList.toggle("visible", typed.length > 0);
  state.query = typed.toLowerCase();
  refreshView();
};

const hideToast = () => {
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = 0;
  toastCallback = null;
  toast.classList.remove("visible");
  toastAction.classList.remove("visible");
};

const showToast = (message, { label = "", action = null } = {}) => {
  if (toastTimer) clearTimeout(toastTimer);
  toastMessage.textContent = message;
  toastAction.textContent = label;
  toastCallback = action;
  toastAction.classList.toggle("visible", Boolean(action));
  toast.classList.add("visible");
  toastTimer = setTimeout(hideToast, action ? 8000 : 3200);
};

toastAction.addEventListener("click", () => {
  const action = toastCallback;
  hideToast();
  action?.();
  viewport.focus();
});
toastAction.addEventListener("focus", () => {
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = 0;
});
toastAction.addEventListener("blur", () => {
  if (toast.classList.contains("visible")) toastTimer = setTimeout(hideToast, 3200);
});

const add = () => {
  const text = entry.value.trim();
  if (!text) return;
  const item = {
    id: state.nextId++,
    text,
    search: text.toLowerCase(),
    group: "Inbox",
    priority: PRIORITIES.includes(entryPriority.getAttribute("data-priority"))
      ? entryPriority.getAttribute("data-priority") : "normal",
    done: false,
  };
  state.items.unshift(item);
  byId.set(item.id, item);
  entry.value = "";
  setPriorityTrigger(entryPriority, "normal", "Priority for new task");
  syncEntry();
  persist();
  animateRow = item.id;
  const visible = state.filter !== "done" && (!state.query || item.search.includes(state.query));
  refreshView({ keepScroll: !visible });
  replay(addButton, "sent", 280);
  showToast(visible ? "Task added to the queue." : "Task added, but hidden by the current view.", {
    label: visible ? "Undo" : "Show task",
    action: () => {
      if (visible) {
        if (!byId.has(item.id)) return;
        state.items = state.items.filter(candidate => candidate.id !== item.id);
        byId.delete(item.id);
        if (item.done) state.doneCount -= 1;
        persist();
      } else {
        state.filter = "all";
        state.query = "";
        search.value = "";
        syncSearch();
        measureFilter();
        return;
      }
      refreshView({ keepScroll: true });
    },
  });
};

const toggle = id => {
  const item = byId.get(id);
  if (!item) return;
  item.done = !item.done;
  state.doneCount += item.done ? 1 : -1;
  persist();
  animateRow = id;
  const remainsVisible = state.filter === "all";
  refreshView({ keepScroll: remainsVisible });
  if (!remainsVisible) viewport.focus();
};

const discard = id => {
  const item = byId.get(id);
  if (!item) return;
  const index = state.items.indexOf(item);
  if (item.done) state.doneCount -= 1;
  byId.delete(id);
  state.items = state.items.filter(candidate => candidate.id !== id);
  persist();
  refreshView({ keepScroll: true });
  viewport.focus();
  showToast("Task deleted.", {
    label: "Undo",
    action: () => {
      if (byId.has(item.id)) return;
      state.items.splice(Math.min(index, state.items.length), 0, item);
      byId.set(item.id, item);
      if (item.done) state.doneCount += 1;
      persist();
      refreshView({ keepScroll: true });
    },
  });
};

entry.addEventListener("input", syncEntry);
compose.addEventListener("submit", event => { event.preventDefault(); add(); });
search.addEventListener("input", syncSearch);
clearSearch.addEventListener("click", () => { search.value = ""; syncSearch(); search.focus(); });
filters.addEventListener("click", event => {
  const button = event.target.closest("button");
  const filter = button?.getAttribute("data-filter");
  if (!filter || filter === state.filter) return;
  state.filter = filter;
  measureFilter();
  refreshView();
});
list.addEventListener("click", event => {
  const row = event.target.closest(".item");
  if (!row) return;
  const id = Number(row.getAttribute("data-id"));
  const action = event.target.closest("button");
  if (action?.classList.contains("drop")) discard(id);
  else if (action?.classList.contains("check")) toggle(id);
  else if (action?.classList.contains("task-priority")) openPriorityMenu(action, id);
});
clearCompleted.addEventListener("click", () => {
  if (state.doneCount === 0) return;
  const cleared = [];
  state.items = state.items.filter((item, index) => {
    if (!item.done) return true;
    cleared.push({ item, index });
    byId.delete(item.id);
    return false;
  });
  state.doneCount = 0;
  persist();
  refreshView();
  viewport.focus();
  showToast(`${format(cleared.length)} completed ${cleared.length === 1 ? "task" : "tasks"} cleared.`, {
    label: "Undo",
    action: () => {
      for (const { item, index } of cleared) {
        if (byId.has(item.id)) continue;
        state.items.splice(Math.min(index, state.items.length), 0, item);
        byId.set(item.id, item);
        state.doneCount += 1;
      }
      persist();
      refreshView({ keepScroll: true });
    },
  });
});

document.addEventListener("keydown", event => {
  if (event.key === "Escape" && search.value) {
    search.value = "";
    syncSearch();
    search.focus();
    return;
  }
  const findShortcut = event.key === "/" || ((event.ctrlKey || event.metaKey)
    && event.key.toLowerCase() === "f");
  if (!findShortcut || document.activeElement === entry || document.activeElement === search) return;
  event.preventDefault();
  search.focus();
});

// The native scroller applies wheel/keyboard defaults after dispatch. Rendering
// on the next frame reads the settled offset and swaps only the bounded window.
viewport.addEventListener("wheel", () => { closePriorityMenu(); scheduleWindow(); });
viewport.addEventListener("scroll", () => { closePriorityMenu(); scheduleWindow(); });
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
const closeButton = document.getElementById("window-close");
let closeRequested = false;
const requestWindowClose = () => {
  if (closeRequested) return;
  if (typeof nativeWindow.close !== "function") {
    showToast("This runtime cannot close the application window.");
    return;
  }
  closeRequested = true;
  closeButton.disabled = true;
  closeButton.classList.add("closing");
  try {
    nativeWindow.close();
  } catch {
    closeRequested = false;
    closeButton.disabled = false;
    closeButton.classList.remove("closing");
    showToast("The application window could not be closed.");
  }
};
closeButton.addEventListener("pointerup", event => {
  if (event.button === 0) requestWindowClose();
});
// Keyboard activation produces a click without a preceding pointer event.
closeButton.addEventListener("click", requestWindowClose);

window.addEventListener("load", () => {
  // Runs before the hidden startup surface is revealed, avoiding a decorated
  // frame flashing ahead of the custom chrome.
  nativeWindow.setDecorations?.(false);
  syncMaximize();
  measureFilter();
  renderWindow();
});
window.addEventListener("resize", () => {
  closePriorityMenu();
  syncMaximize();
  measureFilter();
  scheduleWindow();
});

setPriorityTrigger(entryPriority, entryPriority.getAttribute("data-priority"), "Priority for new task");
refreshView();
if (initializeStorage) persist();
if (loadWarning) {
  saveState.textContent = "Needs saving";
  saveBadge.classList.add("save-failed");
  showToast(loadWarning);
}
