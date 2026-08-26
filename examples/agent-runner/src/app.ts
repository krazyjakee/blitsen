import { spawn } from "node:child_process";
import { readFile, readdir, stat } from "node:fs/promises";
import dialog from "blitsen/dialog";

type Agent = { name: string; path: string; relative: string; folder: string; content: string; mtime: number };
type CliName = "codex" | "claude";
type AgentBranch = { folders: Map<string, AgentBranch>; agents: Agent[] };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const rootInput = $<HTMLInputElement>("root");
const targetInput = $<HTMLInputElement>("target");
const tree = $("tree");
const runner = $("runner");
const empty = $("empty");
const context = $<HTMLTextAreaElement>("context");
const runButton = $<HTMLButtonElement>("run");
const configuration = $("configuration");
let agents: Agent[] = [];
let selected: Agent | null = null;
let selectedCli: CliName = "codex";
let toastTimer = 0;
let settingsOpen = false;
let cliRenderId = 0;
const collapsedFolders = new Set<string>();
try {
  for (const path of JSON.parse(localStorage.getItem("agency.runner.collapsed") ?? "[]") as unknown[]) {
    if (typeof path === "string") collapsedFolders.add(path);
  }
} catch { localStorage.removeItem("agency.runner.collapsed"); }

const join = (base: string, name: string) => `${base.replace(/\/$/, "")}/${name}`;
const basename = (path: string) => path.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? path;
const escapeHtml = (value: string) => value.replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const showToast = (message: string) => {
  const toast = $("toast"); toast.textContent = message; toast.classList.add("visible");
  clearTimeout(toastTimer); toastTimer = window.setTimeout(() => toast.classList.remove("visible"), 2200);
};

async function scan(folder: string, relative = ""): Promise<Agent[]> {
  const result: Agent[] = [];
  const entries = await readdir(folder, { withFileTypes: true });
  entries.sort((a, b) => Number(b.isDirectory()) - Number(a.isDirectory()) || a.name.localeCompare(b.name));
  for (const entry of entries) {
    if (entry.name.startsWith(".") || entry.name === "node_modules" || entry.name === "dist") continue;
    const path = join(folder, entry.name);
    const rel = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await scan(path, rel));
    else if (entry.isFile() && entry.name.toLowerCase().endsWith(".md")) {
      const [content, info] = await Promise.all([readFile(path, "utf8"), stat(path)]);
      result.push({ name: entry.name.replace(/\.md$/i, ""), path, relative: rel, folder: relative, content, mtime: info.mtimeMs });
    }
  }
  return result;
}

function renderTree() {
  tree.innerHTML = "";
  if (!agents.length) { tree.innerHTML = '<div class="tree-empty">No Markdown agents found<br>Choose a different agent root</div>'; return; }

  const root: AgentBranch = { folders: new Map(), agents: [] };
  for (const agent of agents) {
    let branch = root;
    for (const part of agent.folder.split("/").filter(Boolean)) {
      let childBranch = branch.folders.get(part);
      if (!childBranch) {
        childBranch = { folders: new Map(), agents: [] };
        branch.folders.set(part, childBranch);
      }
      branch = childBranch;
    }
    branch.agents.push(agent);
  }

  const renderBranch = (branch: AgentBranch, depth: number, parentPath = "") => {
    for (const [name, childBranch] of branch.folders) {
      const path = parentPath ? `${parentPath}/${name}` : name;
      const collapsed = collapsedFolders.has(path);
      const folder = document.createElement("button");
      folder.className = "tree-row folder";
      folder.style.paddingLeft = `${8 + depth * 13}px`;
      folder.title = name;
      folder.setAttribute("aria-expanded", String(!collapsed));
      folder.innerHTML = `<span class="glyph">${collapsed ? "›" : "⌄"}</span><span class="tree-label">${escapeHtml(name)}</span>`;
      folder.addEventListener("click", () => {
        if (collapsedFolders.has(path)) collapsedFolders.delete(path); else collapsedFolders.add(path);
        localStorage.setItem("agency.runner.collapsed", JSON.stringify([...collapsedFolders]));
        renderTree();
      });
      tree.append(folder);
      if (!collapsed) renderBranch(childBranch, depth + 1, path);
    }

    for (const agent of branch.agents) {
      const button = document.createElement("button");
      button.className = `tree-row${selected?.path === agent.path ? " selected" : ""}`;
      button.style.paddingLeft = `${8 + depth * 13}px`;
      button.title = agent.relative;
      button.setAttribute("aria-current", selected?.path === agent.path ? "page" : "false");
      button.innerHTML = `<span class="glyph file">◇</span><span class="tree-label">${escapeHtml(agent.name)}</span>`;
      button.addEventListener("click", () => selectAgent(agent));
      tree.append(button);
    }
  };
  renderBranch(root, 0);
}

