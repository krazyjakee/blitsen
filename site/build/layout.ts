// The page shell: head, masthead, sidebar, table of contents, footer.
//
// The visual language is inherited from docs/product.html — paper and ink, an
// ultramarine accent, a serif measure with mono display type. The crest is the
// rampant reindeer from assets/brand, inlined so it takes the surrounding ink
// colour and needs no second file for dark mode.

import { BASE, GROUPS, REPO, type DocPage } from "./content.ts";
import type { Heading } from "./markdown.ts";
import { escapeHtml } from "./markdown.ts";

export interface PageOptions {
  title: string;
  description: string;
  /** Site-relative path with trailing slash, e.g. "/docs/tech/". "" for the home page. */
  path: string;
  body: string;
  /** Rendered with the documentation rail and reading measure. */
  chrome?: "docs" | "bare";
  headings?: Heading[];
  /** docs/ file this page was generated from, for the "edit this page" link. */
  sourceFile?: string;
  activeSlug?: string;
}

/** Loaded once at build time and inlined into every page. */
let crest = "";
export function setCrest(svg: string): void {
  // Strip the standalone-document bits; it is being embedded, not served.
  crest = svg
    .replace(/<\?xml[^>]*\?>/g, "")
    .replace(/\swidth="\d+"/, "")
    .replace(/\sheight="\d+"/, "")
    .replace(/<title>.*?<\/title>/s, "")
    .trim();
}

function crestMark(className: string): string {
  return crest.replace("<svg", `<svg class="${className}" aria-hidden="true" focusable="false"`);
}

const THEME_SCRIPT = `(function(){try{var t=localStorage.getItem("blitsen-theme");
if(t){document.documentElement.setAttribute("data-theme",t)}}catch(e){}})()`;

const TOGGLE_SCRIPT = `document.addEventListener("click",function(e){
var b=e.target.closest("[data-theme-toggle]");if(!b)return;
var r=document.documentElement;
var dark=r.getAttribute("data-theme")==="dark"||
(!r.getAttribute("data-theme")&&matchMedia("(prefers-color-scheme:dark)").matches);
var next=dark?"light":"dark";r.setAttribute("data-theme",next);
try{localStorage.setItem("blitsen-theme",next)}catch(err){}
b.setAttribute("aria-pressed",String(next==="dark"));
b.setAttribute("aria-label",next==="dark"?"Use light theme":"Use dark theme")});
document.addEventListener("click",function(e){
var n=e.target.closest("[data-nav-toggle]");if(!n)return;
var open=document.body.classList.toggle("nav-open");
n.setAttribute("aria-expanded",String(open))});
(function(){var b=document.querySelector("[data-theme-toggle]");if(!b)return;
var r=document.documentElement;var dark=r.getAttribute("data-theme")==="dark"||
(!r.getAttribute("data-theme")&&matchMedia("(prefers-color-scheme:dark)").matches);
b.setAttribute("aria-pressed",String(dark));
b.setAttribute("aria-label",dark?"Use light theme":"Use dark theme")})();
document.addEventListener("keydown",function(e){if(e.key!=="Escape"||!document.body.classList.contains("nav-open"))return;
document.body.classList.remove("nav-open");var n=document.querySelector("[data-nav-toggle]");
if(n){n.setAttribute("aria-expanded","false");n.focus()}});`;

function sidebar(activeSlug?: string): string {
  const groups = GROUPS.map((group) => {
    const items = group.pages.map((page: DocPage) => {
      const active = page.slug === activeSlug;
      return `<li><a href="${BASE}/docs/${page.slug}/"${active ? ' aria-current="page"' : ""}>` +
        `${escapeHtml(page.nav)}</a></li>`;
    }).join("");
    return `<div class="rail-group"><h2>${escapeHtml(group.name)}</h2><ul>${items}</ul></div>`;
  }).join("");

  return `<nav class="rail" id="documentation-nav" aria-label="Documentation">
<div class="rail-group"><h2>Start</h2><ul>
<li><a href="${BASE}/"${activeSlug === undefined ? "" : ""}>Overview</a></li>
<li><a href="${BASE}/docs/"${activeSlug === "index" ? ' aria-current="page"' : ""}>All documentation</a></li>
</ul></div>
${groups}
<div class="rail-group"><h2>Elsewhere</h2><ul>
<li><a href="${REPO}" target="_blank" rel="noopener noreferrer">Source ↗</a></li>
<li><a href="${REPO}/releases" target="_blank" rel="noopener noreferrer">Releases ↗</a></li>
</ul></div>
</nav>`;
}

