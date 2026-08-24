import { describe, expect, test } from "bun:test";
import { checkWorkflowSource } from "./check-workflow-security.mjs";

const pinned = "actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0";

describe("workflow security policy", () => {
  test("accepts pinned actions and trusted expressions", () => {
    const errors = checkWorkflowSource(`
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: ${pinned}
      - run: echo "\${{ matrix.target }}"
`);
    expect(errors).toEqual([]);
  });

  test("rejects mutable actions and missing version comments", () => {
    const errors = checkWorkflowSource(`
jobs:
  test:
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
`);
    expect(errors).toHaveLength(3);
    expect(errors.join("\n")).toContain("not pinned");
    expect(errors.join("\n")).toContain("version comment");
  });

  test("rejects untrusted expressions in multiline and inline scripts", () => {
    for (const expression of [
      "inputs.tag",
      "github.event.pull_request.title",
      "github.head_ref",
      "github.ref_name",
      "github.actor",
      "github.triggering_actor",
    ]) {
      const errors = checkWorkflowSource(`
jobs:
  test:
    steps:
      - run: |
          printf '%s\\n' "\${{ ${expression} }}"
      - run: echo "\${{ ${expression} }}"
`);
      expect(errors).toHaveLength(2);
      expect(errors[0]).toContain("through env");
    }
  });

  test("rejects triggers that run elevated against attacker-controlled refs", () => {
    for (const trigger of ["pull_request_target", "workflow_run"]) {
      for (const spelling of [
        `on: ${trigger}`,
        `on: [push, ${trigger}]`,
        `on:\n  ${trigger}:\n    branches: [main]`,
      ]) {
        const errors = checkWorkflowSource(`
${spelling}
jobs:
  test:
    steps:
      - run: echo ok
`);
        expect(errors).toHaveLength(1);
        expect(errors[0]).toContain(trigger);
        expect(errors[0]).toContain("requires explicit review");
      }
    }
  });

  test("accepts the ordinary pull_request trigger", () => {
    const errors = checkWorkflowSource(`
on:
  push:
    branches: [main]
  pull_request:
jobs:
  test:
    steps:
      - run: echo ok
`);
    expect(errors).toEqual([]);
  });
});
