import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildManifest, generateApiManifest, loadApiManifest, readBootstrapScript, renderCompatibilityDoc } from "../src/api-manifest.mjs";
import { main } from "../src/cli.mjs";
import { doctorApplication } from "../src/doctor.mjs";
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
    expect(compatible).toMatchObject({ profile: "v0-strict", errors: 0, warnings: 0, files: 3 });

    const { lines, output } = capture();
    expect(await main(["doctor", join(fixtures, "unsupported")], output)).toBe(1);
    expect(lines.some(([, line]) => line.includes("HTML_CANVAS") && line.includes("native viewport")))
      .toBeTrue();
    expect(lines.some(([, line]) =>
      line.includes("WEB_STORAGE_MEMORY") && line.includes("gone when the application exits")))
      .toBeTrue();
    expect(lines.at(-1)[1]).toContain("1 errors, 2 warnings");
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
        `typeof Worker<"u"&&new Worker(u);`,
        `if(t.getContext)t.getContext("2d");`,
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
    expect(buildManifest(source).apis.find(entry => entry.api === "Worker").status).toBe("absent");
    const implemented = source
      .replace('"Worker", "SharedWorker"', '"SharedWorker"')
      .replace("const globals = {", "const globals = {\n    Worker,");
    expect(buildManifest(implemented).apis.find(entry => entry.api === "Worker").status)
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

  test("diagnoses what the manifest calls absent, and nothing it calls implemented", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-manifest-test-"));
    try {
      await writeFile(join(directory, "app.js"),
        "new Worker(url); customElements.define(); indexedDB.open('x'); window.open('/x');\n"
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

});
