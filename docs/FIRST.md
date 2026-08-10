bun was rewritten in rust. 
DioxusLabs/blitznatively renders html/css and is written in rust but only supports "rsx" a rust hybrid for logic which is where bun would come in. With full html, css and js support, we could make a native game engine using those languages right?
Worked for 45s

Yes. This is now a genuinely plausible architecture, and there’s an especially nice way to structure it that avoids having to “embed Bun into Rust.”

As of August 2026, Bun really has been rewritten in Rust, while JavaScript execution still comes from JavaScriptCore. Bun’s current README describes the runtime as Rust + JavaScriptCore, and the July rewrite post explains the migration.

Blitz is also closer to what you want than “RSX-only” suggests. It already contains an actual HTML parser, CSS engine, DOM, layout engine, window integration and renderer. Its plain blitz frontend can take an HTML string directly; the limitation is that this route currently lacks interactivity. Dioxus/RSX is currently what supplies the interactive VirtualDOM/event side. Blitz explicitly says it doesn't yet have JavaScript bindings.

So effectively, the missing project is:

             index.html
                 │
        ┌────────▼─────────┐
        │      Blitz       │
        │ HTML / CSS / DOM │
        │ Stylo + Taffy    │
        └────────┬─────────┘
                 │
             DOM bridge
                 │
        ┌────────▼─────────┐
        │    JavaScript    │
        │       Bun        │
        │ JavaScriptCore   │
        └────────┬─────────┘
                 │
           Native API
                 │
    ┌────────────▼────────────┐
    │      Rust engine        │
    │                         │
    │ wgpu / audio / input    │
    │ physics / networking    │
    │ assets / ECS / etc.     │
    └─────────────────────────┘

And yes: the end result could absolutely be a native game engine where the developer writes HTML, CSS and JavaScript.

The clever part is that I wouldn't make Rust launch/embed Bun initially. I'd reverse it:

import { Engine } from "@whatever/native";

const game = new Engine();

game.loadHTML("./index.html");

game.on("frame", dt => {
    player.rotation.y += dt;
});

@whatever/native would be a Rust Node-API module loaded directly into Bun. Bun currently implements most of Node-API, and its docs specifically recommend Node-API over bun:ffi for production native integration; its raw FFI API is still marked experimental.

That means Bun and the Rust engine can actually live in the same process.

Even better, Bun already supports bundling native .node addons into compiled standalone executables. So eventually:

bun build --compile game.js --outfile mygame

could theoretically produce something approaching:

mygame.exe
    Bun / JavaScriptCore
    Rust game engine
    Blitz HTML/CSS renderer
    JS game
    assets

without Chromium, Electron or an OS WebView.

Where I'd draw the boundary

I wouldn't try to turn HTML elements themselves into the entire 3D rendering engine. That's where it would become awkward.

Instead I'd make HTML/CSS the application/game UI and declarative scene interface, while Rust owns high-performance rendering.

For example:

<body>
    <game>
        <camera id="camera" />

        <entity id="player">
            <mesh src="player.glb" />
        </entity>

        <entity id="enemy">
            <mesh src="enemy.glb" />
        </entity>
    </game>

    <div id="hud">
        <progress id="health" max="100" value="100"></progress>
        <span id="ammo">30</span>
    </div>

    <script src="game.js"></script>
</body>

CSS:

#hud {
    position: fixed;
    left: 32px;
    bottom: 32px;
    display: flex;
    gap: 12px;
}

#enemy {
    visibility: hidden;
}

JS:

const player = document.querySelector("#player");
const enemy = document.querySelector("#enemy");

player.addEventListener("collision", event => {
    console.log("hit", event.other);
});

engine.onFrame(dt => {
    player.position.z -= 5 * dt;
});

Blitz already explicitly wants custom widgets/extensibility, which makes custom elements like a native game viewport conceptually fit its design quite well.

But I think an even nicer API would avoid abusing CSS for real-time 3D state:

const player = engine.scene.get("#player");

player.position.x += input.axis("horizontal") * dt;
player.rotateY(dt);

while keeping ordinary DOM stuff exactly as expected:

document.querySelector("#health").value = player.health;
document.querySelector("#score").textContent = score;

So you essentially have two scene graphs:

Game scene                     UI DOM

Rust ECS / scene graph         Blitz DOM
       │                           │
       ├─ transforms               ├─ div
       ├─ meshes                   ├─ button
       ├─ lights                   ├─ input
       ├─ physics                  ├─ text
       └─ particles                └─ CSS
             │                         │
             └──────── wgpu ──────────┘
                         │
                      Window

