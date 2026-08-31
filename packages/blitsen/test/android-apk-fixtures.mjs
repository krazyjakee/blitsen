import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const withAndroidWork = async run => {
  const directory = await mkdtemp(join(tmpdir(), "blitsen-android-"));
  try {
    return await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
};

/** A small application on disk, with one reference for the rewriter to follow. */
export async function androidApplication(directory) {
  const root = join(directory, "dist");
  await mkdir(join(root, "assets"), { recursive: true });
  await writeFile(join(root, "index.html"),
    "<html><link rel=stylesheet href=\"/assets/app.css\"><script type=module src=\"/app.js\">"
    + "</script></html>");
  await writeFile(join(root, "app.js"), "export const ready = true;\n");
  await writeFile(join(root, "assets/app.css"), "body { color: red }\n");
  await writeFile(join(root, "orphan.txt"), "not reachable\n");
  return root;
}
