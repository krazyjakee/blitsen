import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join } from "node:path";
import {
  HTML_EXTENSIONS, REWRITTEN_EXTENSIONS, SCANNABLE_EXTENSIONS, SCRIPT_EXTENSIONS, walkFiles,
} from "../src/files.mjs";

describe("shared file policy", () => {
  test("walks nested leaves in stable POSIX order and applies a caller filter", async () => {
    const root = await mkdtemp(join(tmpdir(), "blitsen-files-"));
    try {
      await mkdir(join(root, "nested"));
      await writeFile(join(root, "z.txt"), "z");
      await writeFile(join(root, "app.js"), "js");
      await writeFile(join(root, "nested", "chunk.mjs"), "mjs");
      await writeFile(join(root, "nested", "style.css"), "css");

      const files = await walkFiles(root, {
        filter: file => SCRIPT_EXTENSIONS.includes(extname(file.relative)),
      });
      expect(files.map(file => file.relative)).toEqual(["app.js", "nested/chunk.mjs"]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test.skipIf(process.platform === "win32")("leaves symlink policy to the caller", async () => {
    const root = await mkdtemp(join(tmpdir(), "blitsen-files-link-"));
    try {
      await writeFile(join(root, "app.js"), "js");
      await symlink(join(root, "app.js"), join(root, "linked.js"));
      const links = [];
      const files = await walkFiles(root, { onSymlink: file => links.push(file.relative) });
      expect(files.map(file => file.relative)).toEqual(["app.js"]);
      expect(links).toEqual(["linked.js"]);
      await expect(walkFiles(root, {
        onSymlink: file => {
          throw new Error(`rejected ${file.relative}`);
        },
      })).rejects.toThrow("rejected linked.js");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test("derives rewriting and scanning extensions from the shared groups", () => {
    expect(REWRITTEN_EXTENSIONS).toEqual([...HTML_EXTENSIONS, ".css"]);
    expect(SCANNABLE_EXTENSIONS).toEqual([...REWRITTEN_EXTENSIONS, ...SCRIPT_EXTENSIONS]);
  });
});
