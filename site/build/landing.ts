// The home page is a short route into the task-based documentation. Product
// contributor records remain in the repository; they are deliberately not part
// of this path.

import { BASE, GROUPS, REPO } from "./content.ts";

const INSTALL = `npm install -D blitsen`;

const CONFIG = `{
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}`;

const COMMANDS = `npx blitsen                       # build and open a native window
npx blitsen doctor dist           # check the built application
npx blitsen build                 # create a desktop executable`;

function docCards(): string {
  return GROUPS.map((group) => {
    const items = group.pages.map((page) =>
      `<li><a href="${BASE}/docs/${page.slug}/"><span class="card-title">${page.title}</span>` +
      `<span class="card-blurb">${page.blurb}</span></a></li>`).join("");
    return `<section class="doc-group">
<h3>${group.name}</h3><p class="group-note">${group.note}</p>
<ul class="card-list">${items}</ul></section>`;
  }).join("");
}

export function landingBody(crestSvg: string): string {
  const hero = crestSvg
    .replace(/\swidth="\d+"/, "")
    .replace(/\sheight="\d+"/, "")
    .replace(/\srole="img"/, "")
    .replace(/\saria-label="[^"]*"/, "")
    .replace(/<title>.*?<\/title>/s, "")
    .replace("<svg", '<svg class="hero-crest" aria-hidden="true" focusable="false"');

  return `
<section class="hero">
  <div>
    <div class="hero-title">
      ${hero}
      <div>
        <p class="eyebrow">Pre-alpha · v0.1.0</p>
        <h1>Blitsen</h1>
      </div>
    </div>
    <p class="lede">
      Write an app in HTML, CSS and TypeScript. Ship a native executable.
      <strong>No browser included.</strong>
    </p>
    <p class="sub">
      Keep your framework and build tool. Blitsen opens their static output in a
      native window and packages it with its own renderer and JavaScript runtime.
    </p>
    <div class="cta">
      <a class="btn primary" href="${BASE}/docs/getting-started/">Get started</a>
      <a class="text-link" href="${BASE}/docs/platform-support/">Check platform support →</a>
    </div>
  </div>
  <aside class="quickstart" aria-labelledby="quickstart-title">
    <p class="eyebrow" id="quickstart-title">Install from npm</p>
    <figure class="code hero-code"><span class="code-lang">sh</span><pre><code>${INSTALL}</code></pre></figure>
    <p>
      The package downloads the runtime for the current desktop target. No Rust
      toolchain or post-install compilation is required.
    </p>
    <a href="${BASE}/docs/getting-started/">Run your first application →</a>
  </aside>
</section>

<section class="band">
  <p class="section-index" aria-hidden="true">01 / Configure</p>
  <h2>Point Blitsen at built web output</h2>
  <p class="band-lede">
    Add one object to <code>package.json</code>. The build command stays yours;
    Blitsen consumes the directory it leaves behind.
  </p>
  <figure class="code"><span class="code-lang">json</span><pre><code>${CONFIG}</code></pre></figure>
  <p class="band-note">
    Plain HTML needs no configuration: <code>npx blitsen .</code> opens the directory
    containing <code>index.html</code>.
  </p>
</section>

<section class="band">
  <p class="section-index" aria-hidden="true">02 / Run and build</p>
  <h2>Use the same output from first window to release</h2>
  <figure class="code"><span class="code-lang">sh</span><pre><code>${COMMANDS}</code></pre></figure>
  <p class="band-note">
    During development, point Blitsen at a running server such as
    <code>http://localhost:5173</code> to keep its transforms and hot reload.
    Before release, run <code>doctor</code> against static output and test the exported
    artifact on every target platform.
  </p>
</section>

<section class="band">
  <p class="section-index" aria-hidden="true">03 / Know the boundary</p>
  <h2>A native runtime, not a general-purpose browser</h2>
  <ol class="boundary-list">
    <li>
      <span class="item-index" aria-hidden="true">01</span>
      <div><h3>Use built output</h3><p>Blitsen does not transpile source files or resolve bare npm imports. Vite, webpack or your existing tool must do that first.</p></div>
    </li>
    <li>
      <span class="item-index" aria-hidden="true">02</span>
      <div><h3>Check compatibility</h3><p>The runtime implements a deliberate subset of browser APIs. Missing features are absent so feature detection works, and <code>doctor</code> reports what it can see.</p></div>
    </li>
    <li>
      <span class="item-index" aria-hidden="true">03</span>
      <div><h3>Treat it as native software</h3><p>There is no browser sandbox, same-origin policy or permission prompt. Run only application code and content you trust.</p></div>
    </li>
  </ol>
</section>

<section class="band">
  <p class="section-index" aria-hidden="true">04 / Reach the OS</p>
  <h2>Use native capabilities when the web has no answer</h2>
  <p class="band-lede">
    Package imports expose window controls, dialogs, clipboard formats,
    application directories and live operating-system readings.
  </p>
  <figure class="code"><span class="code-lang">js</span><pre><code>import windowApi from "blitsen/window";

addEventListener("load", () =&gt; {
  windowApi.setSize?.(1024, 720);
});</code></pre></figure>
  <p class="band-note">
    Support varies by version and target, so native members are optional and
    feature detection is part of the API. <a href="${BASE}/docs/native-apis/">Browse native APIs →</a>
  </p>
</section>

<section class="band">
  <p class="section-index" aria-hidden="true">05 / Distribute</p>
  <h2>Build for desktop or Android</h2>
  <p class="band-lede">
    Desktop exports embed reachable assets in one executable by default. Add
    platform metadata, connect your signing command, or cross-build for any of
    the six published desktop targets. Android output is an APK built from a
    source checkout.
  </p>
  <p class="band-note">
    <a href="${BASE}/docs/packaging/">Packaging and release checklist →</a>
  </p>
</section>

<section class="band docs-index" id="documentation">
  <p class="section-index" aria-hidden="true">06 / Documentation</p>
  <h2>Find the task you need</h2>
  <p class="band-lede">
    Choose a task to get from static web output to a tested native artifact.
    Each guide is maintained in the
    <a href="${REPO}/tree/main/docs" target="_blank" rel="noopener noreferrer">repository</a>.
  </p>
  ${docCards()}
</section>
`;
}