That's much more likely to scale to a proper game.

The really interesting bit: implement browser APIs

This is where the project could become extremely cool.

You could expose game-engine functionality through familiar browser-ish APIs:

window.requestAnimationFrame(update);

window.addEventListener("keydown", ...);

const texture = new Image();
texture.src = "./grass.png";

const audio = new Audio("./shot.ogg");
audio.play();

fetch("/levels/one.json");

document.querySelector(...);

Then add engine APIs:

navigator.gamepads

engine.physics
engine.scene
engine.audio
engine.assets
engine.network

Eventually you could potentially implement subsets of:

DOM
CSSOM
Events
requestAnimationFrame
Fetch
WebSocket
Canvas
Gamepad API
Web Audio-ish API
WebGPU-ish API
Pointer Lock
Fullscreen

Blitz deliberately doesn't implement the whole browser platform — its authors say things such as WebSockets/localStorage/etc. should be supplied by regular Rust crates rather than bloating the renderer. That's almost perfect for this idea.

You aren't building a browser.

You're building:

a native runtime that happens to implement the useful parts of the web programming model.

And that distinction is huge.

And Bun becomes more than just JavaScript

Using Bun also gives the developer npm packages, ESM/CommonJS resolution, TypeScript/JSX support, a bundler, filesystem APIs and the broader Node ecosystem. Bun supports both ESM and CommonJS and transpiles TS/JSX itself.

So you could have:

<script type="module" src="./game.ts"></script>

with:

import { vec3 } from "gl-matrix";
import { createStore } from "zustand";

const position = vec3.create();

and npm dependencies just work.

That's something an embedded QuickJS-style scripting engine wouldn't give you nearly as easily.

The biggest obstacle is therefore not rendering anymore.

It's implementing this bridge:

JavaScriptCore
      ↕
 JS objects/events
      ↕
 Blitz DOM
      ↕
 Rust

You need enough of things like:

document
window
Element
Node
querySelector
addEventListener
style
classList
appendChild
remove
innerHTML
textContent

to make the DOM feel real.

Once that exists, Blitz handles:

HTML parsing
    ↓
DOM
    ↓
Stylo CSS
    ↓
Taffy layout
    ↓
painting
    ↓
native GPU window

while Bun handles:

JS / TS
npm
modules
async
timers
fetch
tooling
bundling

and Rust handles:

rendering
audio
physics
input
assets
threads
ECS
native platform

I think there's a real engine hiding in that combination. Not “Electron for games,” but almost a browser programming model rebuilt specifically for native games, with no Chromium and with the expensive parts implemented directly in Rust.

For an MVP, I'd go extraordinarily small: Blitz + Bun + document.querySelector() + click/keyboard events + requestAnimationFrame() + one native <canvas>/<game> element backed by wgpu. Once you can write a tiny Pong game entirely as index.html + style.css + game.js, you've proved the architecture.

Exactly. Then the project is not a game engine and not a framework with its own semantics.

It is a native runtime/export target for web applications:

HTML + CSS + JavaScript in, native executable out, with no browser bundled.

The runtime should make as few decisions as possible about what the application is. A game, dashboard, media app, editor, kiosk, visualization, launcher, or anything else is up to the developer.

The core boundary becomes:

Application
├── index.html
├── styles.css
├── app.js / app.ts
└── assets/
        │
        ▼
Native Web Runtime
├── HTML parser / DOM
├── CSS parser / layout / paint
├── JavaScript runtime
├── Web API compatibility layer
├── DOM ↔ JS bindings
├── native windowing
├── native rendering
└── native platform integration
        │
        ▼
app.exe / app / App.app

Blitz would provide much of the rendering side:

HTML
 ↓
DOM
 ↓
CSS
 ↓
layout
 ↓
paint
 ↓
native GPU output

Bun/JSC would provide the JavaScript side:

JavaScript / TypeScript
 ↓
modules
 ↓
npm ecosystem
 ↓
JavaScriptCore

Your project is principally the layer joining them:

             JavaScriptCore
                   │
            JS bindings
                   │
                   ▼
 window ───────► DOM ◄────── document
                   │
             event system
                   │
                   ▼
                 Blitz
                   │
                   ▼
            native renderer

The compatibility surface would be ordinary web APIs rather than engine-specific APIs:

document.querySelector()
document.createElement()

