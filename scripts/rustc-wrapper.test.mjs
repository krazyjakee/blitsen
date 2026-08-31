// The compiler wrapper has to be transparent (issue: `CARGO_BIN_EXE_*`).
//
//     bun test scripts/rustc-wrapper.test.mjs
//
// Cargo passes an integration test the path of every binary in its package as
// `CARGO_BIN_EXE_<target>`, spelled exactly as the target is: `blitsen-runtime`
// carries a hyphen, which is not a valid shell identifier. A `/bin/sh` wrapper
// is dash on Debian and Ubuntu, and dash drops variables it cannot name out of
// the environment it hands on — so the three tests in `crates/blitsen-runtime`
// stopped compiling, naming an environment variable rather than the wrapper
// that removed it. These run the script through its own shebang, because the
// shebang is the thing under test.
import { describe, expect, test } from "bun:test";

const wrapper = "scripts/rustc-wrapper.sh";

function run(command, env = {}) {
  return Bun.spawnSync([wrapper, ...command], {
    // sccache is not installed in CI and must not be required by these tests;
    // the pass-through branch is the one that has to stay transparent anyway.
    env: { ...process.env, BLITSEN_DISABLE_SCCACHE: "1", ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
}

const echoEnv = (name) => ["bun", "-e", `console.log(process.env[${JSON.stringify(name)}] ?? "MISSING")`];

describe("the rustc wrapper is transparent", () => {
  test("forwards environment variables whose names Cargo, not the shell, chose", () => {
    const result = run(echoEnv("CARGO_BIN_EXE_blitsen-runtime"), {
      "CARGO_BIN_EXE_blitsen-runtime": "/target/debug/blitsen-runtime",
    });
    expect(result.stdout.toString().trim()).toBe("/target/debug/blitsen-runtime");
  });

  test("forwards ordinary environment variables", () => {
    const result = run(echoEnv("CARGO_PKG_NAME"), { CARGO_PKG_NAME: "blitsen-runtime" });
    expect(result.stdout.toString().trim()).toBe("blitsen-runtime");
  });

  test("forwards arguments and the exit status of what it wraps", () => {
    const ok = run(["bun", "-e", "console.log(Bun.argv.slice(-2).join(','))", "one", "two"]);
    expect(ok.stdout.toString().trim()).toBe("one,two");
    expect(ok.exitCode).toBe(0);
    expect(run(["bun", "-e", "process.exit(3)"]).exitCode).toBe(3);
  });
});