function selectAgent(agent: Agent) {
  selected = agent; renderTree(); showSettings(false);
  $("breadcrumb").textContent = agent.relative.replace(/\//g, "  /  ");
  $("agent-name").textContent = agent.name;
  $("agent-path").textContent = agent.path;
  $("agent-lines").textContent = `${agent.content.split(/\r?\n/).length} LINES`;
  context.focus();
}

async function loadAgents(quiet = false) {
  const root = rootInput.value.trim();
  if (!root) return;
  try {
    const next = await scan(root);
    const changed = JSON.stringify(next.map(x => [x.path, x.mtime])) !== JSON.stringify(agents.map(x => [x.path, x.mtime]));
    agents = next;
    if (selected) selected = agents.find(agent => agent.path === selected?.path) ?? null;
    renderTree();
    if (!quiet) showToast(`${agents.length} agent${agents.length === 1 ? "" : "s"} loaded`);
    else if (changed) showToast("Agent files updated");
  } catch (error) { if (!quiet) showToast(error instanceof Error ? error.message : "Could not read agent root"); }
}

function showSettings(show: boolean) {
  settingsOpen = show;
  configuration.classList.toggle("hidden", !show);
  empty.classList.toggle("hidden", show || Boolean(selected));
  runner.classList.toggle("hidden", show || !selected);
  $("breadcrumb").textContent = show ? "Configuration  /  Agent library" : selected?.relative.replace(/\//g, "  /  ") ?? "Select an agent";
  $("open-settings").classList.toggle("active", show);
  if (show) rootInput.focus();
}

async function pickRoot() {
  if (!dialog.openFolder) {
    showToast("Folder picker is unavailable; enter a path directly");
    return;
  }
  try {
    const folder = await dialog.openFolder({ title: "Choose agent root", directory: rootInput.value.trim() || undefined });
    if (!folder) return;
    rootInput.value = folder;
    localStorage.setItem("agency.runner.root", folder);
    await Promise.all([loadAgents(), renderClis()]);
  } catch (error) {
    showToast(error instanceof Error ? error.message : "Could not open folder picker");
  }
}

async function pickTarget() {
  if (!dialog.openFolder) {
    showToast("Folder picker is unavailable; enter a path directly");
    return;
  }
  try {
    const folder = await dialog.openFolder({ title: "Choose target directory", directory: targetInput.value.trim() || undefined });
    if (!folder) return;
    targetInput.value = folder;
    localStorage.setItem("agency.runner.target", folder);
  } catch (error) {
    showToast(error instanceof Error ? error.message : "Could not open folder picker");
  }
}

async function executableExists(name: CliName): Promise<boolean> {
  return new Promise(resolve => {
    const probe = spawn(name, ["--version"], { cwd: rootInput.value, env: process.env, stdio: ["ignore", "pipe", "pipe"] });
    let settled = false;
    const timeout = window.setTimeout(() => { probe.kill("SIGTERM"); done(false); }, 3000);
    const done = (value: boolean) => {
      if (!settled) { settled = true; clearTimeout(timeout); resolve(value); }
    };
    probe.on("error", () => done(false)); probe.on("close", code => done(code === 0));
  });
}

async function renderClis() {
  const renderId = ++cliRenderId;
  const names: CliName[] = ["codex", "claude"];
  const options = $("cli-options");
  options.replaceChildren();
  const buttons = names.map(name => {
    const button = document.createElement("button");
    const logo = document.createElement("span");
    const copy = document.createElement("span");
    const status = document.createElement("small");

    button.className = "cli-option";
    button.disabled = true;
    logo.className = "cli-logo";
    logo.textContent = name === "codex" ? "⌬" : "✣";
    copy.className = "cli-copy";
    copy.append(name.toUpperCase(), status);
    status.textContent = "○ CHECKING";
    button.append(logo, copy);
    options.append(button);
    return button;
  });

  const availability = await Promise.all((["codex", "claude"] as CliName[]).map(executableExists));
  if (renderId !== cliRenderId) return;
  if (!availability[0] && availability[1]) selectedCli = "claude";
  names.forEach((name, index) => {
    const button = buttons[index];
    const available = availability[index];
    button.className = `cli-option${selectedCli === name && available ? " selected" : ""}`;
    button.disabled = !available;
    const status = button.querySelector("small")!;
    status.className = available ? "available" : "";
    status.textContent = available ? "● INSTALLED" : "○ NOT FOUND";
    button.addEventListener("click", () => { selectedCli = name; renderClis(); });
  });
  runButton.disabled = !availability.some(Boolean);
}

type TerminalCommand = { command: string; args: string[] };

function terminalCommands(title: string, command: string, args: string[]): TerminalCommand[] {
  const configured = process.env.TERMINAL?.trim();
  const commands: TerminalCommand[] = [];
  if (configured && !configured.includes(" ")) commands.push({ command: configured, args: ["-T", title, "-e", command, ...args] });
  commands.push(
    { command: "xfce4-terminal", args: ["--disable-server", "--title", title, "--execute", command, ...args] },
    { command: "gnome-terminal", args: ["--title", title, "--", command, ...args] },
    { command: "konsole", args: ["-p", `tabtitle=${title}`, "-e", command, ...args] },
    { command: "x-terminal-emulator", args: ["-T", title, "-e", command, ...args] },
    { command: "xterm", args: ["-T", title, "-e", command, ...args] },
  );
  return commands;
}

function openInTerminal(commands: TerminalCommand[], cwd: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const tryNext = (index: number) => {
      if (index === commands.length) {
        reject(new Error("No supported OS terminal was found"));
        return;
      }
      const candidate = commands[index];
      const terminal = spawn(candidate.command, candidate.args, { cwd, env: process.env, stdio: "ignore", detached: true });
      terminal.on("spawn", () => { terminal.unref(); resolve(); });
      terminal.on("error", () => tryNext(index + 1));
    };
    tryNext(0);
  });
}

