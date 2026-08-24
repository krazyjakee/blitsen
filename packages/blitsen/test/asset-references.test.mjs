import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { planIngest, rewriteRootRelativeReferences } from "../src/application-ingest.mjs";
import { HTML_ASSET_ATTRIBUTES } from "../src/asset-references.mjs";
import { generateApiManifest } from "../src/api-manifest.mjs";

const JS_TABLE = join(import.meta.dir, "../src/asset-references.mjs");
const RUST_VALIDATOR = join(import.meta.dir, "../../../crates/blitsen-host/src/assets.rs");

describe("asset reference rules", () => {
  // The same table exists twice: the JS side decides what an export collects,
  // and the Rust side decides what a running directory accepts. Drift between
  // them is silently asymmetric — a build carries what the runtime refuses, or
  // the runtime accepts what no build ever ships — so the two are held equal
  // here rather than each being compared to a literal copy of itself.
  test("the Rust runtime validates the same (element, attribute) pairs the export collects", async () => {
    const rust = await readFile(RUST_VALIDATOR, "utf8");
    const table = /for \(selector, attribute\) in \[([\s\S]*?)\]\s*\{/.exec(rust)?.[1];
    expect(table,
      `${RUST_VALIDATOR} no longer declares a for (selector, attribute) in [...] table; `
      + `update this test's extraction alongside it`).toBeDefined();
    const rustPairs = [...table.matchAll(/\("([a-z]+)\[([a-z-]+)\]",\s*"([a-z-]+)"\)/g)]
      .map(([, element, selectorAttribute, attribute]) => {
        // The selector and the attribute read from the node must agree with
        // each other before either is compared across languages.
        expect(selectorAttribute).toBe(attribute);
        return `${element}[${attribute}]`;
      });
    // A reader finding nothing must fail, not pass on an empty set.
    expect(rustPairs.length).toBeGreaterThanOrEqual(10);
    const jsPairs = HTML_ASSET_ATTRIBUTES.map(({ element, attribute }) => `${element}[${attribute}]`);
    expect(jsPairs.length).toBeGreaterThanOrEqual(10);
    expect(new Set(rustPairs).size).toBe(rustPairs.length);
    expect(new Set(jsPairs).size).toBe(jsPairs.length);
    expect(rustPairs.sort(), `the HTML asset-reference tables in ${RUST_VALIDATOR} `
      + `(validate_local_assets) and ${JS_TABLE} (HTML_ASSET_ATTRIBUTES) disagree`)
      .toEqual([...jsPairs].sort());
  });

  test("one HTML attribute table drives reachability, rewriting, and remote diagnostics", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-asset-rules-"));
    try {
      await mkdir(join(directory, "assets"));
      const references = HTML_ASSET_ATTRIBUTES.map((rule, index) => ({
        ...rule, path: `assets/${rule.element}-${rule.attribute}-${index}.bin`,
      }));
      for (const reference of references) {
        await writeFile(join(directory, reference.path), reference.path);
      }
      const source = references.map((reference, index) =>
        `<${reference.element} data-order="${index}" `
        + `${reference.attribute}="/${reference.path}?v=${index}"></${reference.element}>`)
        .join("\n");
      await writeFile(join(directory, "index.html"), source);

      const plan = await planIngest(directory);
      const kept = new Set(plan.files.map(file => file.relative));
      const rewritten = rewriteRootRelativeReferences(source, "index.html");
      const manifest = await generateApiManifest();
      for (const reference of references) {
        expect(kept.has(reference.path)).toBeTrue();
        expect(rewritten).toContain(
          `${reference.attribute}="./${reference.path}?v=${references.indexOf(reference)}"`);
        const code = reference.remote === "script" ? "ASSET_REMOTE_SCRIPT" : "ASSET_REMOTE";
        const rule = manifest.assets.find(item => item.kind === "html" && item.code === code);
        expect(new RegExp(rule.pattern, "i").test(
          `<${reference.element} ${reference.attribute}="https://cdn.example/a">`)).toBeTrue();
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("CSS syntax keeps its deliberate scan, rewrite, and diagnostic capabilities", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-css-asset-rules-"));
    try {
      await mkdir(join(directory, "assets"));
      await writeFile(join(directory, "assets/image.png"), "image");
      await writeFile(join(directory, "assets/import.css"), "body {}");
      await writeFile(join(directory, "index.html"), '<link href="/style.css">');
      const source = '@import "/assets/import.css"; .hero { background: url("/assets/image.png") }';
      await writeFile(join(directory, "style.css"), source);

      const plan = await planIngest(directory);
      expect(plan.files.map(file => file.relative)).toEqual([
        "assets/image.png", "assets/import.css", "index.html", "style.css",
      ]);
      const rewritten = rewriteRootRelativeReferences(source, "style.css");
      expect(rewritten).toContain('@import "/assets/import.css"');
      expect(rewritten).toContain('url("./assets/image.png")');
      const manifest = await generateApiManifest();
      const remote = manifest.assets.find(item => item.kind === "css");
      expect(new RegExp(remote.pattern, "i").test('url("//cdn.example/a.png")')).toBeTrue();
      expect(new RegExp(remote.pattern, "i").test('@import "//cdn.example/a.css"')).toBeFalse();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("discovers repeated and cyclic references once in deterministic order", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-cyclic-asset-graph-"));
    try {
      await writeFile(join(directory, "index.html"), [
        '<script src="./a.js"></script>',
        '<script src="./a.js"></script>',
        '<script src="./b.js"></script>',
      ].join("\n"));
      await writeFile(join(directory, "a.js"), 'import "./b.js"; import "./b.js";');
      await writeFile(join(directory, "b.js"), 'import "./a.js";');

      const plan = await planIngest(directory);
      expect(plan.files.map(file => file.relative)).toEqual(["a.js", "b.js", "index.html"]);
      expect([...plan.resolutions.keys()]).toEqual(["index.html", "a.js", "b.js"]);
      expect(plan.unreferenced).toEqual([]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});
