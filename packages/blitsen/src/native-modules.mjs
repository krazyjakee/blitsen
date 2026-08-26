// Which `native:` modules exist on which target, and why the missing ones are
// missing. Read by `doctor`, so an application that imports a capability the
// target it is being built for does not have hears about it at export time
// rather than at run time (#147).
//
// Two axes have to be kept apart, because they fail differently and are fixed
// differently.
//
// *Not implemented anywhere* is the manifest's axis. Every currently exported
// module now has at least one runtime member on some platform, and that fact is
// derived rather than declared here:
// `api-manifest.mjs` reads the bootstrap and a module it finds no members for
// has none. Nothing below repeats it.
//
// *Implemented, but not on this target* is this file's axis, and it cannot be
// derived the same way. The bootstrap is one script shared by every build, and
// what installs under it is decided by `cfg` in `crates/blitsen-host` — which
// the manifest generator cannot read, because a `cfg` is resolved by the
// compiler and not visible in the source it would have to parse. So the table
// below is *declared*, the same way `ENGINE_ABSENT` is declared, and the same
// obligation follows: every row is a decision recorded somewhere in the Rust it
// describes, and `REASONS` quotes the argument rather than restating that the
// module is missing.
//
// `docs/PRODUCT.md` §7 is the rule the rows implement — absent rather than
// approximated. A row here is therefore never a to-do list entry. It says the
// capability does not exist on that platform and names what would have to
// change for it to.

import { NATIVE_MODULES } from "./native/module.mjs";

/// The platforms a target's `native:` surface is decided by. A target is
/// `<platform>-<arch>`; the architecture never changes which modules exist.
export const NATIVE_PLATFORMS = ["linux", "darwin", "win32", "android"];

/// The Android targets `doctor` grades against, in this package's own
/// `<platform>-<arch>` vocabulary rather than the NDK's — `android-arm64` is the
/// ABI Android calls `arm64-v8a` and ships, `android-x64` is `x86_64` and is the
/// emulator one (PRODUCT.md P5c).
///
/// Not in `TARGETS`, and that is the point rather than an omission: `TARGETS` is
/// the six platform packages an install resolves, and Android is a cross-compiled
/// APK that is not one of them (#148). Grading names the ABI without resolving
/// a desktop runtime package; the Android build links the source checkout.
export const ANDROID_TARGETS = ["android-arm64", "android-x64"];

export const platformOf = target => String(target).split("-")[0];

// Absences that are not Android's.
//
// `app` survives everywhere, but not whole: the single-instance lock is a Unix
// domain socket that doubles as the channel a second invocation's `argv`
// arrives on, and Windows wants a named mutex plus a pipe, which is a different
// design rather than this one with the socket swapped out. That is a member
// rather than a module, so it is not in this table; `NATIVE_ABSENT` in
// `api-manifest.mjs` carries the member-level absences.
//
// `menu` is the other one that is not Android's. An application menu is the
// macOS main menu and the Windows window menu bar; Linux desktops have neither
// as something a winit window can own, and the tray menu next to it is a
// different object with a different owner rather than the same one relocated.
const ABSENT = {
  linux: ["menu"],
  darwin: [],
  win32: [],
  android: ["app", "clipboard", "dialog", "window", "tray", "menu"],
};

