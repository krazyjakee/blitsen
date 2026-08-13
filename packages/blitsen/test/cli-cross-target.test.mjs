// Cross-target export, and the on-demand runtime fetch behind it (issue #72).
//
// The point of these is that a build for another platform produces that
// platform's executable rather than this one's quietly renamed — so the format
// of the linked artifact is read back out of its own header, not inferred from
// the flag that asked for it.
//
// Nothing here reaches the network. `fetchRuntime` takes the command runner as
// an argument, and the tarball it is handed is a real one built by `npm pack`,
// so the extraction is exercised against npm's actual output rather than a
// hand-written archive that might agree with a hand-written reader.
import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.mjs";
import {
  extractFromTarball, fetchRuntime, resolveRuntime, runtimeCacheDir, TARGETS,
} from "../src/runtime.mjs";
import { capture, executableStub, nativeStub, phase2Name } from "./cli-support.mjs";

const VERSION = "9.9.9";
const npm = Bun.which("npm");

const withWork = async run => {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-cross-target-"));
  try {
    return await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
};

/**
 * Seeds the cache the way a completed fetch leaves it.
 *
 * Both halves of a platform package: the addon the export checks against its
 * target, and the Phase 2 executable it links into.
 */
const seedCache = async (cacheDir, target, version = VERSION) => {
  const directory = join(cacheDir, "runtimes", version, target);
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, "blitsen.node"), nativeStub(target));
  await writeFile(join(directory, phase2Name(target)), executableStub(target));
  return join(directory, "blitsen.node");
};

