import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildManifest, generateApiManifest, loadApiManifest, readBootstrapScript, renderCompatibilityDoc } from "../src/api-manifest.mjs";
import { main, parseArgs } from "../src/cli.mjs";
import { doctorApplication } from "../src/doctor.mjs";
import { checkNativeModuleTable } from "../src/native-modules.mjs";
import { resolvePhase2Runtime } from "../src/runtime.mjs";
import { capture } from "./cli-support.mjs";

describe("directory CLI", () => {
  // The profile's engine-level absences are declared rather than derived (see
  // ENGINE_ABSENT in api-manifest.mjs), so this is what keeps the declaration
  // from becoming fiction: the shipping runtime reports what it does not
  // define, and the manifest has to agree exactly.
  test("the engine agrees with what the profile says it does not implement", async () => {
    const runtime = await resolvePhase2Runtime().catch(() => null);
    if (!runtime || !(await Bun.file(runtime.path).exists())) return;
    const reported = Bun.spawnSync({ cmd: [runtime.path, "--engine-report"], stdout: "pipe", stderr: "pipe" });
    expect(reported.exitCode).toBe(0);
    const report = JSON.parse(reported.stdout.toString());
    const manifest = await loadApiManifest();
    const declared = manifest.apis
      .filter(entry => entry.origin === "engine" && entry.status === "absent")
      .map(entry => entry.api)
      .sort();
    expect([...report.absentGlobals].sort()).toEqual(declared);
  });

  test("diagnoses output outside the strict compatibility profile", async () => {
    const fixtures = join(import.meta.dir, "fixtures/doctor");
    const compatible = await doctorApplication(join(fixtures, "compatible"));
    expect(compatible).toMatchObject({ profile: "v1-strict", errors: 0, warnings: 0, files: 3 });

    // A canvas is no longer one of these (issue #99): what is left in this
    // fixture degrades, so the build is graded and not refused.
    const { lines, output } = capture();
    expect(await main(["doctor", join(fixtures, "unsupported")], output)).toBe(0);
    expect(lines.some(([, line]) => line.includes("WEB_CANVAS") && line.includes("2D context is")))
      .toBeTrue();
    expect(lines.some(([, line]) =>
      line.includes("WEB_STORAGE_MEMORY") && line.includes("gone when the application exits")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("0 errors, 2 warnings");

    // What still blocks: an entry point that names source rather than output,
    // because nothing in the runtime transpiles it and the window stays blank.
    const blocked = capture();
    expect(await main(["doctor", join(fixtures, "source-entry")], blocked.output)).toBe(1);
    expect(blocked.lines.some(([, line]) =>
      line.includes("HTML_SOURCE_ENTRY") && line.includes("vite build"))).toBeTrue();
    expect(blocked.lines.at(-1)[1]).toContain("1 errors, 0 warnings");
  });

  // The severities the third-party evidence settled. Every remote subresource
  // degrades rather than killing the page: one the export will not serve is
  // answered with empty bytes, and a remote <script src> is skipped by the
  // loader while the rest of the document runs. So none of them blocks a build.
  test("grades every remote subresource a warning, including a script", async () => {
    const fixtures = join(import.meta.dir, "fixtures/doctor");
    const subresource = await doctorApplication(join(fixtures, "remote-subresource"));
    expect(subresource).toMatchObject({ errors: 0, warnings: 5 });
    // A preconnect, a Google Fonts stylesheet, a remote <img>, a CSS url().
    expect(subresource.diagnostics.map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`))
      .toEqual(["warning:WEB_XHR", "warning:ASSET_REMOTE", "warning:ASSET_REMOTE",
        "warning:ASSET_REMOTE", "warning:ASSET_REMOTE"]);

    const script = await doctorApplication(join(fixtures, "remote"));
    expect(script).toMatchObject({ errors: 0, warnings: 1 });
    expect(script.diagnostics.map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`))
      .toEqual(["warning:ASSET_REMOTE_SCRIPT"]);
  });

  // Every one of these shapes is verbatim from an unmodified third-party build
  // that renders correctly. A reference is not a call, and the scan cannot see
  // the guard around it, so none of them may block a build.
  test("does not block a build on absent APIs a bundle feature-detects", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-guarded-test-"));
    try {
      await writeFile(join(directory, "app.js"), [
        `typeof XMLHttpRequest<"u"&&new XMLHttpRequest;`,
        `typeof ShadowRoot<"u"&&e instanceof ShadowRoot;`,
        `typeof customElements<"u"&&customElements.get(n);`,
        `try{document.cookie="theme=dark"}catch{}`,
        `typeof SharedWorker<"u"&&new SharedWorker(u);`,
        `typeof OffscreenCanvas<"u"&&new OffscreenCanvas(1,1);`,
        `e?window.open(u,"_blank"):0;`,
        `typeof indexedDB<"u"&&indexedDB.open("x");`,
      ].join("\n"));
      const report = await doctorApplication(directory);
      expect(report.errors).toBe(0);
      expect([...new Set(report.diagnostics.map(diagnostic => diagnostic.code))].sort())
        .toEqual(["WEB_CANVAS", "WEB_COMPONENTS", "WEB_COOKIE", "WEB_NAVIGATION", "WEB_STORAGE",
          "WEB_WORKER", "WEB_XHR"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("accepts the routing and fetch surface the runtime actually implements", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-doctor-test-"));
    try {
      await writeFile(join(directory, "app.js"), [
        `history.pushState({ page: 1 }, "", "/reports");`,
        `history.replaceState(null, "", location.pathname);`,
        `addEventListener("popstate", () => fetch("https://api.example.com/reports"));`,
        `fetch(endpoint).then(response => response.json());`,
      ].join("\n"));
      const report = await doctorApplication(directory);
      expect(report).toMatchObject({ errors: 0, warnings: 0 });

      await writeFile(join(directory, "app.js"), [
        `fetch("/api/reports");`,
        `location.reload();`,
        `new ReadableStream();`,
      ].join("\n"));
      const codes = (await doctorApplication(directory)).diagnostics
        .map(diagnostic => `${diagnostic.severity}:${diagnostic.code}`);
      expect(codes).toEqual(["error:WEB_FETCH", "warning:WEB_NAVIGATION", "warning:WEB_STREAM"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // Issue #125: fetch reads the files the application shipped, so the question
  // a literal path raises is whether the export carries it — not whether it
  // starts with a slash.
  test("reports a fetched path the output does not ship, and nothing for one it does", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-doctor-fetch-"));
    try {
      await writeFile(join(directory, "data.json"), `{"ok":true}`);
      await mkdir(join(directory, "assets"), { recursive: true });
      await writeFile(join(directory, "assets", "blip.wav"), "RIFF");
      await writeFile(join(directory, "app.js"), [
        `fetch("./data.json");`,
        `fetch("/data.json");`,
        `fetch("/assets/blip.wav?v=2");`,
      ].join("\n"));
      expect(await doctorApplication(directory)).toMatchObject({ errors: 0, warnings: 0 });

      await writeFile(join(directory, "app.js"), `fetch("/api/reports");`);
      const report = await doctorApplication(directory);
      expect(report.errors).toBe(1);
      expect(report.diagnostics[0].code).toBe("WEB_FETCH");
      expect(report.diagnostics[0].message).toContain("does not ship");

      // The idiomatic spelling, whose literal is one level in and whose base is
      // the module rather than the document: a chunk naming its own neighbour
      // is silent, and one naming a file nothing ships is not.
      await writeFile(join(directory, "app.js"), "");
      await writeFile(join(directory, "assets", "app.js"), [
        `fetch(new URL("./blip.wav", import.meta.url));`,
        `fetch(new URL("../data.json", import.meta.url).href);`,
        `fetch(new URL("./absent.wav", import.meta.url));`,
      ].join("\n"));
      const modules = await doctorApplication(directory);
      expect(modules.diagnostics.filter(entry => entry.code === "WEB_FETCH")
        .map(entry => entry.target)).toEqual(["./absent.wav"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test.skipIf(process.platform === "win32")("ignores symbolic links while grading output", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-doctor-link-"));
    try {
      await writeFile(join(directory, "app.js"), "new SharedWorker(url);");
      await symlink(join(directory, "app.js"), join(directory, "linked.js"));
      const report = await doctorApplication(directory);
      expect(report.files).toBe(1);
      expect(report.diagnostics.map(diagnostic => diagnostic.code)).toEqual(["WEB_WORKER"]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("keeps the API manifest and the documented tiers generated from the runtime source", async () => {
    const generated = await generateApiManifest();
    expect(generated).toEqual(await loadApiManifest());
    expect(await renderCompatibilityDoc(generated))
      .toBe(await readFile(join(import.meta.dir, "../../../docs/COMPATIBILITY.md"), "utf8"));
  });

  // The manifest is worth nothing unless it fails when it stops matching the
  // bootstrap, so each way the two can diverge is made to happen here.
  test("refuses a manifest the runtime source disagrees with", async () => {
    const source = await readBootstrapScript();
    expect(buildManifest(source).apis.find(entry => entry.api === "SharedWorker").status)
      .toBe("absent");
    const implemented = source
      .replace('"SharedWorker", "ServiceWorker"', '"ServiceWorker"')
      .replace("const globals = {", "const globals = {\n    SharedWorker,");
    expect(buildManifest(implemented).apis.find(entry => entry.api === "SharedWorker").status)
      .toBe("implemented");

    expect(() => buildManifest(source.replace("const globals = {", "const globals = {\n    speechSynthesis,")))
      .toThrow("installs speechSynthesis");
    expect(() => buildManifest(source.replace('"requestIdleCallback",', '"speechSynthesis", "requestIdleCallback",')))
      .toThrow("deletes speechSynthesis");
    expect(() => buildManifest(source.replace(", AbortSignal, fetch,", ", AbortSignal,")))
      .toThrow("fetch are absent from the runtime but not deleted");
    expect(() => buildManifest(source.replace("const globals = {", "const stubbed = {")))
      .toThrow("no longer declares const globals = {");
  });

  test("reads class members and instances by structure rather than indentation", async () => {
    const source = await readBootstrapScript();
    const reformatted = source
      .replace("  class Element extends Node {\n    get tagName() {",
        "class Element\n  extends Node\n{\nget tagName\n(\n)\n{")
      .replace("    querySelector(selector) {",
        "querySelector\n(\n  selector\n)\n{")
      .replace("  const document = new Document();",
        "const document\n=\nnew Document\n(\n)\n;");
    expect(buildManifest(reformatted)).toEqual(buildManifest(source));

    const malformed = source.replace("    querySelector(selector) {",
      "    querySelector(selector) => {");
    expect(() => buildManifest(malformed)).toThrow("Element.querySelector must have a method body");
  });

  test("diagnoses what the manifest calls absent, and nothing it calls implemented", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-manifest-test-"));
    try {
      await writeFile(join(directory, "app.js"),
        "new SharedWorker(url); customElements.define(); indexedDB.open('x'); window.open('/x');\n"
        + "localStorage.setItem('theme', 'dark');");
      const codes = (await doctorApplication(directory)).diagnostics.map(entry => entry.code);
      expect(codes.sort()).toEqual(["WEB_COMPONENTS", "WEB_NAVIGATION", "WEB_STORAGE",
        "WEB_STORAGE_MEMORY", "WEB_WORKER"]);

      const manifest = await loadApiManifest();
      await writeFile(join(directory, "app.js"), manifest.apis
        .filter(entry => entry.status === "implemented" && entry.kind === "global")
        .map(entry => `void ${entry.api};`).join("\n"));
      expect(await doctorApplication(directory)).toMatchObject({ errors: 0, warnings: 0 });
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // Issue #147. The table is declared rather than derived — a `cfg` in the Rust
  // is not readable from here — so the one thing that can be checked mechanically
  // is that it has not drifted from the module list or from its own reasons.
  test("keeps every native module absence paired with the reason for it", () => {
    expect(checkNativeModuleTable()).toBeGreaterThan(0);
  });

  // A `native:` module the target does not have is a finding at export time
  // rather than an `undefined` at run time (#147). Android is the reason the
  // rule exists, but it is not the only column: `blitsen/dialog` has been absent
  // on macOS and Windows since it shipped and nothing said so.
  test("reports a native: module the target being built for does not have", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-native-target-"));
    try {
      await writeFile(join(directory, "app.js"), [
        `import clipboard from "blitsen/clipboard";`,
        `import dialog from "blitsen/dialog";`,
        `import os from "blitsen/os";`,
        `import notify from "blitsen/notify";`,
        `const later = () => import("blitsen/app");`,
        `export { clipboard, dialog, os, notify, later };`,
      ].join("\n"));
      // The CLI resolves an application before it grades one, so the directory
      // has to be one; `doctorApplication` on its own does not care.
      await writeFile(join(directory, "index.html"),
        `<!doctype html><script type="module" src="./app.js"></script>`);

      const reported = async target => (await doctorApplication(directory, { target }))
        .diagnostics.filter(entry => entry.code === "NATIVE_MODULE_ABSENT")
        .map(entry => entry.message);

      // Linux has all five, so the same file is silent there. Without this the
      // test could pass against a rule that fires on everything.
      expect(await reported("linux-x64")).toEqual([]);
      expect(await reported("win32-x64")).toEqual(["blitsen/dialog does not exist on win32."]);
      expect(await reported("darwin-arm64")).toEqual(["blitsen/dialog does not exist on darwin."]);
      // Sorted by position in the file, which is why `app` — the dynamic import
      // after the direct imports — comes last rather than first. notify is
      // deliberately absent from the findings: Android implements that module.
      expect(await reported("android-arm64")).toEqual([
        "blitsen/clipboard does not exist on android.",
        "blitsen/dialog does not exist on android.",
        "blitsen/app does not exist on android.",
      ]);

      // The reason travels with the finding: a user who is told a module is
      // missing and not told why has nothing to decide with.
      const android = (await doctorApplication(directory, { target: "android-arm64" }))
        .diagnostics.find(entry => entry.code === "NATIVE_MODULE_ABSENT");
      expect(android.severity).toBe("warning");
      expect(android.guidance).toContain("`arboard` has no Android backend");
      expect(android.guidance).toContain("holds focus");

      // Absent modules are warnings, so an Android grading still exits 0 — the
      // application degrades rather than failing to render.
      const { lines, output } = capture();
      expect(await main(["doctor", directory, "--target", "android-arm64"], output)).toBe(0);
      expect(lines.some(([, line]) => line.includes("NATIVE_MODULE_ABSENT")
        && line.includes("blitsen/clipboard does not exist on android"))).toBeTrue();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  // Grading for a platform is not claiming to build for one: Android has no
  // runtime package to resolve and `blitsen build` still refuses it (#148).
  test("grades for a target it cannot build for, and refuses to build for it", () => {
    expect(parseArgs(["doctor", "dist", "--target", "android-arm64"]).target).toBe("android-arm64");
    expect(parseArgs(["doctor", "dist"]).target).toBeUndefined();
    expect(() => parseArgs(["build", "dist", "--target", "android-arm64"]))
      .toThrow("unknown --target android-arm64");
    expect(() => parseArgs(["doctor", "dist", "--json", "--force"]))
      .toThrow("--force is only valid with build");
    expect(() => parseArgs(["doctor", "dist", "--out", "app"]))
      .toThrow("--out is not valid with doctor");
  });

});
