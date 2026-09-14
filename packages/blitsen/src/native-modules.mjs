// Which `blitsen/*` modules exist on which target, and why the missing ones are
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

/// The platforms a target's native surface is decided by. A target is
/// `<platform>-<arch>`; the architecture never changes which modules exist.
export const NATIVE_PLATFORMS = ["linux", "darwin", "win32"];

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
};

// Why, per platform, in the words of the module that made the call. Keyed
// `<platform>.<module>` so a module absent on two platforms for two different
// reasons says both.
const REASONS = {
  "linux.menu": "A Linux menu bar is a widget inside the window, and the only backend the menu "
    + "crate has for one is a gtk::MenuBar packed into a gtk::Window — Blitsen windows are winit's, "
    + "and the renderer owns the whole client area, so there is nowhere to pack it and no GTK main "
    + "loop to run it. The desktop-level alternative is the D-Bus global menu, which only some "
    + "desktops implement, needs an X11 window id and so answers nothing on Wayland, and would "
    + "leave the same application with a menu on KDE and none on GNOME. The tray menu is not this "
    + "under another name: it belongs to a status item the application may never show. What would "
    + "change this is a menu bar Blitsen renders itself, which is a different feature — an "
    + "in-document menu is DOM, not a native one.",
};

/// The `blitsen/*` modules that do not exist on `target`, each with its reason.
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
