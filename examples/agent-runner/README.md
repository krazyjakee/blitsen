# Agency Runner

A TypeScript Blitsen example for browsing local Markdown agents and running them with Codex or
Claude. It scans the selected root recursively, hot reloads changed agent files, detects installed
CLIs, and opens each interactive run in the desktop's OS terminal.

Agent files can live at any depth beneath the root and must use a `.md` extension.

```text
~/.agents/
├── engineering/
│   └── reviewer.md
└── writer.md
```

## Build and run

```sh
npm install
npm run build
blitsen dist
```

Or, with Blitsen installed and available on `PATH`:

```sh
npm run desktop
```

The app invokes the selected CLI in interactive mode inside a desktop terminal. The selected agent
file and execution context are combined into the initial prompt, and the terminal opens in the
chosen target directory. On Linux it supports `$TERMINAL` (when it is a bare executable name),
XFCE Terminal, GNOME Terminal, Konsole, `x-terminal-emulator`, and xterm.
