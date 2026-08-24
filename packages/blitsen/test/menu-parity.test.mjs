// The menu vocabulary exists twice: the JS side (config.schema.json for the
// role enums, the config validator for accelerator modifiers and size limits)
// grades a configuration at export time, and the Rust parser under
// crates/blitsen-host/src/native_window/ decides what the runtime accepts. A
// role or modifier present on one side only is silently asymmetric — the CLI
// blesses a menu the runtime refuses, or the runtime accepts one the CLI would
// have rejected — so the two vocabularies are held equal here, in the style of
// android-renderer.test.mjs (assertions over source text rather than pretending
// a JS test can parse Rust).
//
// Extraction is anchored to distinctive syntax and vocabulary rather than file
// layout, so it survives the in-flight native_window file split and the config
// consolidation — and every reader asserts a positive count, because a reader
// that finds nothing must fail rather than pass on two empty sets.
import { describe, expect, test } from "bun:test";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const repository = join(import.meta.dir, "../../..");
const NATIVE_WINDOW = "crates/blitsen-host/src/native_window";
const JS_SOURCES = "packages/blitsen/src";
const SCHEMA = "packages/blitsen/src/config.schema.json";
const NATIVE_BOOTSTRAP = "crates/blitsen-host/src/dom_bridge/bootstrap/native.js";

/** Every .rs file under native_window (recursively, surviving the file split),
 *  plus native_window.rs itself, concatenated. */
async function rustMenuSource() {
  const directory = join(repository, NATIVE_WINDOW);
  const files = (await readdir(directory, { recursive: true }))
    .filter(name => name.endsWith(".rs"))
    .map(name => join(directory, name));
  files.push(join(repository, `${NATIVE_WINDOW}.rs`));
  const sources = await Promise.all(files.map(file => readFile(file, "utf8")));
  return sources.join("\n");
}

/** Every .mjs file directly under packages/blitsen/src, concatenated: the menu
 *  validator lives in config.mjs today, but the consolidation may move it. */
async function jsConfigSource() {
  const directory = join(repository, JS_SOURCES);
  const files = (await readdir(directory))
    .filter(name => name.endsWith(".mjs"))
    .map(name => join(directory, name));
  const sources = await Promise.all(files.map(file => readFile(file, "utf8")));
  return sources.join("\n");
}

/** All string values of `enum` arrays in the schema for which `test` accepts
 *  the enum's sibling properties — how a role enum is found without depending
 *  on where the consolidation leaves it in the tree. */
function schemaEnums(node, accepts, found = []) {
  if (Array.isArray(node)) {
    for (const entry of node) schemaEnums(entry, accepts, found);
  } else if (node !== null && typeof node === "object") {
    if (Array.isArray(node.enum) && accepts(node)) found.push(node.enum);
    for (const value of Object.values(node)) schemaEnums(value, accepts, found);
  }
  return found;
}

const quoted = text => [...text.matchAll(/"([^"]+)"/g)].map(([, value]) => value);

