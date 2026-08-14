// A hardware and storage report, in the shape CPU-Z made familiar, on facts the
// web platform cannot reach.
//
// Reaching the native namespace
// -----------------------------
// An application with a build step imports the module by name:
//
//     import os from "blitsen/os";
//
// `blitsen/os` is an ordinary package subpath, so every bundler resolves it, and
// the package proxies it onto the namespace the runtime installs. This example
// has no build step — it is three files a browser could open — and a bare
// specifier is not resolvable without one, so it reads that same namespace
// directly. Both spellings reach the identical frozen object.
const native = globalThis[Symbol.for("blitsen.native")];
const os = native?.os;

// Everything below is declarations; the one statement that runs is the last line
// of the file. `start()` is hoisted and so could be called from up here, but the
// helpers it reaches — `element`, `write`, `fill` — are `const`, and calling it
// before they are initialized is a temporal-dead-zone error rather than a
// working program.

// ---- formatting -----------------------------------------------------------

// Binary units, labelled as such. A 1 TB drive reads 931.3 GiB here, which is
// the number every file manager shows for it and not a rounding mistake.
const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

function bytes(value, precision = 1) {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : precision)} ${UNITS[unit]}`;
}

const percent = (part, whole) => (whole > 0 ? (part / whole) * 100 : 0);
const pct = value => `${value.toFixed(value >= 10 ? 0 : 1)}%`;

function duration(seconds) {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m ${Math.floor(seconds % 60)}s`;
}

