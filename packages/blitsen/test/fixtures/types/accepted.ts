// What the published types must accept: the documented way to use every
// `blitsen/*` subpath, plus `<blitsen-view>`.
import { defineConfig } from "blitsen";
import app from "blitsen/app";
import clipboard from "blitsen/clipboard";
import dialog from "blitsen/dialog";
import hid from "blitsen/hid";
import input from "blitsen/input";
import menu from "blitsen/menu";
import notify from "blitsen/notify";
import tray from "blitsen/tray";
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
if (nativeWindow.setMaximized && nativeWindow.isMaximized) {
  nativeWindow.setMaximized(!nativeWindow.isMaximized());
}
if (nativeWindow.setMinimized) nativeWindow.setMinimized(true);
if (nativeWindow.startDrag) nativeWindow.startDrag();
if (nativeWindow.close) nativeWindow.close();
if (nativeWindow.setCursorGrab) nativeWindow.setCursorGrab("confined");
if (nativeWindow.monitors) {
  const screens: Monitor[] = nativeWindow.monitors();
  void screens;
}
if (dialog.openFile) {
  void dialog.openFile({ filters: [{ name: "Text", extensions: ["txt"] }] })
    .then((chosen: string | null) => chosen);
}
if (tray.configure) {
  void tray.configure({
    icon: new Uint8Array(16),
    menu: [
      { id: "open", label: "Open", accelerator: "CmdOrCtrl+KeyO" },
      { type: "checkbox", id: "launch", label: "Launch at login", checked: true },
      {
        type: "submenu",
        label: "Theme",
        menu: [
          { type: "radio", id: "light", label: "Light", group: "theme", checked: true },
          { type: "radio", id: "dark", label: "Dark", group: "theme" },
        ],
      },
      { type: "separator" },
      { action: "quit" },
    ],
  });
}
if (tray.onAction) {
  const unsubscribe: () => void = tray.onAction(event => {
    void event.id;
    void event.checked;
  });
  unsubscribe();
}
// The application menu needs no tray: nothing above is configured to use one.
if (menu.configure) {
  void menu.configure({
    menu: [
      { type: "submenu", role: "application", label: "Demo", menu: [
        { type: "role", role: "about" },
        { type: "separator" },
        { type: "role", role: "quit" },
      ] },
      { type: "submenu", label: "File", menu: [
        { id: "new", label: "New", accelerator: "CmdOrCtrl+KeyN" },
        { type: "checkbox", id: "autosave", label: "Autosave", checked: true },
        { type: "submenu", label: "Theme", menu: [
          { type: "radio", id: "menu-light", label: "Light", group: "theme", checked: true },
          { type: "radio", id: "menu-dark", label: "Dark", group: "theme" },
        ] },
      ] },
    ],
  });
}
if (menu.onAction) {
  const unsubscribe: () => void = menu.onAction(event => {
    void event.id;
    void event.checked;
  });
  unsubscribe();
}
if (menu.remove) void menu.remove();
if (input.snapshot) {
  const state = input.snapshot();
  void state.sequence;
  void state.keys[0]?.code;
  void state.pointer.movementX;
}
if (input.onDeviceChange) {
  const unsubscribe = input.onDeviceChange(event => {
    const slot: number = event.index;
    const identity: string = event.id;
    const kind: "connected" | "disconnected" = event.type;
    void [slot, identity, kind];
  });
  unsubscribe();
}
if (input.vibrateGamepad) {
  void input.vibrateGamepad(0, {
    duration: 100,
    strongMagnitude: 0.75,
    weakMagnitude: 0.25,
  });
}
if (hid.devices) {
  void hid.devices().then(async devices => {
    const found = devices[0];
    if (!found || !hid.open) return;
    void found.usages[0]?.usagePage;
    void found.serialNumber;
    const device = await hid.open(found.id);
    const stop: () => void = device.onInputReport(report => {
      void report.reportId;
      void report.data.byteLength;
    });
    await device.write(new Uint8Array([0x00, 0x01]));
    await device.sendFeatureReport(new Uint8Array([0x03, 0x01]));
    const feature: Uint8Array = await device.receiveFeatureReport(3);
    void feature;
    void device.maxOutputReportSize;
    device.onDisconnect(event => { void event.deviceId; });
    stop();
    await device.close();
  });
}
if (hid.onDeviceChange) {
  const unsubscribe: () => void = hid.onDeviceChange(event => {
    if (event.type === "connected") void event.device.productName;
  });
  unsubscribe();
}
if (notify.show) void notify.show({ title: "Complete", urgency: "normal" });
if (notify.permission) void notify.permission();
if (notify.requestPermission) void notify.requestPermission();
if (notify.update) void notify.update("n1", { body: "Still working" });
if (notify.close) void notify.close("n1");
if (notify.onEvent) {
  const unsubscribe: () => void = notify.onEvent(event => {
    void event.id;
    if (event.type === "action") void event.action;
    if (event.type === "close") void event.reason;
    if (event.type === "error") void event.message;
  });
  unsubscribe();
}
if ("Notification" in globalThis) {
  void Notification.permission;
  void Notification.requestPermission();
  const standardNotification = new Notification("Complete", {
    body: "The export is ready",
    requireInteraction: true,
  });
  standardNotification.onclick = () => {};
  standardNotification.close();
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
      { label: "Open", action: "show", icon: "native/open.png", accelerator: "CmdOrCtrl+KeyO" },
      { action: "separator" },
      { type: "checkbox", id: "launch", label: "Launch at login", checked: true },
      { type: "submenu", label: "Theme", menu: [
        { type: "radio", id: "light", label: "Light", group: "theme", checked: true },
        { type: "radio", id: "dark", label: "Dark", group: "theme" },
      ] },
      { label: "Quit", action: "quit", enabled: true },
    ],
  },
  menu: {
    menu: [
      { type: "submenu", role: "application", label: "Demo", menu: [
        { type: "role", role: "about" },
        { type: "separator" },
        { type: "role", role: "quit" },
      ] },
      { type: "submenu", label: "File", menu: [
        { id: "new", label: "New", accelerator: "CmdOrCtrl+KeyN" },
      ] },
      { type: "submenu", role: "help", label: "Help", menu: [
        { id: "docs", label: "Documentation" },
      ] },
    ],
  },
});
export type { ClipboardImage };
