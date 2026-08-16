#!/usr/bin/env bun
// Verifies the built site. Run after site:build.
//
// Three classes of failure this catches, all of which have shipped on documentation
// sites before: an internal link pointing at a page that was never generated, an
// in-page anchor that no heading actually defines, and markdown that the renderer
// failed to consume and emitted as literal text.

import { readdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { BASE } from "./content.ts";

const DIST = resolve(import.meta.dir, "../dist");

interface Problem { file: string; kind: string; detail: string }

async function htmlFiles(dir: string, acc: string[] = []): Promise<string[]> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) await htmlFiles(full, acc);
    else if (entry.name.endsWith(".html")) acc.push(full);
  }
  return acc;
}

/** Maps a site-absolute URL path to the file that should serve it. */
function targetFor(urlPath: string): string {
  const withoutBase = BASE && urlPath.startsWith(BASE) ? urlPath.slice(BASE.length) : urlPath;
  const clean = withoutBase || "/";
  if (clean.endsWith("/")) return join(DIST, clean, "index.html");
  return join(DIST, clean);
}

async function main(): Promise<void> {
  if (!existsSync(DIST)) {
    console.error("site/dist does not exist — run `bun run site:build` first.");
    process.exit(1);
  }

  const files = await htmlFiles(DIST);
  const problems: Problem[] = [];
  const anchorsByPath = new Map<string, Set<string>>();
  const contents = new Map<string, string>();

  const cnamePath = join(DIST, "CNAME");
  const cname = existsSync(cnamePath) ? await readFile(cnamePath, "utf8") : "";
  if (cname.trim() !== "blitsen.dev") {
    problems.push({
      file: "/CNAME",
      kind: "invalid custom domain",
      detail: cname.trim() || "file missing",
    });
  }

  for (const file of files) {
    const html = await readFile(file, "utf8");
    contents.set(file, html);
    const ids = new Set<string>();
    for (const m of html.matchAll(/\sid="([^"]+)"/g)) ids.add(m[1]!);
    anchorsByPath.set(file, ids);
  }

  for (const [file, html] of contents) {
    const rel = file.slice(DIST.length) || "/";

    // Literal markdown that survived rendering. Restricted to patterns that cannot
    // occur legitimately in prose, to keep this from crying wolf.
    const leaks: Array<[RegExp, string]> = [
      [/\]\((?!#)[^)\s]*\.md[^)]*\)/, "unrendered markdown link to a .md file"],
      [/(^|\n)\s{0,3}#{1,6}\s+\w/, "unrendered ATX heading"],
      [/(^|\n)\|[^\n]*\|\s*\n\s*\|?\s*:?-{3}/, "unrendered pipe table"],
    ];
    const text = html.replace(/<pre[\s\S]*?<\/pre>/g, "").replace(/<code>[\s\S]*?<\/code>/g, "");
    for (const [pattern, kind] of leaks) {
      const hit = text.match(pattern);
      if (hit) problems.push({ file: rel, kind, detail: hit[0].slice(0, 70).replace(/\n/g, "⏎") });
    }

    for (const m of html.matchAll(/href="([^"]+)"/g)) {
      const href = m[1]!;
      if (/^(https?:|mailto:|#)/.test(href)) continue;
      const [path, hash] = href.split("#");
      if (!path) continue;

      const target = targetFor(path);
      if (!existsSync(target)) {
        problems.push({ file: rel, kind: "dead internal link", detail: href });
        continue;
      }
      if (hash) {
        const ids = anchorsByPath.get(target);
        if (ids && !ids.has(hash)) {
          problems.push({ file: rel, kind: "anchor not found on target page", detail: href });
        }
      }
    }

    for (const m of html.matchAll(/src="([^"]+)"/g)) {
      const src = m[1]!;
      if (/^(https?:|data:)/.test(src)) continue;
      if (!existsSync(targetFor(src))) {
        problems.push({ file: rel, kind: "missing image", detail: src });
      }
    }
  }

  const anchorMisses = problems.filter((p) => p.kind.startsWith("anchor"));
  const hard = problems.filter((p) => !p.kind.startsWith("anchor"));

  console.log(`checked ${files.length} pages`);
  for (const p of hard) console.log(`  ✗ ${p.file}: ${p.kind} — ${p.detail}`);
  for (const p of anchorMisses) console.log(`  ⚠ ${p.file}: ${p.kind} — ${p.detail}`);

  if (hard.length === 0 && anchorMisses.length === 0) {
    console.log("  no dead links, no missing images, no unrendered markdown");
  } else {
    console.log(`\n${hard.length} error(s), ${anchorMisses.length} anchor warning(s)`);
  }
  process.exit(hard.length > 0 ? 1 : 0);
}

await main();
