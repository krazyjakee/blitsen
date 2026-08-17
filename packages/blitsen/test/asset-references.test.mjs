import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { planIngest, rewriteRootRelativeReferences } from "../src/application-ingest.mjs";
import { CSS_ASSET_REFERENCES, HTML_ASSET_ATTRIBUTES } from "../src/asset-references.mjs";
import { generateApiManifest } from "../src/api-manifest.mjs";

describe("asset reference rules", () => {
  test("one HTML attribute table drives reachability, rewriting, and remote diagnostics", async () => {
    expect(HTML_ASSET_ATTRIBUTES.map(({ element, attribute, remote }) =>
      [element, attribute, remote])).toEqual([
      ["script", "src", "script"],
      ["img", "src", "asset"],
      ["source", "src", "asset"],
      ["audio", "src", "asset"],
      ["video", "src", "asset"],
      ["track", "src", "asset"],
      ["embed", "src", "asset"],
      ["input", "src", "asset"],
      ["link", "href", "asset"],
      ["video", "poster", "asset"],
      ["object", "data", "asset"],
    ]);

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
    expect(CSS_ASSET_REFERENCES.map(({ syntax, rewriteRoot, remote }) =>
      ({ syntax, rewriteRoot, remote }))).toEqual([
      { syntax: "url", rewriteRoot: true, remote: true },
      { syntax: "import", rewriteRoot: false, remote: false },
    ]);
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