// Why, per platform, in the words of the module that made the call. Keyed
// `<platform>.<module>` so a module absent on two platforms for two different
// reasons says both.
const REASONS = {
  "android.app": "The directories are the Activity's `filesDir` and `cacheDir`, which only the "
    + "Activity can name; Android sets none of the XDG variables, so resolving them would answer "
    + "a path nothing can write to. `relaunch` has no executable to spawn inside an APK, and "
    + "single-instance ownership is the platform's own — a second launch is an Intent delivered "
    + "to the process already running, not a command line to hand over.",
  "android.clipboard": "`arboard` has no Android backend and does not compile there. The service "
    + "it would wrap, `ClipboardManager`, refuses a read outright unless the application holds "
    + "focus, and these readers report an empty clipboard as `null` — so a refusal and an empty "
    + "clipboard would be indistinguishable. It needs a module shaped for that, over JNI.",
  "android.dialog": "There is no XDG desktop portal on Android. The system's own choosers are "
    + "Intents answered by another activity, which is a different shape from a call that resolves.",
  "android.window": "winit accepts every setter on Android and discards it, then answers the "
    + "getter as though the request had never been made: `setDecorations(false)` is followed by "
    + "`isDecorated()` saying true, on a platform with no decorations. The monitor list goes too, "
    + "and it is the one worth naming because it looks like the survivor — winit enumerates no "
    + "monitors there, so `monitors()` would report a device with no display. Immersive mode and "
    + "orientation are the real capabilities here and are not these under another name.",
  "linux.menu": "A Linux menu bar is a widget inside the window, and the only backend the menu "
    + "crate has for one is a gtk::MenuBar packed into a gtk::Window — Blitsen windows are winit's, "
    + "and the renderer owns the whole client area, so there is nowhere to pack it and no GTK main "
    + "loop to run it. The desktop-level alternative is the D-Bus global menu, which only some "
    + "desktops implement, needs an X11 window id and so answers nothing on Wayland, and would "
    + "leave the same application with a menu on KDE and none on GNOME. The tray menu is not this "
    + "under another name: it belongs to a status item the application may never show. What would "
    + "change this is a menu bar Blitsen renders itself, which is a different feature — an "
    + "in-document menu is DOM, not a native one.",
  "android.menu": "Android has no application menu bar. Its equivalents are the app bar's overflow "
    + "menu and the navigation drawer, which are views inside the activity's own layout rather "
    + "than a menu the platform owns, and neither has this shape.",
  "android.tray": "Android has no desktop notification area or status-item menu. Its persistent "
    + "status UI is a notification, which belongs to blitsen/notify and carries its own runtime "
    + "permission and channel semantics rather than pretending to be a tray icon.",
};

/// The `native:` modules that do not exist on `target`, each with its reason.
///
/// An unknown platform reports nothing rather than guessing. `doctor` runs
/// against the host by default and the six shipping targets are all listed, so
/// the only way here is a target this table has not been taught, which must not
/// turn into a wave of findings the user cannot act on.
export function absentNativeModules(target) {
  const platform = platformOf(target);
  return (ABSENT[platform] ?? []).map(module => ({
    module,
    platform,
    reason: REASONS[`${platform}.${module}`],
  }));
}

/// Refuses a table that has drifted from the module list or from its own
/// reasons. Called by the test rather than at load: this is a source-integrity
/// check, and paying for it on every `doctor` run would be paying for it in the
/// one place it can no longer fail.
export function checkNativeModuleTable() {
  const problems = [];
  for (const [platform, modules] of Object.entries(ABSENT)) {
    if (!NATIVE_PLATFORMS.includes(platform)) problems.push(`${platform} is not a known platform`);
    for (const module of modules) {
      if (!NATIVE_MODULES.includes(module))
        problems.push(`${platform} calls ${module} absent, which is not a blitsen/ module`);
      if (!REASONS[`${platform}.${module}`])
        problems.push(`${platform}.${module} is absent and the table does not say why`);
    }
  }
  for (const key of Object.keys(REASONS)) {
    const [platform, module] = key.split(".");
    if (!ABSENT[platform]?.includes(module))
      problems.push(`${key} has a reason but is not listed absent`);
  }
  for (const platform of NATIVE_PLATFORMS)
    if (!(platform in ABSENT))
      problems.push(`${platform} has no row, so it cannot be told from a platform that has them all`);
  if (problems.length > 0)
    throw new Error(`the native module table is inconsistent:\n  ${problems.join("\n  ")}`);
  return Object.values(ABSENT).flat().length;
}