// Built by hand rather than with the locale-aware formatters: the engine ships
// none of ECMA-402, so those methods still return a string but ignore the locale
// they are handed, which makes a missed one wrong output rather than an error.
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function timestamp(unixSeconds) {
  const at = new Date(unixSeconds * 1000);
  const pad = number => String(number).padStart(2, "0");
  return `${pad(at.getDate())} ${MONTHS[at.getMonth()]} ${at.getFullYear()}`
    + ` · ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

const element = id => document.getElementById(id);
const write = (id, text) => { element(id).textContent = text; };
const bar = (id, value) => { element(id).style.width = `${Math.min(100, Math.max(0, value))}%`; };

// One class for the whole load scale, so a saturated core reads as saturated
// before the number is looked at.
const heat = usage => (usage >= 85 ? "hot" : "cpu");

function fill(node, usage, tone) {
  node.className = `fill ${tone}`;
  node.style.width = `${Math.min(100, Math.max(0, usage))}%`;
}

// ---- tabs -----------------------------------------------------------------

for (const tab of document.querySelectorAll(".tab")) {
  tab.addEventListener("click", () => {
    for (const other of document.querySelectorAll(".tab")) other.classList.remove("on");
    for (const panel of document.querySelectorAll(".panel")) panel.classList.remove("on");
    tab.classList.add("on");
    element(`panel-${tab.dataset.panel}`).classList.add("on");
  });
}

// ---- rendering ------------------------------------------------------------

function specs(container, rows) {
  container.replaceChildren();
  for (const [label, value] of rows) {
    const cell = document.createElement("div");
    const term = document.createElement("dt");
    term.textContent = label;
    const detail = document.createElement("dd");
    detail.textContent = value;
    cell.append(term, detail);
    container.append(cell);
  }
}

// The per-core tiles are built once and then only written to. Rebuilding 24 of
// them every second would be the one part of this that could not keep up.
let coreTiles = [];

function buildCores(cpu) {
  const container = element("cores");
  container.replaceChildren();
  coreTiles = cpu.cores.map(core => {
    const tile = document.createElement("div");
    tile.className = "core";
    tile.innerHTML = '<div class="core-head"><span class="core-name"></span>'
      + '<span class="core-pct"></span></div>'
      + '<div class="track"><div class="fill cpu"></div></div>'
      + '<div class="core-mhz"></div>';
    tile.querySelector(".core-name").textContent = core.name;
    container.append(tile);
    return {
      pct: tile.querySelector(".core-pct"),
      fill: tile.querySelector(".fill"),
      mhz: tile.querySelector(".core-mhz"),
    };
  });
  write("core-count", `${cpu.cores.length} threads`);
}

function renderCpu(cpu, sampled) {
  write("cpu-brand", cpu.brand || "Unknown processor");
  specs(element("cpu-specs"), [
    ["Vendor", cpu.vendor || "—"],
    ["Architecture", cpu.architecture],
    ["Physical cores", cpu.physicalCores === null ? "not reported" : String(cpu.physicalCores)],
    ["Logical cores", String(cpu.logicalCores)],
  ]);

  if (coreTiles.length !== cpu.cores.length) buildCores(cpu);

  write("cpu-usage", sampled ? pct(cpu.usage) : "—");
  fill(element("cpu-usage-fill"), sampled ? cpu.usage : 0, heat(cpu.usage));
  write("live-cpu", sampled ? pct(cpu.usage) : "—");
  fill(element("live-cpu-fill"), sampled ? cpu.usage : 0, heat(cpu.usage));

  cpu.cores.forEach((core, index) => {
    const tile = coreTiles[index];
    if (!tile) return;
    tile.pct.textContent = sampled ? pct(core.usage) : "—";
    fill(tile.fill, sampled ? core.usage : 0, heat(core.usage));
    tile.mhz.textContent = core.frequency > 0 ? `${core.frequency} MHz` : "—";
  });

  const fastest = Math.max(...cpu.cores.map(core => core.frequency));
  write("cpu-note", sampled
    ? `Load is the share of each thread busy since the previous sample, one second ago.`
      + ` Fastest thread right now: ${fastest} MHz.`
    : "First sample taken — it measures since boot rather than an interval, so it is"
      + " discarded and the reading starts a second from now.");
}

function renderMemory(memory) {
  const used = percent(memory.used, memory.total);
  write("mem-headline", `${bytes(memory.used)} of ${bytes(memory.total)} in use`);
  fill(element("mem-used-fill"), used, used >= 85 ? "hot" : "mem");
  write("mem-note", `${bytes(memory.available)} available to a new allocation, which counts`
    + " reclaimable cache and so is not simply total minus used.");
  write("live-mem", pct(used));
  fill(element("live-mem-fill"), used, used >= 85 ? "hot" : "mem");

  const tiles = element("mem-tiles");
  tiles.replaceChildren();
  for (const [label, value, note] of [
    ["Installed", bytes(memory.total), "physical"],
    ["In use", bytes(memory.used), pct(used)],
    ["Available", bytes(memory.available), pct(percent(memory.available, memory.total))],
    ["Swap in use", bytes(memory.swapUsed), `of ${bytes(memory.swapTotal)}`],
  ]) {
    const tile = document.createElement("div");
    tile.className = "tile";
    tile.innerHTML = "<span></span><b></b><i></i>";
    tile.querySelector("span").textContent = label;
    tile.querySelector("b").textContent = value;
    tile.querySelector("i").textContent = note;
    tiles.append(tile);
  }

  const swap = percent(memory.swapUsed, memory.swapTotal);
  write("swap-headline", memory.swapTotal > 0
    ? `${bytes(memory.swapUsed)} of ${bytes(memory.swapTotal)}` : "none configured");
  fill(element("swap-fill"), swap, swap >= 85 ? "hot" : "swap");
  write("swap-note", memory.swapTotal > 0
    ? `${pct(swap)} of the configured swap is in use.`
    : "This machine has no swap configured.");
}

function renderStorage(volumes) {
  // A running desktop mounts pseudo-filesystems with no capacity — an AppImage,
  // a snap loopback. They are real mounts and the module reports them honestly;
  // they are just not drives, and zero capacity is exactly how to tell.
  const drives = volumes.filter(volume => volume.total > 0)
    .sort((left, right) => right.total - left.total);
  const hidden = volumes.length - drives.length;

  const container = element("volumes");
  container.replaceChildren();
  for (const volume of drives) {
    const used = volume.total - volume.available;
    const share = percent(used, volume.total);
    const card = document.createElement("div");
    card.className = "volume";
    card.innerHTML = '<div class="vol-head"><span class="vol-mount"></span>'
      + '<span class="badge kind"></span><span class="badge fs"></span>'
      + '<span class="vol-dev"></span></div>'
      + '<div class="vol-figures"><span class="vol-free"></span>'
      + '<span class="vol-total"></span></div>'
      + '<div class="track tall"><div class="fill disk"></div></div>';
    card.querySelector(".vol-mount").textContent = volume.mountPoint;
    const kind = card.querySelector(".kind");
    kind.textContent = volume.kind === "unknown" ? "disk" : volume.kind;
    kind.className = `badge kind ${volume.kind}`;
    card.querySelector(".fs").textContent = volume.fileSystem || "—";
    card.querySelector(".vol-dev").textContent = volume.name
      + (volume.removable ? " · removable" : "");
    card.querySelector(".vol-free").textContent = `${bytes(volume.available)} free`;
    card.querySelector(".vol-total").textContent =
      `${bytes(used)} used of ${bytes(volume.total)} · ${pct(share)}`;
    fill(card.querySelector(".fill"), share, share >= 90 ? "hot" : "disk");
    if (volume.readOnly) {
      const badge = document.createElement("span");
      badge.className = "badge ro";
      badge.textContent = "read-only";
      card.querySelector(".vol-head").append(badge);
    }
    container.append(card);
  }

  const total = drives.reduce((sum, volume) => sum + volume.total, 0);
  const free = drives.reduce((sum, volume) => sum + volume.available, 0);
  write("storage-note", `${drives.length} volume${drives.length === 1 ? "" : "s"}`
    + ` · ${bytes(free)} free of ${bytes(total)} total`
    + (hidden > 0
      ? ` · ${hidden} zero-capacity mount${hidden === 1 ? "" : "s"} hidden (AppImages, loopbacks)`
      : ""));
  return drives.length;
}

function renderHost(host, cpu) {
  write("machine", host.hostName ?? "This machine");
  write("platform", `${host.longName ?? host.name ?? "Unknown OS"} · ${cpu.architecture}`);
  write("live-uptime", duration(host.uptime));
  write("os-headline", host.longName ?? host.name ?? "Unknown operating system");
  specs(element("system-specs"), [
    ["Operating system", host.name ?? "—"],
    ["Version", host.osVersion ?? "—"],
    ["Kernel", host.kernelVersion ?? "—"],
    ["Distribution id", host.distributionId],
    ["Host name", host.hostName ?? "—"],
    ["Architecture", cpu.architecture],
    ["Booted", timestamp(host.bootTime)],
    ["Uptime", duration(host.uptime)],
    ["Processor", cpu.brand],
  ]);
}

// ---- polling --------------------------------------------------------------

function start() {
  // The first `cpu()` call has no previous call to measure against, so what it
  // reports is a baseline against the counters' own origin — on Linux, the
  // average since boot. It is a real number but not the one this is showing, so
  // it is drawn as "—" and every reading from the second on is an interval.
  let sampled = false;

  const paint = () => {
    const cpu = os.cpu();
    renderCpu(cpu, sampled);
    renderMemory(os.memory());
    renderHost(os.host(), cpu);
    sampled = true;
    return cpu;
  };

  const cpu = paint();
  const volumes = renderStorage(os.storage());
  setInterval(paint, 1000);
  // Mounts change when someone plugs something in, not every second.
  setInterval(() => renderStorage(os.storage()), 5000);

  // The acceptance marker, in the shape pong and interactive use: proof the
  // script reached the end, carrying the two counts that prove it read a real
  // machine rather than rendering an empty shell.
  const header = element("bar");
  header.dataset.ready = "true";
  header.dataset.threads = String(cpu.logicalCores);
  header.dataset.volumes = String(volumes);
}

function unsupported() {
  element("tabs").style.display = "none";
  document.querySelector("main").style.display = "none";
  element("unsupported").style.display = "block";
}

// The only statement in the file, so every declaration above it is initialized.
if (os) start(); else unsupported();