describe("cross-target export", () => {
  test("caches per version and target, so one ABI cannot occupy another's slot", () => {
    const cache = runtimeCacheDir({ BLITSEN_CACHE_DIR: "/tmp/explicit" });
    expect(cache).toBe("/tmp/explicit");
    expect(runtimeCacheDir({ XDG_CACHE_HOME: "/xdg" }, "linux")).toBe("/xdg/blitsen");
    expect(runtimeCacheDir({ LOCALAPPDATA: "C:\\Users\\a\\AppData\\Local" }, "win32"))
      .toContain("blitsen");
    expect(runtimeCacheDir({}, "darwin")).toContain(join("Library", "Caches", "blitsen"));
  });

  test("reads the addon out of a tarball npm actually produced", async () => {
    if (!npm) return;
    await withWork(async work => {
      const source = join(work, "package");
      await mkdir(source, { recursive: true });
      await writeFile(join(source, "package.json"), JSON.stringify({
        name: "@blitsen/win32-x64", version: VERSION, files: ["blitsen.node"],
      }));
      const addon = nativeStub("win32-x64");
      await writeFile(join(source, "blitsen.node"), addon);
      // A second file, so the reader has to find the right member rather than
      // returning whichever one it reached first.
      await writeFile(join(source, "README.md"), "# not the addon\n");
      const packed = Bun.spawnSync({
        cmd: [npm, "pack", source, "--pack-destination", work], cwd: work,
        stdout: "pipe", stderr: "pipe",
      });
      expect(packed.exitCode).toBe(0);
      const tarball = (await readdir(work)).find(file => file.endsWith(".tgz"));
      const bytes = await readFile(join(work, tarball));

      const extracted = extractFromTarball(bytes, "package/blitsen.node");
      expect(extracted).not.toBeNull();
      expect(Buffer.from(extracted).equals(addon)).toBeTrue();
      expect(extractFromTarball(bytes, "package/absent.node")).toBeNull();
    });
  });

  test("uses the cached runtime rather than downloading again", async () => {
    await withWork(async work => {
      await seedCache(work, "darwin-arm64");
      let downloads = 0;
      const resolved = await fetchRuntime({
        target: "darwin-arm64", version: VERSION, cacheDir: work,
        run: async () => { downloads += 1; return { code: 1, stdout: "", stderr: "" }; },
      });
      expect(downloads).toBe(0);
      expect(resolved.source).toBe("cache");
      expect(resolved.package).toBe("@blitsen/darwin-arm64");
    });
  });

  test("says what to do when a target's package is not published", async () => {
    await withWork(async work => {
      const failure = fetchRuntime({
        target: "win32-arm64", version: VERSION, cacheDir: work,
        run: async () => ({ code: 1, stdout: "",
          stderr: "npm error code E404\nnpm error 404 Not Found - GET https://registry.npmjs.org/@blitsen%2fwin32-arm64" }),
      });
      // Names the target, says it is the registry rather than the machine, and
      // gives the two ways out.
      await expect(failure).rejects.toThrow("is not published");
      await expect(failure).rejects.toThrow("win32-arm64 host");
      await expect(failure).rejects.toThrow("BLITSEN_NATIVE_PATH");
      await expect(failure).rejects.toThrow("404 Not Found");
    });
  });

  test("reports a download failure that is not a missing package as itself", async () => {
    await withWork(async work => {
      const failure = fetchRuntime({
        target: "linux-arm64", version: VERSION, cacheDir: work,
        run: async () => ({ code: 1, stdout: "", stderr: "npm error network ETIMEDOUT" }),
      });
      await expect(failure).rejects.toThrow("could not download");
      await expect(failure).rejects.toThrow("ETIMEDOUT");
    });
  });

  test("only a cross-target build is allowed to reach the network for a runtime", async () => {
    await withWork(async work => {
      let downloads = 0;
      const run = async () => { downloads += 1; return { code: 1, stdout: "", stderr: "E404" }; };
      const resolver = { resolve() { throw new Error("not installed"); } };
      // Default: no fetch. A host build must not start downloading because a
      // checkout happens to be missing its own addon.
      await expect(resolveRuntime({
        target: "win32-arm64", version: VERSION, env: {}, require: resolver, run, cacheDir: work,
      })).rejects.toThrow();
      expect(downloads).toBe(0);
    });
  });

  // The end of the whole feature: the artifact's own header says which platform
  // it is for. A flag that was accepted and then quietly built for the host
  // would pass every check above and fail only in a user's hands.
  test("builds an executable in the target's own format", async () => {
    await withWork(async work => {
      const application = join(work, "dist");
      await mkdir(application, { recursive: true });
      await writeFile(join(application, "index.html"),
        "<!doctype html><html><body><h1>cross</h1></body></html>");
      const cache = join(work, "cache");
      const previousCache = process.env.BLITSEN_CACHE_DIR;
      const previousNative = process.env.BLITSEN_NATIVE_PATH;
      process.env.BLITSEN_CACHE_DIR = cache;
      // An addon named in the environment outranks everything, which would
      // quietly make all three builds link this host's runtime.
      delete process.env.BLITSEN_NATIVE_PATH;

      const formats = {
        "win32-x64": /PE32\+ executable.*x86-64/,
        "darwin-arm64": /Mach-O 64-bit arm64/,
        "linux-arm64": /ELF 64-bit.*(aarch64|ARM)/,
      };
      for (const [target, expected] of Object.entries(formats)) {
        await seedCache(cache, target, await import("../src/runtime.mjs")
          .then(module => module.packageVersion()));
        const outfile = join(work, `App-${target}`);
        const { output } = capture();
        const code = await main(
          ["build", application, "--target", target, "--outfile", outfile], output);
        expect(code).toBe(0);
        // bun appends .exe when it targets Windows and the path has no extension.
        const linked = await Bun.file(outfile).exists() ? outfile : `${outfile}.exe`;
        const described = Bun.spawnSync({ cmd: ["file", "-b", linked], stdout: "pipe" })
          .stdout.toString();
        expect(described).toMatch(expected);
      }
      if (previousCache === undefined) delete process.env.BLITSEN_CACHE_DIR;
      else process.env.BLITSEN_CACHE_DIR = previousCache;
      if (previousNative !== undefined) process.env.BLITSEN_NATIVE_PATH = previousNative;
    }, 240_000);
  }, 240_000);

  test("refuses a runtime built for a platform other than the target", async () => {
    await withWork(async work => {
      const application = join(work, "dist");
      await mkdir(application, { recursive: true });
      await writeFile(join(application, "index.html"), "<!doctype html><html><body>x</body></html>");
      // A host addon reached through BLITSEN_NATIVE_PATH, with a Windows target:
      // it links, ships, and then fails at dlopen in front of whoever runs it.
      const host = join(work, "host.node");
      await writeFile(host, nativeStub("linux-x64"));
      const { output, lines } = capture();
      const previous = process.env.BLITSEN_NATIVE_PATH;
      process.env.BLITSEN_NATIVE_PATH = host;
      try {
        const code = await main(
          ["build", application, "--target", "win32-x64", "--outfile", join(work, "App")], output);
        expect(code).toBe(1);
        expect(lines.map(([, line]) => line).join("\n"))
          .toContain("the linked runtime is built for linux-x64");
      } finally {
        if (previous === undefined) delete process.env.BLITSEN_NATIVE_PATH;
        else process.env.BLITSEN_NATIVE_PATH = previous;
      }
    });
  });

  test("translates every supported target, and only those, into a bun target", async () => {
    const source = await readFile(join(import.meta.dir, "../src/export.mjs"), "utf8");
    const table = /const BUN_TARGETS = \{([\s\S]*?)\};/.exec(source);
    expect(table).not.toBeNull();
    const named = [...table[1].matchAll(/"([\w-]+)":\s*"bun-[\w-]+"/g)].map(([, target]) => target);
    expect(named.sort()).toEqual([...TARGETS].sort());
  });
});
