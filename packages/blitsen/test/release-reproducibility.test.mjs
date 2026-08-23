import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { compareReleaseBuilds, hashReleaseArtifacts }
  from "../../../scripts/compare-release-builds.mjs";
import { repository } from "./build-addon.mjs";

function peFixture({ timestamp, payload = Buffer.alloc(256) }) {
  const bytes = Buffer.alloc(0x300);
  bytes.write("MZ", 0, "ascii");
  bytes.writeUInt32LE(0x80, 0x3c);
  bytes.write("PE\0\0", 0x80, "ascii");
  bytes.writeUInt16LE(0x8664, 0x84);
  bytes.writeUInt16LE(1, 0x86);
  bytes.writeUInt32LE(timestamp, 0x88);
  bytes.writeUInt16LE(0xf0, 0x94);
  bytes.write(".rdata\0\0", 0x188, "ascii");
  bytes.writeUInt32LE(256, 0x198);
  bytes.writeUInt32LE(0x200, 0x19c);
  payload.copy(bytes, 0x200, 0, Math.min(payload.length, 256));
  return bytes;
}

describe("release reproducibility", () => {
  test("compares both unsigned artifacts and names the first differing byte", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-repro-"));
    const first = join(directory, "first");
    const second = join(directory, "second");
    await mkdir(first);
    await mkdir(second);
    await Bun.write(join(first, "addon"), Buffer.from([1, 2, 3, 4]));
    await Bun.write(join(first, "runtime"), Buffer.from([5, 6, 7, 8]));
    await Bun.write(join(second, "addon"), Buffer.from([1, 2, 3, 4]));
    await Bun.write(join(second, "runtime"), Buffer.from([5, 6, 9, 8]));
    try {
      const lines = [];
      await expect(compareReleaseBuilds({
        firstRoot: first, secondRoot: second, library: "addon", executable: "runtime",
        output: line => lines.push(line), summaryPath: null,
      })).rejects.toThrow("first differing byte 2");
      expect(lines.join("\n")).toContain("clean build A blitsen.node:");
      expect(lines.join("\n")).toContain("clean build B runtime:");
      expect(lines.join("\n")).toMatch(/[0-9a-f]{64}/);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("records matching SHA-256 hashes without changing the artifacts", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-repro-hash-"));
    try {
      await writeFile(join(directory, "addon"), "addon");
      await writeFile(join(directory, "runtime"), "runtime");
      const before = await readFile(join(directory, "addon"));
      const records = await hashReleaseArtifacts({
        root: directory, library: "addon", executable: "runtime", output: () => {},
        summaryPath: null,
      });
      expect(records).toHaveLength(2);
      expect(records.every(record => /^[0-9a-f]{64}$/.test(record.hash))).toBeTrue();
      expect(await readFile(join(directory, "addon"))).toEqual(before);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("explains /Brepro-style PE timestamps separately from section bytes", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-pe-repro-"));
    const first = join(directory, "first", "target", "release");
    const second = join(directory, "second", "target", "release");
    await mkdir(first, { recursive: true });
    await mkdir(second, { recursive: true });
    await writeFile(join(first, "addon"), peFixture({ timestamp: 0x12345678 }));
    await writeFile(join(second, "addon"), peFixture({ timestamp: 0x87654321 }));
    await writeFile(join(first, "runtime"), "same");
    await writeFile(join(second, "runtime"), "same");
    try {
      await expect(compareReleaseBuilds({
        firstRoot: first, secondRoot: second, library: "addon", executable: "runtime",
        output: () => {}, summaryPath: null,
      })).rejects.toThrow(
        /PE COFF TimeDateStamp A=0x12345678, B=0x87654321;.*no PE section payload differences/,
      );
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("names changed PE sections and leaked checkout-root spellings", async () => {
    const directory = await mkdtemp(join(tmpdir(), "blitsen-pe-path-"));
    const firstSource = join(directory, "first");
    const secondSource = join(directory, "other");
    const first = join(firstSource, "target", "release");
    const second = join(secondSource, "target", "release");
    await mkdir(first, { recursive: true });
    await mkdir(second, { recursive: true });
    const firstPayload = Buffer.from(firstSource.replaceAll("/", "\\"));
    const secondPayload = Buffer.from(secondSource.replaceAll("/", "\\"));
    await writeFile(join(first, "addon"), peFixture({ timestamp: 1, payload: firstPayload }));
    await writeFile(join(second, "addon"), peFixture({ timestamp: 2, payload: secondPayload }));
    await writeFile(join(first, "runtime"), "same");
    await writeFile(join(second, "runtime"), "same");
    try {
      await expect(compareReleaseBuilds({
        firstRoot: first, secondRoot: second, library: "addon", executable: "runtime",
        output: () => {}, summaryPath: null,
      })).rejects.toThrow(/changed PE sections: \.rdata\(256B\).*checkout-root occurrences A=/);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("passes both Windows path spellings to rustc and the native compiler", async () => {
    const script = join(repository, "scripts/build-release-runtime.sh");
    const child = Bun.spawn([
      "bash", "-c", `
        cargo() {
          printf '%s' "$CARGO_ENCODED_RUSTFLAGS" | tr '\\037' '\\n'
          printf '\\nCFLAGS=%s\\n' "$CFLAGS"
        }
        pwd() {
          if [[ "\${1:-}" = -W ]]; then
            printf 'D:/a/blitsen/blitsen\\n'
          else
            builtin pwd "$@"
          fi
        }
        export -f cargo pwd
        OSTYPE=msys RUSTFLAGS='--cfg inherited' bash "$1" win32-x64
      `, "release-build-test", script,
    ], { cwd: repository, stdout: "pipe", stderr: "pipe" });
    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    expect(stderr).toBe("");
    expect(exitCode).toBe(0);
    expect(stdout).toContain("--cfg\ninherited\n");
    expect(stdout).toContain("--remap-path-prefix=D:/a/blitsen/blitsen=/src/blitsen\n");
    expect(stdout).toContain("--remap-path-prefix=D:\\a\\blitsen\\blitsen=/src/blitsen\n");
    expect(stdout).toContain("-C\ntarget-feature=+crt-static\n-C\nlink-arg=/Brepro");
    expect(stdout).toContain("CFLAGS=/pathmap:D:/a/blitsen/blitsen=/src/blitsen ");
    expect(stdout).toContain("/pathmap:D:\\a\\blitsen\\blitsen=/src/blitsen");
  });

  test("gates one pinned native runner per executable format before signing", async () => {
    const workflow = await readFile(join(repository, ".github/workflows/release.yml"), "utf8");
    const build = await readFile(join(repository, "scripts/build-release-runtime.sh"), "utf8");
    const addonBuild = await readFile(join(repository, "crates/blitsen-node/build.rs"), "utf8");
    const selected = [...workflow.matchAll(
      /- target: ([^\n]+)\n(?: {12}[^\n]+\n){3} {12}reproducible: true/g,
    )].map(match => match[1]);
    expect(selected).toEqual(["linux-x64", "darwin-x64", "win32-x64"]);
    expect(workflow).toContain("scripts/build-release-runtime.sh");
    expect(workflow).toContain("scripts/compare-release-builds.mjs --compare");
    expect(workflow.indexOf("name: Verify unsigned reproducibility"))
      .toBeLessThan(workflow.indexOf("name: Sign (macOS)"));
    expect(build).toContain("--remap-path-prefix=");
    expect(build).toContain("CARGO_ENCODED_RUSTFLAGS");
    expect(build).toContain("source_root_backslash");
    expect(build).toContain("-ffile-prefix-map=");
    expect(build).toContain("/pathmap:");
    expect(build).toContain("/Brepro");
    expect(build).toContain("SOURCE_DATE_EPOCH");
    expect(addonBuild).toContain("rustc-cdylib-link-arg=-Wl,-install_name,@rpath/blitsen.node");
  });
});
