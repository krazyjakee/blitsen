// The home page. This is the one hand-authored page on the site.
//
// Every number and claim below is transcribed from README.md or the docs it links,
// which are themselves gated on measurements. Nothing here is rounded, restated
// from memory, or inferred — if a figure changes in the repo it must be changed
// here too, which is why the figures are kept few and each one is attributed.

import { BASE, GROUPS, REPO } from "./content.ts";

const INSTALL = `npm install -D blitsen`;

const COMMANDS = `npx blitsen .                       # open index.html in a native window
npx blitsen http://localhost:5173   # point at a running dev server
npx blitsen doctor dist             # check output against the compatibility profile
npx blitsen build                   # one executable: runtime + application`;

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
    .replace("<svg", '<svg class="hero-crest" aria-hidden="true"');

  return `
<section class="hero">
  ${hero}
  <p class="eyebrow">Pre-alpha · Linux, macOS and Windows</p>
  <h1>Blitsen</h1>
  <p class="lede">
    Write an app in HTML, CSS and TypeScript. Ship a native executable.
    <strong>No browser included.</strong>
  </p>
  <p class="sub">
    Blitsen hosts a JavaScript engine directly and pairs it with
    <a href="https://github.com/DioxusLabs/blitz" target="_blank" rel="noopener noreferrer">Blitz</a>'s
    native HTML/CSS renderer. It does not embed Chromium, and it does not use the
    operating system's WebView.
  </p>
  <div class="cta">
    <a class="btn primary" href="${BASE}/docs/">Read the documentation</a>
    <a class="btn" href="${REPO}" target="_blank" rel="noopener noreferrer">Source on GitHub</a>
  </div>
  <figure class="code hero-code"><span class="code-lang">sh</span><pre><code>${INSTALL}</code></pre></figure>
  <p class="fineprint">
    A dev dependency that acts as a native export toolchain. Your project's shape,
    bundler and framework are not prescribed.
  </p>
</section>

<section class="band">
  <h2>Four commands</h2>
  <figure class="code"><span class="code-lang">sh</span><pre><code>${COMMANDS}</code></pre></figure>
  <p class="band-note">
    With no directory, running and building both read the <code>blitsen</code> config in
    <code>package.json</code>, run the configured build command and take its output
    directory. <code>--target</code> builds for another platform, fetching and caching
    its runtime.
  </p>
</section>

<section class="band negative">
  <h2>What it is not</h2>
  <p class="band-lede">
    Blitsen is not a browser and does not aspire to be one. Three consequences,
    none of them accidental.
  </p>
  <div class="grid-3">
    <article>
      <h3>It renders less of the web than a browser does</h3>
      <p>
        That is the trade: a renderer you ship and control, at a size that is not
        absurd, against specification coverage. An application targeting Blitsen is
        authored against Blitsen, not ported blind from the web.
      </p>
      <p class="more"><a href="${BASE}/docs/compatibility/">The boundary, published as capability tiers →</a></p>
    </article>
    <article>
      <h3>There is no sandbox by default</h3>
      <p>
        An application is trusted native software, not an untrusted document.
        No same-origin policy, no permission prompts.
      </p>
    </article>
    <article>
      <h3>It is not for third-party web content</h3>
      <p>
        Rendering arbitrary pages you did not write is a browser engine's job.
        Use one for that.
      </p>
    </article>
  </div>
</section>

<section class="band">
  <h2>An unimplemented API is <em>absent</em></h2>
  <p class="band-lede">
    The property does not exist, so feature detection works. Never a stub that
    resolves to nothing, never a silent no-op.
  </p>
  <p class="band-note">
    That is enforced rather than reviewed. The API manifest is parsed out of the
    runtime source, and a test asserts every API the manifest calls absent is
    genuinely <code>undefined</code> against a real bridge context. The
    <a href="${BASE}/docs/compatibility/">compatibility profile</a> is generated from
    the runtime rather than hand-maintained, and <code>blitsen doctor</code> reports
    what a bundle uses that the runtime lacks before you hit it at runtime.
  </p>
</section>

<section class="band measured">
  <h2>Measured, not estimated</h2>
  <p class="band-lede">
    Every size figure in this project comes from a measured build. The tracked
    baseline lives in the repository and CI fails on growth beyond 2%.
  </p>

  <div class="table-scroll">
    <table>
      <caption>
        A release-build “hello” window on Ubuntu x64 (Ryzen 9 5900X, X11), against
        Electron 43.4.0 and Tauri 2.11.5.
      </caption>
      <thead>
        <tr><th>Runtime</th><th class="num">Disk</th><th class="num">Idle CPU</th>
        <th class="num">Idle memory (PSS)</th></tr>
      </thead>
      <tbody>
        <tr><td>Electron</td><td class="num">339.4 MB</td><td class="num">0.2%</td><td class="num">284.3 MB</td></tr>
        <tr><td>Tauri</td><td class="num">4.7 MB</td><td class="num">&lt;0.1%</td><td class="num">191.9 MB</td></tr>
        <tr class="mine"><td>Blitsen</td><td class="num">38.8 MB</td><td class="num">0.1%</td><td class="num">101.0 MB</td></tr>
      </tbody>
    </table>
  </div>
  <p class="fineprint">
    Medians of five runs after a five-second warm-up. CPU is the whole process tree
    over ten seconds, where 100% is one core; disk is the packaged app's apparent
    size. Tauri's figures include its host and WebKit processes, and its disk figure
    excludes the system WebKitGTK it uses. Electron ships Chromium; Blitsen ships its
    renderer and JavaScript engine.
  </p>

  <div class="stats">
    <div class="stat">
      <span class="stat-n">38.1 MB</span>
      <span class="stat-l">Standalone Pong export, down from 144.7 MB once the runtime
      replaced a bundled copy of Bun. The whole download — the JavaScript engine is
      statically linked, not shipped beside it.</span>
    </div>
    <div class="stat">
      <span class="stat-n">0.809 ms</span>
      <span class="stat-l">Median frame cost against a 16.7 ms budget, with the
      windowed export sustaining 60 fps.
      <a href="${BASE}/docs/m3/">M3 evidence →</a></span>
    </div>
    <div class="stat">
      <span class="stat-n">6 apps</span>
      <span class="stat-l">Written by other people — React, Vue 3, Svelte and the three
      stock <code>create-vite</code> templates — rendered from their own unmodified
      <code>vite build</code> output. All six failed when first measured.
      <a href="${BASE}/docs/m3b/">M3b evidence →</a></span>
    </div>
  </div>
</section>

<section class="band showcase">
  <h2>Rendering real applications</h2>
  <figure class="shot">
    <img src="${BASE}/assets/shadcn-admin.png" alt="Shadcn Admin rendered by Blitsen"
      loading="lazy" decoding="async">
    <figcaption>
      <a href="https://github.com/satnaing/shadcn-admin" target="_blank" rel="noopener noreferrer">Shadcn
      Admin</a> (MIT), unmodified, rendered without a browser engine. The empty chart
      panel is Recharts SVG —
      <a href="https://github.com/DioxusLabs/blitz/issues/448" target="_blank" rel="noopener noreferrer">tracked
      upstream</a>.
    </figcaption>
  </figure>
  <figure class="shot">
    <img src="${BASE}/assets/pong.gif" alt="Pong running in Blitsen" loading="lazy" decoding="async">
    <figcaption>
      Every frame is HTML and CSS laid out by Blitz and mutated from JavaScript — the
      paddles, the ball and the scoreboard are ordinary DOM nodes. The recording comes
      from the same harness the acceptance gate asserts on, so it cannot drift from
      what the tests verify.
    </figcaption>
  </figure>
</section>

<section class="band">
  <h2>Past what a browser can answer</h2>
  <p class="band-lede">
    <code>examples/hardware</code> is a CPU-Z-shaped report on the machine it is running
    on: processor and per-thread load, memory and swap, every mounted volume, kernel
    and boot time.
  </p>
  <p class="band-note">
    None of it has a web spelling — the closest the platform comes is
    <code>navigator.hardwareConcurrency</code>, one deliberately coarsened number. It is
    read through <a href="${BASE}/docs/compatibility/#native-modules"><code>blitsen/os</code></a>,
    and it is three files with no build step.
  </p>
</section>

<section class="band docs-index" id="documentation">
  <h2>Documentation</h2>
  <p class="band-lede">
    Generated from the markdown in the repository, so it cannot drift from the source.
  </p>
  ${docCards()}
</section>
`;
}
