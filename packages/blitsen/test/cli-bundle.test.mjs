import { describe, expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { buildPayload, buildTrailer, linkBundle, readBundle, FORMAT_VERSION } from "../src/bundle.mjs";

const run = promisify(execFile);
const REPO = new URL("../../../", import.meta.url).pathname;

// Issue #88: the format has two implementations — this package writes it, and
// the shipped runtime reads it. Nothing keeps them honest except a test that
// exercises both against the same bytes, so that is what these are.
describe("Phase 2 link step", () => {
  const application = new Map([
    ["index.html", Buffer.from("<!doctype html><body><main id=x>waiting</main>")],
    ["assets/app.js", Buffer.from("document.querySelector('#x').textContent = 'linked'")],
    ["assets/logo.png", Buffer.from([0x89, 0x50, 0x4e, 0x47])],
  ]);

  async function link(files = application) {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-"));
    const runtime = join(directory, "runtime");
    // Not the real runtime: what is under test is the container, and a stub
    // keeps the test independent of whether Rust has been built.
    await writeFile(runtime, Buffer.alloc(4096, 0x7f));
    const output = join(directory, "MyApp");
    const report = await linkBundle({ runtime, output, files });
    return { directory, output, report };
  }

  test("writes a payload the reader accepts, and reads it back unchanged", async () => {
    const { directory, output, report } = await link();
    try {
      expect(report.files).toBe(3);
      expect(report.totalBytes).toBe((await readFile(output)).length);

      const bundle = readBundle(await readFile(output));
      expect(bundle.version).toBe(FORMAT_VERSION);
      expect(bundle.verified).toBe(true);
      expect(bundle.digest).toBe(report.digest);
      expect([...bundle.files.keys()]).toEqual(["assets/app.js", "assets/logo.png", "index.html"]);
      expect(bundle.files.get("index.html").toString()).toBe(
        "<!doctype html><body><main id=x>waiting</main>");
      expect([...bundle.files.get("assets/logo.png")]).toEqual([0x89, 0x50, 0x4e, 0x47]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("links the same input to the same bytes", async () => {
    const first = await link();
    const second = await link();
    try {
      expect(first.report.digest).toBe(second.report.digest);
      expect(await readFile(first.output)).toEqual(await readFile(second.output));
    } finally {
      await rm(first.directory, { recursive: true, force: true });
      await rm(second.directory, { recursive: true, force: true });
    }
  });

  test("survives a code signature appended after the trailer", async () => {
    const { directory, output } = await link();
    try {
      const signed = Buffer.concat([await readFile(output), Buffer.alloc(9000, 0xa5)]);
      const bundle = readBundle(signed);
      expect(bundle.verified).toBe(true);
      expect(bundle.files.size).toBe(3);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("refuses a path that would escape the application", async () => {
    for (const escape of ["../secret", "/etc/passwd", "a/../../b", "", "a//b"]) {
      expect(() => buildPayload(new Map([[escape, Buffer.from("x")]]))).toThrow();
    }
  });

  test("is not fooled by the magic appearing inside the runtime", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-decoy-"));
    try {
      const runtime = join(directory, "runtime");
      await writeFile(runtime, Buffer.concat([
        Buffer.alloc(2048, 0x7f),
        Buffer.from("BLITSEN\x1a", "latin1"),
        Buffer.from("BLITSEN\0", "latin1"),
        Buffer.alloc(2048, 0x7f),
      ]));
      expect(readBundle(await readFile(runtime))).toBe(null);
      const output = join(directory, "MyApp");
      await linkBundle({ runtime, output, files: application });
      expect(readBundle(await readFile(output)).files.size).toBe(3);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // The one that matters: the Rust reader is what a shipped application uses,
  // so agreement with it is the whole point of the format. Skipped rather than
  // failed when the runtime has not been built, so this file still runs in the
  // JavaScript-only CI job.
  test("the Rust runtime reads what this package writes", async () => {
    const runtime = join(REPO, "target/debug/blitsen-runtime");
    if (!(await Bun.file(runtime).exists())) return;
    const directory = await mkdtemp(join(tmpdir(), "blitsen-bundle-rust-"));
    try {
      const output = join(directory, "MyApp");
      const report = await linkBundle({ runtime, output, files: application });
      const { stdout } = await run(output, ["--bundle-report"]);
      const runtimeReport = JSON.parse(stdout);
      expect(runtimeReport.bundled).toBe(true);
      expect(runtimeReport.verified).toBe(true);
      expect(runtimeReport.formatVersion).toBe(FORMAT_VERSION);
      expect(runtimeReport.digest).toBe(report.digest);
      expect(runtimeReport.payloadBytes).toBe(report.payloadBytes);
      expect(runtimeReport.files.map(file => file.path))
        .toEqual(["assets/app.js", "assets/logo.png", "index.html"]);
      expect(runtimeReport.files.find(file => file.path === "assets/logo.png").bytes).toBe(4);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("a trailer is exactly the bytes the format specifies", () => {
    const payload = buildPayload(new Map([["a.js", Buffer.from("x")]]));
    const trailer = buildTrailer(payload, 100);
    expect(trailer.length).toBe(64);
    expect(Number(trailer.readBigUInt64LE(32))).toBe(100);
    expect(Number(trailer.readBigUInt64LE(40))).toBe(payload.length);
    expect(trailer.readUInt32LE(48)).toBe(FORMAT_VERSION);
    expect(trailer.subarray(56).toString("latin1")).toBe("BLITSEN\x1a");
  });
});