element.append()
element.remove()

element.style
element.classList

addEventListener()

requestAnimationFrame()

setTimeout()
setInterval()

fetch()

WebSocket

Image

Audio

localStorage

Implementation can be incremental. An API existing in browsers does not mean the runtime has to support it immediately.

For example:

v0
HTML
CSS
DOM
JS
events
requestAnimationFrame
mouse
keyboard

v1
fetch
WebSocket
images
fonts
audio

v2
storage
workers
clipboard
drag/drop
gamepads

later
Canvas
WebGL
WebGPU
WebRTC
etc.

And there should be no requirement that rendering happen through a particular abstraction.

Someone could build a game using:

<div class="player"></div>

and CSS transforms.

Someone else could bring in a Rust library through a native addon.

Someone could use Box2D compiled to WASM.

Someone could use pure JavaScript physics.

Someone could eventually use <canvas>.

Someone could use WebGPU if the runtime implements it.

The runtime shouldn't care.

Similarly npm stays useful because application code remains normal JS/TS:

import Matter from "matter-js";
import { Howl } from "howler";
import lodash from "lodash";

or potentially:

import nativePhysics from "./physics.node";

if Bun/native-addon compatibility is retained.

So there's a useful separation:

YOUR PROJECT
────────────────────────────

Web platform compatibility
Native execution
Native rendering
Native packaging


USER APPLICATION
────────────────────────────

Architecture
Libraries
Physics
Game engine
UI framework
State management
Networking
Rendering technique
Application type

The export process could ultimately be conceptually as simple as:

native-web build

taking:

src/
├── index.html
├── main.ts
├── style.css
└── package.json

and producing:

dist/
└── my-app.exe

Internally that executable contains roughly:

native runtime
+
JS runtime
+
Blitz
+
application bundle
+
assets

rather than:

Chromium
+
Node
+
application

That also gives the project a very clear definition:

A browserless implementation of enough of the web platform to run HTML/CSS/JS applications as native desktop executables.

A 2D game is then just one thing somebody can build with it.

what kind of size are we looking at for the exe?
Worked for 45s

For a Windows .exe, I’d expect roughly 50–120 MB initially, depending on how much of Bun you actually ship.

The crucial distinction is whether Bun is only the build tool, or whether the entire Bun runtime is embedded in every exported app.

Current Bun Windows x64 releases are about 38 MB compressed, and Bun’s own --compile mechanism explicitly works by embedding/copying the Bun runtime into the standalone executable. Historical/current reports put tiny Bun-compiled programs around roughly 60–100 MB uncompressed, because even “hello world” carries the runtime.

So a straightforward first implementation might look roughly like:

Full Bun/JSC runtime       ~60–100 MB
Blitz + Stylo + Taffy
+ window/rendering          ~5–15 MB
Your bridge/runtime         ~1–5 MB
HTML/CSS/JS                 tiny
────────────────────────────────────
Bare application          ~70–120 MB

Assets obviously come on top.

Blitz itself probably isn't the problem. Dioxus currently claims optimized native applications below roughly 5 MB, and its native renderer is Blitz-based.

But you don't necessarily need to ship Bun

This is the more interesting possibility for your exporter.

You can conceptually separate:

DEVELOPMENT / BUILD

Bun
├── TypeScript
├── bundler
├── npm resolution
├── transpilation
└── packaging

          ↓ export

SHIPPED APPLICATION

JavaScriptCore
Blitz
DOM bindings
Web APIs
window/rendering
compiled JS bundle
assets

The user doesn't need Bun's:

package manager
test runner
bundler
transpiler
CLI
dev server
installer
etc.

inside game.exe.

They only need JavaScript execution.

So if the project eventually builds a purpose-specific native runtime around JSC rather than embedding the entire Bun executable, I'd expect the achievable target to be more like:

JSC + required runtime      ~15–30 MB  ?
Blitz/native rendering       ~5–15 MB  ?
web compatibility layer      ~1–5 MB
application JS/CSS/HTML      usually <5 MB
───────────────────────────────────────
Potential bare export       ~25–50 MB

The ? is important: that's an engineering target, not a measured current build. We'd need to actually compile JSC + Blitz together to establish the floor.

And a compressed download could be substantially smaller:

Installed EXE       ~30–50 MB
ZIP/download        ~15–30 MB

versus an Electron app commonly carrying an entire Chromium/Node stack.

So I'd think of the milestones as:

First prototype
≈ 80–120 MB