describe("menu vocabulary parity between the CLI and the runtime", () => {
  test("the runtime has one parameterized menu walker and accelerator parser", async () => {
    const source = await readFile(join(repository, NATIVE_BOOTSTRAP), "utf8");
    expect(source.match(/const normaliseMenuTree\s*=/g)?.length).toBe(1);
    expect(source.match(/const normaliseAccelerator\s*=/g)?.length).toBe(1);
    expect(source.match(/normaliseMenuTree\(menu, \{/g)?.length,
      "tray and application menu normalization must both use the shared walker").toBe(2);
    expect(source).not.toContain("const normaliseMenu =");
    expect(source).not.toContain("const normaliseLevel =");
  });

  test("the schema's item role enum matches MenuRole::parse", async () => {
    const rust = await rustMenuSource();
    // The parse arms live inside `impl MenuRole { ... }`; the block is matched
    // to its closing brace at column zero, wherever the split puts the file.
    const block = /impl MenuRole \{([\s\S]*?)\n\}/.exec(rust)?.[1];
    expect(block, `no \`impl MenuRole {\` block found under ${NATIVE_WINDOW}; `
      + "update this test's extraction alongside the parser").toBeDefined();
    const rustRoles = [...block.matchAll(/"(\w+)" => Self::\w+/g)].map(([, role]) => role);
    expect(rustRoles.length,
      `MenuRole::parse under ${NATIVE_WINDOW} no longer has recognizable match arms`)
      .toBeGreaterThanOrEqual(15);

    const schema = JSON.parse(await readFile(join(repository, SCHEMA), "utf8"));
    // An item role enum is the one that offers "selectAll"; the submenu role
    // enum ("application"/"edit"/...) deliberately does not qualify.
    const enums = schemaEnums(schema, node => node.enum.includes("selectAll"));
    expect(enums.length, `${SCHEMA} no longer declares an item role enum `
      + "(no enum containing \"selectAll\")").toBeGreaterThanOrEqual(1);
    for (const roles of enums) {
      expect([...roles].sort(), `the menu role vocabulary in ${SCHEMA} disagrees with `
        + `MenuRole::parse under ${NATIVE_WINDOW}`).toEqual([...rustRoles].sort());
    }
  });

  test("the accelerator modifier vocabulary matches the Rust parser's", async () => {
    const rust = await rustMenuSource();
    const matcher = /matches!\(\s*part\.to_ascii_lowercase\(\)\.as_str\(\),([\s\S]*?)\)/
      .exec(rust)?.[1];
    expect(matcher, `no accelerator modifier matches! block found under ${NATIVE_WINDOW}; `
      + "update this test's extraction alongside the parser").toBeDefined();
    const rustModifiers = quoted(matcher);
    expect(rustModifiers.length).toBeGreaterThanOrEqual(8);

    const js = await jsConfigSource();
    // Anchored to the vocabulary: the JS validator's modifier sets are the Set
    // literals that contain "cmdorctrl", wherever the consolidation puts them.
    const sets = [...js.matchAll(/new Set\(\[([^\]]*)\]\)/gs)]
      .map(([, body]) => quoted(body))
      .filter(values => values.includes("cmdorctrl"));
    expect(sets.length, `no accelerator modifier Set literal (containing "cmdorctrl") `
      + `found in ${JS_SOURCES}/*.mjs`).toBeGreaterThanOrEqual(1);
    for (const modifiers of sets) {
      expect([...modifiers].sort(), `the accelerator modifier vocabulary in `
        + `${JS_SOURCES}/*.mjs disagrees with the parser under ${NATIVE_WINDOW}`)
        .toEqual([...rustModifiers].sort());
    }
  });

  test("the nesting and entry-count limits match the Rust parser's", async () => {
    const rust = await rustMenuSource();
    const js = await jsConfigSource();
    // Both sides state the limits in the same sentence, which is the anchor.
    const limits = text => ({
      depths: [...text.matchAll(/at most (\d+) levels/g)].map(([, value]) => Number(value)),
      counts: [...text.matchAll(/at most (\d+) entries/g)].map(([, value]) => Number(value)),
    });
    const rustLimits = limits(rust);
    const jsLimits = limits(js);
    expect(rustLimits.depths.length, `no "at most N levels" limit found under ${NATIVE_WINDOW}`)
      .toBeGreaterThanOrEqual(1);
    expect(rustLimits.counts.length, `no "at most N entries" limit found under ${NATIVE_WINDOW}`)
      .toBeGreaterThanOrEqual(1);
    expect(jsLimits.depths.length, `no "at most N levels" limit found in ${JS_SOURCES}/*.mjs`)
      .toBeGreaterThanOrEqual(1);
    expect(jsLimits.counts.length, `no "at most N entries" limit found in ${JS_SOURCES}/*.mjs`)
      .toBeGreaterThanOrEqual(1);
    // The stated limits also have to match the comparisons enforcing them.
    expect([...new Set([...rust.matchAll(/\bdepth\s*>\s*(\d+)/g)].map(([, v]) => Number(v)))])
      .toEqual([rustLimits.depths[0]]);
    expect([...new Set([...rust.matchAll(/\bitems\s*>\s*(\d+)/g)].map(([, v]) => Number(v)))])
      .toEqual([rustLimits.counts[0]]);
    // On the JS side enforcement runs through named constants, so the constants
    // are held to the parser too — otherwise a constant edit could leave the
    // messages (asserted above) stating a limit the validator no longer applies.
    expect([...js.matchAll(/MENU_MAX_DEPTH\s*=\s*(\d+)/g)].map(([, v]) => Number(v)),
      `no MENU_MAX_DEPTH constant found in ${JS_SOURCES}/*.mjs`)
      .toEqual([rustLimits.depths[0]]);
    expect([...js.matchAll(/MENU_MAX_ITEMS\s*=\s*(\d+)/g)].map(([, v]) => Number(v)),
      `no MENU_MAX_ITEMS constant found in ${JS_SOURCES}/*.mjs`)
      .toEqual([rustLimits.counts[0]]);
    for (const depth of [...rustLimits.depths, ...jsLimits.depths]) {
      expect(depth, `the menu nesting limit in ${JS_SOURCES}/*.mjs disagrees with `
        + `the parser under ${NATIVE_WINDOW}`).toBe(rustLimits.depths[0]);
    }
    for (const count of [...rustLimits.counts, ...jsLimits.counts]) {
      expect(count, `the menu entry-count limit in ${JS_SOURCES}/*.mjs disagrees with `
        + `the parser under ${NATIVE_WINDOW}`).toBe(rustLimits.counts[0]);
    }
  });
});