async function runAgent() {
  if (!selected) return;
  const target = targetInput.value.trim();
  if (!target) { showToast("Choose a target directory first"); targetInput.focus(); return; }
  const userContext = context.value.trim();
  const prompt = userContext
    ? `${selected.content}\n\n---\n\nExecution context:\n${userContext}`
    : selected.content;
  localStorage.setItem("agency.runner.target", target);
  runButton.disabled = true;
  try {
    await openInTerminal(terminalCommands(`${selectedCli} · ${selected.name}`, selectedCli, [prompt]), target);
    showToast(`Opened ${selected.name} in the OS terminal`);
  } catch (error) {
    showToast(error instanceof Error ? error.message : "Could not open an OS terminal");
  } finally {
    runButton.disabled = false;
  }
}

rootInput.value = localStorage.getItem("agency.runner.root") ?? `${process.env.HOME ?? "."}/.agents`;
targetInput.value = localStorage.getItem("agency.runner.target") ?? process.env.PWD ?? ".";
rootInput.addEventListener("keydown", event => {
  if (event.key === "Enter") { event.preventDefault(); $("load-root").click(); }
});
$("load-root").addEventListener("click", async () => {
  localStorage.setItem("agency.runner.root", rootInput.value.trim());
  await Promise.all([loadAgents(), renderClis()]);
  showSettings(false);
});
$("pick-root").addEventListener("click", pickRoot);
$("pick-target").addEventListener("click", pickTarget);
$("open-settings").addEventListener("click", () => showSettings(!settingsOpen));
$("close-settings").addEventListener("click", () => showSettings(false));
$("refresh").addEventListener("click", () => loadAgents());
context.addEventListener("input", () => { $("characters").textContent = `${context.value.length} characters`; });
runButton.addEventListener("click", runAgent);
addEventListener("keydown", event => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); runAgent(); }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "r") { event.preventDefault(); loadAgents(); }
});

void Promise.all([loadAgents(), renderClis()]);
window.setInterval(() => loadAgents(true), 2500);