function tocFor(headings: Heading[]): string {
  const usable = headings.filter((h) => h.depth === 2 || h.depth === 3);
  if (usable.length < 3) return "";
  const items = usable.map((h) =>
    `<li class="d${h.depth}"><a href="#${h.slug}">${escapeHtml(h.text)}</a></li>`).join("");
  return `<aside class="toc" aria-label="On this page">
<h2>On this page</h2><ul>${items}</ul></aside>`;
}

export function renderPage(options: PageOptions): string {
  const { title, description, path, body, chrome = "docs", headings = [] } = options;
  const origin = process.env.SITE_ORIGIN ?? "https://blitsen.dev";
  const canonical = `${origin}${BASE}${path}`;
  const full = path === "/" || path === "" ? "Blitsen" : `${title} — Blitsen`;
  const toc = chrome === "docs" ? tocFor(headings) : "";
  const edit = options.sourceFile
    ? `<a href="${REPO}/blob/main/docs/${options.sourceFile}">Edit this page on GitHub</a>`
    : `<a href="${REPO}">Blitsen on GitHub</a>`;

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(full)}</title>
<meta name="description" content="${escapeHtml(description)}">
<meta property="og:title" content="${escapeHtml(full)}">
<meta property="og:description" content="${escapeHtml(description)}">
<meta property="og:type" content="website">
<meta property="og:image" content="${origin}${BASE}/assets/blitsen-rampant-512.png">
<meta name="twitter:card" content="summary">
<link rel="canonical" href="${canonical}">
<link rel="icon" href="${BASE}/assets/blitsen.ico" sizes="any">
<link rel="icon" href="${BASE}/assets/blitsen-rampant.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="${BASE}/assets/blitsen-rampant-256.png">
<link rel="stylesheet" href="${BASE}/assets/site.css">
<script>${THEME_SCRIPT}</script>
</head>
<body class="${chrome === "docs" ? "with-rail" : "landing"}">
<a class="skip" href="#main">Skip to content</a>
<header class="masthead">
  <a class="brand" href="${BASE}/">
    ${crestMark("brand-crest")}
    <span class="brand-name">Blitsen</span>
  </a>
  <nav class="top" id="site-nav" aria-label="Site">
    <a href="${BASE}/docs/">Documentation</a>
    <a href="${BASE}/docs/platform-support/">Platform support</a>
    <a href="${REPO}" target="_blank" rel="noopener noreferrer">GitHub</a>
  </nav>
  <div class="mast-actions">
    <button type="button" class="icon-btn" data-theme-toggle aria-pressed="false"
      aria-label="Use dark theme" title="Change colour theme">
      <svg viewBox="0 0 24 24" aria-hidden="true" width="18" height="18"><path fill="currentColor"
        d="M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9Z"/></svg>
    </button>
    <button type="button" class="icon-btn nav-only" data-nav-toggle aria-expanded="false"
      aria-controls="${chrome === "docs" ? "documentation-nav" : "site-nav"}"
      aria-label="Toggle navigation">
      <svg viewBox="0 0 24 24" aria-hidden="true" width="18" height="18"><path fill="currentColor"
        d="M3 6h18v2H3V6Zm0 5h18v2H3v-2Zm0 5h18v2H3v-2Z"/></svg>
    </button>
  </div>
</header>
${chrome === "docs" ? sidebar(options.activeSlug) : ""}
<main id="main" class="${chrome === "docs" ? "doc" : "home"}">
${body}
</main>
${toc}
<footer class="foot">
  <div class="foot-inner">
    <p class="foot-note">
      Blitsen is <strong>pre-alpha</strong>. Check built output with <code>blitsen doctor</code>
      and test every target before distribution. It is an independent project built on
      <a href="https://github.com/DioxusLabs/blitz" target="_blank" rel="noopener noreferrer">Blitz</a> —
      not an official DioxusLabs project, and not endorsed by DioxusLabs.
    </p>
    <p class="foot-meta">
      Source dual-licensed Apache-2.0 or MIT · ${edit}
    </p>
  </div>
</footer>
<script>${TOGGLE_SCRIPT}</script>
</body>
</html>
`;
}
