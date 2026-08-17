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
import { copyFile, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { main } from "../src/cli.mjs";
import {
  extractFromTarball, fetchRuntime, hostTarget, resolveRuntime, runtimeCacheDir, TARGETS,
} from "../src/runtime.mjs";
import { capture, executableStub, nativeStub, phase2Name } from "./cli-support.mjs";

const VERSION = "9.9.9";
const npm = Bun.which("npm");

// A published target this host is not. `--target` naming the host is not a
// cross-target build — it takes the host path, which resolves and *opens* the
// host runtime — so every test here that means "another platform" has to ask
// for one rather than name a favourite (#134).
const elsewhere = () => (hostTarget() === "win32-x64" ? "linux-x64" : "win32-x64");

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
    // `join` is this host's, and the cache directory is built with it whichever
    // platform is named — so the expectation is spelled the same way rather
    // than asserting that this host is not Windows (#134).
    expect(runtimeCacheDir({ XDG_CACHE_HOME: "/xdg" }, "linux")).toBe(join("/xdg", "blitsen"));
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
    // `npm pack` on a cold Windows runner takes longer than the default 5s, and
    // a timeout there reads as a broken tarball reader (#134).
  }, 120_000);

  // Issue #121, on the one path that links a runtime this machine never
  // installed: an export reads its notices from beside the runtime, so a fetch
  // that brings back the binary alone produces an artifact that reports itself
  // uncleared for redistribution — the only export shape that could not legally
  // be shipped, and the one a `--target` build always takes.
  test("brings the target's notices back with its binary", async () => {
    if (!npm) return;
    await withWork(async work => {
      const source = join(work, "package");
      await mkdir(source, { recursive: true });
      await writeFile(join(source, "package.json"), JSON.stringify({
        name: "@blitsen/win32-x64", version: VERSION,
        files: ["blitsen.node", "NOTICES.txt", "NOTICES.json"],
      }));
      await writeFile(join(source, "blitsen.node"), nativeStub("win32-x64"));
      await writeFile(join(source, "NOTICES.txt"), "THIRD-PARTY NOTICES\n\nstub\n");
      await writeFile(join(source, "NOTICES.json"), '{"packages":[]}');
      const packed = Bun.spawnSync({
        cmd: [npm, "pack", source, "--pack-destination", work], cwd: work,
        stdout: "pipe", stderr: "pipe",
      });
      expect(packed.exitCode).toBe(0);
      const tarball = join(work, (await readdir(work)).find(file => file.endsWith(".tgz")));
      // Stands in for `npm pack` against a registry: it leaves a tarball where
      // the downloader looks for one.
      const run = async (_cmd, cwd) => {
        await copyFile(tarball, join(cwd, "fetched.tgz"));
        return { code: 0, stdout: "", stderr: "" };
      };
      const resolved = await fetchRuntime({
        target: "win32-x64", version: VERSION, cacheDir: join(work, "cache"), run,
      });
      expect(resolved.source).toBe("fetched");
      const beside = dirname(resolved.path);
      expect(await Bun.file(join(beside, "NOTICES.txt")).text()).toContain("THIRD-PARTY NOTICES");
      expect(await Bun.file(join(beside, "NOTICES.json")).exists()).toBeTrue();
    });
  }, 120_000);

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
        target: elsewhere(), version: VERSION, env: {}, require: resolver, run, cacheDir: work,
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
      const previousRuntime = process.env.BLITSEN_RUNTIME_PATH;
      process.env.BLITSEN_CACHE_DIR = cache;
      // An addon named in the environment outranks everything, which would
      // quietly make all three builds link this host's runtime. So does a Phase
      // 2 runtime named there — the half that was missed, and the half that
      // decides what the artifact *is*: with BLITSEN_RUNTIME_PATH set, this
      // test asked for win32-x64 and got an ELF, and only said so on a runner
      // whose ELF was the wrong architecture as well (#134).
      delete process.env.BLITSEN_NATIVE_PATH;
      delete process.env.BLITSEN_RUNTIME_PATH;

      // Keep the Linux entry foreign to its ARM64 runner. That smoke job stages
      // its own real runtime before this suite, so asking it for linux-arm64
      // takes the host path and stops testing the cache-backed cross-target path.
      const linuxTarget = hostTarget() === "linux-arm64" ? "linux-x64" : "linux-arm64";
      const formats = {
        "win32-x64": /PE32\+ executable.*x86-64/,
        // `file` words this differently per host — macOS says "64-bit executable
        // arm64" where Linux says "64-bit arm64" — so the pattern reads the
        // format and the architecture and not the sentence between them.
        "darwin-arm64": /Mach-O 64-bit.*arm64/,
        [linuxTarget]: linuxTarget === "linux-x64"
          ? /ELF 64-bit.*x86-64/
          : /ELF 64-bit.*(aarch64|ARM)/,
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
      if (previousRuntime !== undefined) process.env.BLITSEN_RUNTIME_PATH = previousRuntime;
    }, 240_000);
  }, 240_000);

  test("refuses a runtime built for a platform other than the target", async () => {
    await withWork(async work => {
      const application = join(work, "dist");
      await mkdir(application, { recursive: true });
      await writeFile(join(application, "index.html"), "<!doctype html><html><body>x</body></html>");
      // A host addon reached through BLITSEN_NATIVE_PATH, with another
      // platform's target: it links, ships, and then fails at dlopen in front of
      // whoever runs it. Both halves are derived from this host, because a
      // `--target` naming *this* host is not a cross-target build at all — it
      // resolves the host runtime and opens it, and on Windows that failed at
      // LoadLibrary before reaching the check under test (#134).
      const host = join(work, "host.node");
      await writeFile(host, nativeStub(hostTarget()));
      const { output, lines } = capture();
      const previous = process.env.BLITSEN_NATIVE_PATH;
      process.env.BLITSEN_NATIVE_PATH = host;
      try {
        const code = await main(
          ["build", application, "--target", elsewhere(), "--outfile", join(work, "App")], output);
        expect(code).toBe(1);
        expect(lines.map(([, line]) => line).join("\n"))
          .toContain(`the linked runtime is built for ${hostTarget()}`);
      } finally {
        if (previous === undefined) delete process.env.BLITSEN_NATIVE_PATH;
        else process.env.BLITSEN_NATIVE_PATH = previous;
      }
    });
  });

  // The other half of the same refusal, and the one that decides what the
  // artifact is rather than what it loads: a Phase 2 export *is* the runtime
  // executable with the application appended. `BLITSEN_RUNTIME_PATH` outranks
  // the target's own runtime, so without this check a build that named another
  // platform produced this host's executable under that platform's file name —
  // a `.exe` that is an ELF, reported as a success (#134).
  test("refuses a Phase 2 runtime built for a platform other than the target", async () => {
    await withWork(async work => {
      const application = join(work, "dist");
      await mkdir(application, { recursive: true });
      await writeFile(join(application, "index.html"), "<!doctype html><html><body>x</body></html>");
      const cache = join(work, "cache");
      const target = elsewhere();
      const addon = join(work, "target.node");
      const runtime = join(work, "host-runtime");
      await writeFile(addon, nativeStub(target));
      await writeFile(runtime, executableStub(hostTarget()));
      const { output, lines } = capture();
      const previous = {
        native: process.env.BLITSEN_NATIVE_PATH,
        runtime: process.env.BLITSEN_RUNTIME_PATH,
        cache: process.env.BLITSEN_CACHE_DIR,
      };
      // The addon matches the target, so the only thing wrong is the executable
      // underneath it — which is what this is about.
      process.env.BLITSEN_NATIVE_PATH = addon;
      process.env.BLITSEN_RUNTIME_PATH = runtime;
      process.env.BLITSEN_CACHE_DIR = cache;
      try {
        const code = await main(
          ["build", application, "--target", target, "--outfile", join(work, "App")], output);
        expect(code).toBe(1);
        const said = lines.map(([, line]) => line).join("\n");
        expect(said).toContain(`the linked Phase 2 runtime is built for ${hostTarget()}`);
        expect(said).toContain("BLITSEN_RUNTIME_PATH");
      } finally {
        for (const [name, value] of [["BLITSEN_NATIVE_PATH", previous.native],
          ["BLITSEN_RUNTIME_PATH", previous.runtime], ["BLITSEN_CACHE_DIR", previous.cache]]) {
          if (value === undefined) delete process.env[name];
          else process.env[name] = value;
        }
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
