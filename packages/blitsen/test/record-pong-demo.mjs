// Records docs/pong.gif. The frames come from the same document-animation harness
// the acceptance gate asserts on, so the published recording cannot drift away from
// what the tests actually verify. Needs ffmpeg on PATH for the GIF encode.
import { copyFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "../../..");
const libraryName = {
  linux: "libblitsen_node.so",
  darwin: "libblitsen_node.dylib",
  win32: "blitsen_node.dll",
}[process.platform];

if (!libraryName) throw new Error(`unsupported recording platform: ${process.platform}`);

const FRAMES = 480;
const WIDTH = 720;
const HEIGHT = 520;

// Two synthetic players tracking the ball, so the recording shows a rally rather
// than the serve prompt. Input goes through the same keydown path a human uses;
// nothing reaches into the game's state.
const setup = `{
  const game = document.getElementById("game");
  const ball = document.getElementById("ball");
  const held = new Set();
  const key = (type, name) => __blitsenDispatchKeyboardEvent(type,
    { bubbles: true, cancelable: true, key: name, code: name, repeat: false });
  const hold = (name, want) => {
    if (want === held.has(name)) return;
    if (want) { held.add(name); key("keydown", name); } else { held.delete(name); key("keyup", name); }
  };
  const centre = element => {
    const box = element.getBoundingClientRect();
    return box.y + box.height / 2;
  };
  const track = (paddle, up, down) => {
    const offset = centre(ball) - centre(document.getElementById(paddle));
    hold(up, offset < -6);
    hold(down, offset > 6);
  };
  const drive = () => {
    if (game.getAttribute("data-state") !== "playing") key("keydown", " ");
    else { track("left-paddle", "w", "s"); track("right-paddle", "ArrowUp", "ArrowDown"); }
    requestAnimationFrame(drive);
  };
  requestAnimationFrame(drive);
}`;

const build = Bun.spawnSync({
  cmd: ["cargo", "build", "-p", "blitsen-node"],
  cwd: repository,
  stdout: "inherit",
  stderr: "inherit",
});
if (build.exitCode !== 0) process.exit(build.exitCode);

const target = join(repository, "target", "debug");
const addon = join(target, "blitsen.node");
await copyFile(join(target, libraryName), addon);

const frames = await mkdtemp(join(tmpdir(), "blitsen-demo-"));
try {
  const native = (await import("node:module")).createRequire(import.meta.url)(addon);
  native.recordDocumentAnimationHarness(
    join(repository, "examples/pong/index.html"), setup, frames, FRAMES, WIDTH, HEIGHT,
  );
  const output = join(repository, "docs/pong.gif");
  const encode = Bun.spawnSync({
    cmd: [
      "ffmpeg", "-y", "-loglevel", "error", "-framerate", "60",
      "-i", join(frames, "frame-%05d.png"),
      "-vf", "fps=25,scale=640:-1:flags=lanczos,split[a][b];"
        + "[a]palettegen=max_colors=48[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
      output,
    ],
    stdout: "inherit",
    stderr: "inherit",
  });
  if (encode.exitCode !== 0) throw new Error("ffmpeg is required to encode the demo");
  console.log(`Recorded ${FRAMES} frames to ${output}`);
} finally {
  await rm(frames, { recursive: true, force: true });
}
