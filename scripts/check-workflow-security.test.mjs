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
});
