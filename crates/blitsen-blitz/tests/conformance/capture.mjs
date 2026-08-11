// Rebuilds the framework-derived corpus cases from real build output.
//
// A framework's markup is the thing the corpus is supposed to be about, and it
// only exists after the bundle's own JavaScript has run — which needs the whole
// runtime, not just the renderer. So it is captured here, once, and committed:
// the corpus itself stays a set of static documents that `cargo test` can render
// with no JavaScript engine, no bundler and no network.
//
// The captured markup is verbatim. Only two things are added to it: the app's
// own stylesheet, inlined because the corpus resolves relative URLs against the
// fixtures directory rather than the bundle's, and a rule pinning every element
// to the corpus font. That pin is what makes the case portable — without it the
// text metrics, and therefore every box below the first line of text, would be
// whatever fonts the host happens to have installed.
//
// usage: bun crates/blitsen-blitz/tests/conformance/capture.mjs
import { copyFile, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "../../../..");
const cases = join(import.meta.dir, "cases");

const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];
if (!libraryName) throw new Error(`unsupported capture target: ${process.platform}`);

const sources = [
  {
    name: "react-vite",
    // The M3b acceptance bundle: ordinary `vite build` output, unmodified. The
    // relative-base variant only because a corpus case is loaded from disk with
    // no ingest step to rewrite `/assets/...` for it.
    entrypoint: "examples/vite-react/dist-relative/index.html",
    stylesheet: "examples/vite-react/dist-relative/assets/index-BmMNvTTY.css",
    width: 800,
    height: 600,
    header: `Real framework output: the React tree that the committed \`vite build\` bundle
in examples/vite-react builds at runtime, captured after its own JavaScript ran
and then frozen. Layout here is Tailwind-free but otherwise ordinary: a centred
fixed-width shell, a flex card row, borders and radii.

This case is a change detector, not a correctness oracle. Nobody derived what
this markup ought to look like; the golden image records what it does look like,
so that a renderer change that moves it says so. The few boxes below are the
exception — they follow from the stylesheet without knowing anything about text:
the shell is 720 wide and \`margin: 48px auto\` centres it in 800, so it starts at
40; inside its 1px border and 32px padding, 720 - 2 - 64 = 654 is left from
x = 73, and three \`flex: 1\` cards with two 12px gaps take (654 - 24) / 3 = 210
each, starting at 73, 295 and 517. Heights are written \`-\` because they follow
from the pinned font rather than from the stylesheet, and recording them here
would be circular.

@size 800 600
@box .shell 40 48 720 -
@box .cards 73 - 654 -
@box .cards > article:nth-child(1) 73 - 210 -
@box .cards > article:nth-child(2) 295 - 210 -
@box .cards > article:nth-child(3) 517 - 210 -`,
  },
];

// Every glyph in this face is a solid em block, so a captured case renders as
// the shape of its own layout with nothing of the host in it.
const PINNED_FONT = `@font-face { font-family: "Block ASCII"; src: url("block-ascii.ttf") format("truetype") }
      * { font-family: "Block ASCII" !important }`;

const build = Bun.spawnSync({
  cmd: ["cargo", "build", "--release", "-p", "blitsen-node"],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
if (build.exitCode !== 0) process.exit(build.exitCode);
const target = join(repository, "target/release");
const addon = join(target, "blitsen.node");
await copyFile(join(target, libraryName), addon);
const native = createRequire(import.meta.url)(addon);

for (const source of sources) {
  native.runDocumentScriptsHarness(
    join(repository, source.entrypoint),
    source.width,
    source.height,
  );
  // React's scheduler defers the first render off the load event, so the tree
  // only exists once the host loop has drained the tasks it queued.
  await Bun.sleep(50);
  const captured = native.captureDocumentHarnessHtml();
  const start = captured.indexOf("<body");
  const end = captured.lastIndexOf("</body>");
  if (start === -1 || end === -1) throw new Error(`${source.name} captured no body`);
  const body = captured.slice(start, end + "</body>".length);
  const stylesheet = (await readFile(join(repository, source.stylesheet), "utf8")).trim();

  await writeFile(
    join(cases, `${source.name}.html`),
    `<!-- conformance
${source.header}
-->
<!doctype html>
<html>
  <head>
    <style>${stylesheet}</style>
    <style>
      ${PINNED_FONT}
    </style>
  </head>
  ${body}
</html>
`,
  );
  console.log(`captured ${source.name} from ${source.entrypoint}`);
}
