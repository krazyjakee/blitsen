#!/usr/bin/env bun
// Builds the static site into site/dist.
//
//   bun run site:build                    → root base path, for blitsen.dev
//   SITE_BASE=/preview bun run site:build → explicit sub-path, when needed
//
// Output is plain HTML, one stylesheet and a handful of images. No client-side
// framework, no hydration, no build-time dependency beyond the bun already in the
// repo — which is also the kind of static bundle Blitsen itself is built to run.

import { mkdir, readFile, writeFile, copyFile, rm, readdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { ALL_PAGES, BASE, GROUPS, GUIDE_PAGES, REPO, rewriteDocLink, rewriteRootLink } from "./content.ts";
import { renderMarkdown, escapeHtml } from "./markdown.ts";
import { renderPage, setCrest } from "./layout.ts";
import { landingBody } from "./landing.ts";

const ROOT = resolve(import.meta.dir, "../..");
const DOCS = join(ROOT, "docs");
const BRAND = join(ROOT, "assets/brand");
const SITE = join(ROOT, "site");
const DIST = join(SITE, "dist");

async function write(relative: string, contents: string): Promise<void> {
  const target = join(DIST, relative);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, contents, "utf8");
}

async function copyInto(from: string, relative: string): Promise<boolean> {
  if (!existsSync(from)) {
    console.warn(`  ! missing asset, skipped: ${from}`);
    return false;
  }
  const target = join(DIST, relative);
  await mkdir(dirname(target), { recursive: true });
  await copyFile(from, target);
  return true;
}

function docsIndexBody(): string {
  const groups = GROUPS.map((group) => {
    const items = group.pages.map((page) =>
      `<li><a href="${BASE}/docs/${page.slug}/">` +
      `<span class="card-title">${escapeHtml(page.title)}</span>` +
      `<span class="card-blurb">${escapeHtml(page.blurb)}</span></a></li>`).join("");
    return `<section class="doc-group"><h2>${escapeHtml(group.name)}</h2>` +
      `<p class="group-note">${escapeHtml(group.note)}</p>` +
      `<ul class="card-list">${items}</ul></section>`;
  }).join("");

  return `<article class="prose">
<h1>Documentation</h1>
<p class="lede">
  Start with the task you need to complete. The main guide covers running,
  configuring, building and distributing Blitsen applications. The guides are
  maintained in <a href="${REPO}/tree/main/docs">the repository</a>.
</p>
${groups}
</article>`;
}

async function build(): Promise<void> {
  const started = Date.now();
  await rm(DIST, { recursive: true, force: true });
  await mkdir(DIST, { recursive: true });

  const crestSvg = await readFile(join(BRAND, "svg/blitsen-rampant.svg"), "utf8");
  setCrest(crestSvg);

  // ── home ────────────────────────────────────────────────────────────────────
  await write("index.html", renderPage({
    title: "Blitsen",
    description:
      "Write an app in HTML, CSS and TypeScript. Ship a native executable. " +
      "A pre-alpha native runtime that hosts a JavaScript engine and renders through Blitz — no Chromium, no WebView.",
    path: "/",
    chrome: "bare",
    body: landingBody(crestSvg),
  }));

  // ── documentation index ─────────────────────────────────────────────────────
  await write("docs/index.html", renderPage({
    title: "Documentation",
    description: "Task-based guides for running, configuring, building and distributing Blitsen applications.",
    path: "/docs/",
    activeSlug: "index",
    body: docsIndexBody(),
  }));

  // ── one page per doc ────────────────────────────────────────────────────────
  let pages = 0;
  for (const page of ALL_PAGES) {
    const source = join(DOCS, page.file);
    if (!existsSync(source)) {
      console.warn(`  ! ${page.file} is registered but missing from docs/ — skipped`);
      continue;
    }
    const markdown = await readFile(source, "utf8");
    const { html, headings, summary } = renderMarkdown(markdown, rewriteDocLink);

    const body = `<article class="prose">
<p class="crumb"><a href="${BASE}/docs/">Documentation</a> <span aria-hidden="true">/</span> ${escapeHtml(page.nav)}</p>
${html}
</article>`;

    await write(`docs/${page.slug}/index.html`, renderPage({
      title: page.title,
      description: page.blurb || summary.slice(0, 180),
      path: `/docs/${page.slug}/`,
      body,
      headings,
      sourceFile: page.file,
      activeSlug: page.slug,
    }));
    pages += 1;
  }

  // ── 404 ─────────────────────────────────────────────────────────────────────
  await write("404.html", renderPage({
    title: "Not found",
    description: "That page does not exist.",
    path: "/404.html",
    chrome: "bare",
    body: `<section class="hero notfound">
<p class="eyebrow">404</p>
<h1>Nothing here</h1>
<p class="lede">That page does not exist — which, for a runtime whose whole discipline is
that a missing API is <em>absent</em> rather than stubbed, is at least consistent.</p>
<div class="cta"><a class="btn primary" href="${BASE}/">Home</a>
<a class="btn" href="${BASE}/docs/">Documentation</a></div>
</section>`,
  }));

  // ── assets ──────────────────────────────────────────────────────────────────
  await copyInto(join(SITE, "assets/site.css"), "assets/site.css");
  for (const name of ["blitsen-rampant.svg", "blitsen-rampant-icon.svg"]) {
    await copyInto(join(BRAND, "svg", name), `assets/${name}`);
  }
  const icons = existsSync(join(BRAND, "icons")) ? await readdir(join(BRAND, "icons")) : [];
  for (const name of icons) {
    await copyInto(join(BRAND, "icons", name), `assets/${name}`);
  }
  for (const name of ["pong.gif", "shadcn-admin.png"]) {
    await copyInto(join(DOCS, name), `assets/${name}`);
  }

  // GitHub Pages runs Jekyll unless told otherwise, which would eat _-prefixed paths.
  await write(".nojekyll", "");

  // Pages reads the custom domain from this file on every deploy, so it has to be
  // part of the artifact — a domain set only in the repository settings is dropped
  // the next time the workflow publishes.
  await write("CNAME", "blitsen.dev\n");

  // ── sitemap and robots ──────────────────────────────────────────────────────
  const origin = process.env.SITE_ORIGIN ?? "https://blitsen.dev";
  const urls = ["/", "/docs/", ...GUIDE_PAGES.map((p) => `/docs/${p.slug}/`)];
  await write("sitemap.xml",
    `<?xml version="1.0" encoding="UTF-8"?>\n` +
    `<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n` +
    urls.map((u) => `  <url><loc>${origin}${BASE}${u}</loc></url>`).join("\n") +
    `\n</urlset>\n`);
  await write("robots.txt", `User-agent: *\nAllow: /\nSitemap: ${origin}${BASE}/sitemap.xml\n`);

  console.log(`built ${pages + 2} pages in ${Date.now() - started} ms → site/dist`);
  console.log(`base path ${BASE || "(root)"}`);
}

await build();