↓ stop shipping all of Bun

Optimised runtime
≈ 30–50 MB

↓ aggressive LTO, stripping,
  feature gating, runtime trimming

Possible eventual target
≈ 20–40 MB

The key architectural point is therefore: use Bun to produce the application bundle, but don't necessarily make Bun itself part of the exported runtime. Bun can be your toolchain while JSC is the runtime. That keeps the browserless-native idea intact while giving you a realistic route to much smaller executables.

any handy functionality we can add to js to make it more than a sandbox?
Worked for 14s

Absolutely. And if you keep Bun itself as the JS environment, you're already quite far beyond a browser sandbox: Bun exposes filesystem access, child processes, TCP/UDP sockets, SQLite, workers, environment variables, memory mapping, FFI and native Node-API addons.

So your runtime could deliberately have two layers:

Web APIs                         Native APIs
────────────────────            ────────────────────
document                        filesystem
window                          processes
fetch                           sockets
WebSocket                       native libraries
requestAnimationFrame           OS integration
localStorage                    application lifecycle
...

For the extra native functionality, I'd expose things along these lines:

Filesystem — unrestricted or permission-controlled read/write, directories, file watching, temp files, app-data paths, executable path.
Processes — spawn programs, shell commands, stdin/stdout/stderr, environment variables, exit codes.
Native FFI — load .dll/.so/.dylib files and call C ABI functions. Bun already has bun:ffi, though it currently labels it experimental; Node-API is the more stable addon route.
Native addons — importantly, allow .node modules. That means users can write Rust/C/C++ extensions and import them from JavaScript. Node-API is designed to provide an ABI-stable native addon interface.
Native windows — create windows, resize them, fullscreen, borderless, always-on-top, transparency, cursor control, multiple monitors, DPI information.
System dialogs — open/save file pickers, folder pickers, message boxes.
Clipboard — text, images and arbitrary MIME data.
Drag and drop — importantly, actual filesystem paths rather than browser File abstractions where appropriate.
Notifications — native desktop notifications.
Tray/menu bar — tray icons, context menus, application menus.
Raw input — keyboard state, mouse state, controllers/gamepads, potentially raw HID input.
Networking — TCP, UDP and listening sockets, not merely HTTP/WebSocket. Bun already exposes TCP and UDP primitives.
Local servers — let an application start HTTP/WebSocket servers directly.
Database access — SQLite is particularly handy and Bun already exposes it natively.
Threads/workers — real background computation and possibly shared memory. Bun supports workers, while the wider Node ecosystem already has established patterns for native threaded addons.
OS information — CPU, memory, displays, username, platform, architecture, battery, locale.
App lifecycle — startup arguments, single-instance locking, restart, quit, suspend/resume events.
File associations / protocols — myapp://foo, opening associated file types, handling command-line launches.
Dynamic libraries/plugins — native plugin loading at runtime.
Memory-mapped files / buffers — useful for large assets, IPC and specialist applications; Bun already exposes mmap.

I think a clean API would look more like a native standard library than framework APIs:

import { openFile } from "native:dialog";
import { clipboard } from "native:clipboard";
import { Window } from "native:window";
import { app } from "native:app";

const path = await openFile();

await clipboard.writeText("hello");

const secondWindow = new Window({
    width: 800,
    height: 600,
    html: "./tools.html"
});

console.log(app.executablePath);

While generic OS functionality could stay compatible with existing Node/Bun modules:

import fs from "node:fs";
import { spawn } from "node:child_process";

const config = fs.readFileSync("./config.json", "utf8");

spawn("ffmpeg", [
    "-i", "input.mp4",
    "output.webm"
]);

And then the really powerful escape hatch is:

import physics from "./box2d.node";
import emulator from "./emulator.node";
import database from "./database.node";
import whatever from "./whatever.node";

That fits your earlier point particularly well: your project doesn't need to decide what functionality developers should have. Give them JS + DOM/CSS + OS access + a stable native-extension mechanism, and they can bolt on whatever they want.

You could therefore describe the capability difference quite simply:

Browser
HTML + CSS + JS
        │
        ▼
   sandbox boundary
        ✕

Your runtime
HTML + CSS + JS
        │
        ├── OS
        ├── filesystem
        ├── processes
        ├── sockets
        ├── native libraries
        └── Rust/C/C++ addons

At that point it starts feeling less like a stripped-down browser and more like a native application runtime that happens to use the web platform as its UI/rendering environment.
