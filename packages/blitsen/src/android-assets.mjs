// The application's own files as they go into an APK, and the listing that
// travels with them (issue #148, writing what issue #144 designed).
//
// `crates/blitsen-host/src/apk.rs` is the reader and carries the argument for
// the layout; this is the writer, and everything below follows from that file
// rather than restating it. Three constants have to agree across the seam —
// `DEFAULT_ASSET_ROOT`, `ASSET_INDEX` and `INDEX_VERSION` — and they are
// repeated here rather than derived, because there is no build step that could
// derive a JavaScript constant from a Rust one. `cli-android.test.mjs` reads
// them back out of `apk.rs` and fails if the two drift.
//
// # Why there is an index at all
//
// `AAssetManager_openDir` lists one directory and not its subdirectories, so
// there is no walk of `assets/` available from the NDK. Nothing about *reading*
// a file needs the index — an asset is opened by name — so an APK without one
// still runs, and the reader treats its absence as "built without a listing"
// rather than as an error. What needs it is every question of the form "what is
// in here": the standalone check's asset count, and the report that stands in
// for `--bundle-report` on an artifact that can never be handed an argv.
//
// # Why the staging loop is not the desktop one
//
// `export.mjs` walks the same plan and rewrites the same references, and this
// deliberately does not call into it. The desktop loop is doing three more jobs
// at the same time — deciding whether the application needs a module loader,
// inspecting every carried `.node` addon against the target, and hashing each
// file into the bundle manifest — and none of the three exists here. There is
// no module-loader decision because the Android host is not selected per build;
// there are no addons because an APK has no Node-API host to load one; and
// there is no manifest hash because the APK signature covers the archive, which
// is the argument `apk.rs` makes for not recomputing a digest inside it.
//
// Pulling those apart to share the walk would put a fourth set of conditionals
// through the one loop that produces every shipping desktop artifact, to save
// about thirty lines. The two things that must not drift — `planIngest` and
// `rewriteRootRelativeReferences` — are shared, and they are the parts that
// decide what the application *is*.

import { copyFile, mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { dirname, extname, join } from "node:path";
import { planIngest, rewriteRootRelativeReferences } from "./export.mjs";

/// Where inside `assets/` a Blitsen application is packaged.
///
/// Must equal `blitsen_host::apk::DEFAULT_ASSET_ROOT`. Namespaced rather than
/// laid at the root of `assets/` because Gradle merges the assets of every
/// library in a build into one directory, so the root is shared.
export const ASSET_ROOT = "blitsen";

/// The listing's name, inside the application's own root.
///
/// Must equal `blitsen_host::apk::ASSET_INDEX`.
export const ASSET_INDEX = "blitsen.assets.json";

/// The only index format this writes.
///
/// Must equal `blitsen_host::apk::INDEX_VERSION`. The reader accepts anything
/// at or below its own, so an older host reads a newer package's files and
/// declines only its listing.
export const INDEX_VERSION = 1;

/// The extensions rewritten on the way in, matching the desktop export.
const REWRITTEN = [".html", ".htm", ".css"];

/**
 * The index for a set of staged files, as the bytes that go into the APK.
 *
 * Sorted by path and written without insignificant whitespace, so two builds of
 * the same application produce the same bytes — the reproducibility property
 * (#71) that the appended bundle has, kept for the artifact that replaces it.
 * The index does not record itself: it is a file in `assets/` like any other,
 * but a listing that includes its own length cannot be written, because writing
 * it changes the length.
 */
export function assetIndex(files) {
  const listed = files
    .filter(file => file.path !== ASSET_INDEX)
    .map(file => ({ path: file.path, bytes: file.bytes }))
    .sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0));
  return `${JSON.stringify({ version: INDEX_VERSION, files: listed })}\n`;
}

/**
 * Writes one application's files into a directory laid out as an APK's `assets/`.
 *
 * `directory` is the packager's `assets/` root, so what lands on disk is
 * `<directory>/blitsen/index.html` and the rest — exactly the tree
 * `ApkAssets::open_directory` reads, which is what makes an Android package
 * testable on a machine with no APK in it.
 *
 * Returns the manifest the index was written from, so a caller can report what
 * it packaged without reading the directory back.
 */
export async function stageAndroidAssets({
  root,
  directory,
  include = [],
  extra = new Map(),
  assetRoot = ASSET_ROOT,
}) {
  const plan = await planIngest(root, { include });
  const base = assetRoot ? join(directory, ...assetRoot.split("/")) : directory;
  const staged = [];
  for (const file of plan.files) {
    const destination = join(base, ...file.relative.split("/"));
    await mkdir(dirname(destination), { recursive: true });
    if (REWRITTEN.includes(extname(file.relative).toLowerCase())) {
      const resolutions = plan.resolutions.get(file.relative);
      const source = rewriteRootRelativeReferences(
        await readFile(file.absolute, "utf8"),
        file.relative,
        reference => resolutions?.get(reference) ?? null,
      );
      await writeFile(destination, source);
    } else {
      await copyFile(file.absolute, destination);
    }
    staged.push({ path: file.relative, bytes: (await stat(destination)).size });
  }
  // What the build adds rather than what the application shipped: the runtime
  // record, and the third-party notices the artifact owes (#121). They are
  // indexed like everything else, because they are files in the same place and
  // a listing that quietly omitted them would be wrong about the package.
  for (const [path, bytes] of [...extra].sort(([left], [right]) =>
    left < right ? -1 : left > right ? 1 : 0)) {
    const destination = join(base, ...path.split("/"));
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, bytes);
    staged.push({ path, bytes: bytes.length });
  }
  const index = assetIndex(staged);
  await mkdir(base, { recursive: true });
  await writeFile(join(base, ASSET_INDEX), index);
  return { files: staged, unreferenced: plan.unreferenced, index };
}
