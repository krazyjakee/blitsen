// What the published types must reject. Every line here is expected to be an
// error, and the runner fails if any of them compiles — types that accept
// everything are worth nothing, and this is the half that proves they do not.
//
// Each `@ts-expect-error` is its own assertion: TypeScript reports an unused one
// as an error of its own, so a line that quietly starts compiling still fails.
import app from "blitsen/app";
import nativeWindow from "blitsen/window";
import notify from "blitsen/notify";
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

// An undeclared member remains unavailable even after the module gains a surface.
// @ts-expect-error
tray.create();

// An application-defined item needs the label the user will see.
// @ts-expect-error
if (tray.configure) tray.configure({ icon: new Uint8Array(), menu: [{ id: "open" }] });

// A radio item needs a group and a submenu needs children.
// @ts-expect-error
if (tray.configure) tray.configure({ icon: new Uint8Array(), menu: [{ type: "radio", id: "x", label: "X" }] });
// @ts-expect-error
if (tray.configure) tray.configure({ icon: new Uint8Array(), menu: [{ type: "submenu", label: "More" }] });

// The signatures are real signatures.
// @ts-expect-error
if (nativeWindow.setSize) nativeWindow.setSize("640", "480");
// @ts-expect-error
if (nativeWindow.setCursorGrab) nativeWindow.setCursorGrab("sideways");
// @ts-expect-error
if (notify.show) notify.show({ title: "Bad", urgency: "urgent" });
// @ts-expect-error
if (notify.update) notify.update("n1", { timeout: "soon" });
// @ts-expect-error
if (notify.onEvent) notify.onEvent("click");

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
// @ts-expect-error
defineConfig({ output: "dist", tray: { icon: "tray.png", contextMenu: [{ id: "both", action: "show", label: "Both" }] } });
// @ts-expect-error
defineConfig({ output: "dist", tray: { icon: "tray.png", contextMenu: [{ type: "radio", id: "theme", label: "Theme" }] } });
