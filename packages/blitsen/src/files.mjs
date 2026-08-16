import { readdir } from "node:fs/promises";
import { join, relative, sep } from "node:path";

export const HTML_EXTENSIONS = Object.freeze([".html", ".htm"]);
export const SCRIPT_EXTENSIONS = Object.freeze([".js", ".mjs", ".cjs"]);
export const REWRITTEN_EXTENSIONS = Object.freeze([...HTML_EXTENSIONS, ".css"]);
export const SCANNABLE_EXTENSIONS = Object.freeze([
  ...REWRITTEN_EXTENSIONS,
  ...SCRIPT_EXTENSIONS,
]);

/**
 * Recursively collects leaf entries below `root` as sorted, POSIX-style paths.
 *
 * A caller decides which leaves count as files because `Dirent` also exposes
 * platform-specific entry kinds. Symlinks never enter the result; callers can
 * reject them or deliberately ignore them through `onSymlink`.
 */
export async function walkFiles(root, { filter = () => true, onSymlink = () => {} } = {}) {
  const files = [];
  const visit = async directory => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const absolute = join(directory, entry.name);
      const file = {
        absolute,
        relative: relative(root, absolute).split(sep).join("/"),
      };
      if (entry.isSymbolicLink()) {
        await onSymlink(file);
      } else if (entry.isDirectory()) {
        await visit(absolute);
      } else if (filter(file, entry)) {
        files.push(file);
      }
    }
  };
  await visit(root);
  return files.sort((left, right) => left.relative.localeCompare(right.relative));
}
