// What the published types must reject. Every line here is expected to be an
// error, and the runner fails if any of them compiles — types that accept
// everything are worth nothing, and this is the half that proves they do not.
//
// Each `@ts-expect-error` is its own assertion: TypeScript reports an unused one
// as an error of its own, so a line that quietly starts compiling still fails.
import app from "blitsen/app";
import nativeWindow from "blitsen/window";
import tray from "blitsen/tray";

// A capability is optional because the running version may not install it.
// Calling one without narrowing is the mistake these definitions exist to catch.
// @ts-expect-error
app.dataDir("demo");

// Another module's method is not on this one: one declaration file per subpath.
// @ts-expect-error
app.openFile();

// Declared absent in this version — `window.create` is a real absence with a
// written reason — so it is `unknown` and cannot be called.
// @ts-expect-error
nativeWindow.create();

// A module that installs nothing declares nothing.
// @ts-expect-error
tray.create();

// The signatures are real signatures.
// @ts-expect-error
if (nativeWindow.setSize) nativeWindow.setSize("640", "480");
// @ts-expect-error
if (nativeWindow.setCursorGrab) nativeWindow.setCursorGrab("sideways");

// `<blitsen-view>` is typed as itself, so its method exists...
const view = document.createElement("blitsen-view");
const surface = view.acquireSurface();
// ...and an ordinary element's does not.
// @ts-expect-error
document.createElement("div").acquireSurface();

// A surface takes pixels, not a number.
// @ts-expect-error
surface.write(surface.byteLength);

// The config is validated by its type before it is validated at run time.
import { defineConfig } from "blitsen";
// @ts-expect-error
defineConfig({ name: "Demo" });
// @ts-expect-error
defineConfig({ output: "dist", unknownKey: true });
// @ts-expect-error
defineConfig({ output: "dist", window: { type: "frameless" } });
// @ts-expect-error
defineConfig({ output: "dist", tray: { icon: "tray.png", contextMenu: [{ action: "launch" }] } });
