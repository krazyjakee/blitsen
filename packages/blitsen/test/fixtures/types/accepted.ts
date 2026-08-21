// What the published types must accept: the documented way to use every
// `blitsen/*` subpath, plus `<blitsen-view>`.
import { defineConfig } from "blitsen";
import app from "blitsen/app";
import clipboard from "blitsen/clipboard";
import dialog from "blitsen/dialog";
import nativeWindow from "blitsen/window";
import type { ClipboardImage } from "blitsen/clipboard";
import type { Monitor } from "blitsen/window";

// Feature detection is the documented pattern, because a capability the running
// version does not implement is `undefined` rather than an error. It must narrow
// the member to something callable with the right signature.
if (app.dataDir) {
  const directory: string = app.dataDir("demo");
  void directory;
}
if (app.requestSingleInstanceLock) {
  const held: boolean = app.requestSingleInstanceLock("demo", invocation => {
    const [first]: readonly string[] = invocation.argv;
    void first;
    void invocation.cwd;
  });
  void held;
}
if (clipboard.readText) {
  const text: string | null = clipboard.readText();
  void text;
}
if (clipboard.writeImage) {
  clipboard.writeImage({ width: 2, height: 2, data: new Uint8Array(16) });
}
if (nativeWindow.setFullscreen) nativeWindow.setFullscreen(true);
if (nativeWindow.setCursorGrab) nativeWindow.setCursorGrab("confined");
if (nativeWindow.monitors) {
  const screens: Monitor[] = nativeWindow.monitors();
  void screens;
}
if (dialog.openFile) {
  void dialog.openFile({ filters: [{ name: "Text", extensions: ["txt"] }] })
    .then((chosen: string | null) => chosen);
}

// The view element: typed as itself through the tag-name map, not as HTMLElement.
const view = document.createElement("blitsen-view");
view.addEventListener("resize", () => {});
const surface = view.acquireSurface();
const frame = new Uint8Array(surface.byteLength);
frame[0] = surface.width + surface.height + surface.devicePixelRatio + surface.generation;
surface.write(frame);
surface.release();

void defineConfig({
  output: "dist",
  name: "Demo",
  build: "vite build",
  window: { type: "borderless", resizable: false, transparent: true, alwaysOnTop: true },
  tray: {
    icon: "native/tray.png",
    openOnClick: true,
    closeToTray: true,
    contextMenu: [
      { label: "Open", action: "show" },
      { action: "separator" },
      { label: "Quit", action: "quit", enabled: true },
    ],
  },
});
export type { ClipboardImage };
