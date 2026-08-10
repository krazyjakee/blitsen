import { describe, expect, test } from "bun:test";
import { main } from "../src/cli.mjs";

function capture() {
  const lines = [];
  return {
    lines,
    output: {
      log: (line) => lines.push(["out", line]),
      error: (line) => lines.push(["err", line]),
    },
  };
}

describe("CLI skeleton", () => {
  test("prints help", () => {
    const { lines, output } = capture();
    expect(main(["--help"], output)).toBe(0);
    expect(lines[0][1]).toContain("Usage: blitsen");
  });

  test("fails clearly for unimplemented commands", () => {
    const { lines, output } = capture();
    expect(main(["build", "dist"], output)).toBe(1);
    expect(lines).toEqual([["err", "blitsen: build is not implemented yet"]]);
  });
});
