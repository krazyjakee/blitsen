import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { copyFile, mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, resolve } from "node:path";

const ICON_FORMATS = { darwin: [".png", ".icns"], linux: [".png", ".svg"], win32: [".png", ".ico"] };
// The PNG-bearing ICNS types. Smaller entries exist but are raw-bitmap only, so a
// small source PNG is refused rather than written as an icon macOS cannot read.
const ICNS_TYPES = { 128: "ic07", 256: "ic08", 512: "ic09", 1024: "ic10" };
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

const XML_ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" };
const escapeXml = text => String(text).replace(/[&<>"']/g, character => XML_ESCAPES[character]);

function slug(text) {
  const cleaned = text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return cleaned || "app";
}

/** Stable identity used by packaging and application-owned persistent data. */
export const defaultApplicationIdentifier = title => `com.blitsen.${slug(title)}`;

export function pngDimensions(bytes, path) {
  if (bytes.length < 24 || PNG_SIGNATURE.some((byte, index) => bytes[index] !== byte)) {
    throw new Error(`icon is not a PNG file: ${path}`);
  }
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (width === 0 || height === 0) throw new Error(`icon has invalid PNG dimensions: ${path}`);
  return { width, height };
}

function pngSize(bytes, path) {
  const { width, height } = pngDimensions(bytes, path);
  if (width !== height) throw new Error(`icon must be square, got ${width}x${height}: ${path}`);
  return width;
}

// Vista and later accept a PNG payload inside an .ico directory entry, so no
// bitmap encoder is needed. 256 is stored as 0 in the single-byte size fields.
function icoFromPng(png, size, path) {
  if (size > 256) throw new Error(`Windows icons cannot exceed 256x256, got ${size}x${size}: ${path}`);
  const header = Buffer.alloc(22);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(1, 4);
  header[6] = size === 256 ? 0 : size;
  header[7] = size === 256 ? 0 : size;
  header.writeUInt16LE(1, 10);
  header.writeUInt16LE(32, 12);
  header.writeUInt32LE(png.length, 14);
  header.writeUInt32LE(header.length, 18);
  return Buffer.concat([header, png]);
}

function icnsFromPng(png, size, path) {
  const type = ICNS_TYPES[size];
  if (!type) {
    throw new Error(`macOS icons need a PNG of ${Object.keys(ICNS_TYPES).join(", ")} pixels `
      + `or a prebuilt .icns, got ${size}x${size}: ${path}`);
  }
  const entry = Buffer.alloc(8);
  entry.write(type, 0, "ascii");
  entry.writeUInt32BE(png.length + entry.length, 4);
  const header = Buffer.alloc(8);
  header.write("icns", 0, "ascii");
  header.writeUInt32BE(png.length + entry.length + header.length, 4);
  return Buffer.concat([header, entry, png]);
}

function iconFile(platform, icon, name) {
  const extension = extname(icon).toLowerCase();
  const accepted = ICON_FORMATS[platform];
  if (!accepted.includes(extension)) {
    throw new Error(`${platform} icons must be ${accepted.join(" or ")}, got ${extension || icon}`);
  }
  if (extension !== ".png" || platform === "linux") return `${name}${extension}`;
  return platform === "win32" ? `${name}.ico` : `${name}.icns`;
}

async function iconResource(platform, icon, name) {
  const path = resolve(icon);
  const file = iconFile(platform, path, name);
  const bytes = await readFile(path).catch(() => {
    throw new Error(`icon file does not exist: ${icon}`);
  });
  if (extname(file).toLowerCase() === extname(path).toLowerCase()) return { file, bytes };
  const size = pngSize(bytes, path);
  return {
    file,
    bytes: platform === "win32" ? icoFromPng(bytes, size, path) : icnsFromPng(bytes, size, path),
  };
}

function infoPlist({ name, executable, identifier, icon, version }) {
  const entries = [
    ["CFBundleName", name],
    ["CFBundleDisplayName", name],
    ["CFBundleExecutable", executable],
    ["CFBundleIdentifier", identifier],
    ["CFBundleInfoDictionaryVersion", "6.0"],
    ["CFBundlePackageType", "APPL"],
    ...icon ? [["CFBundleIconFile", icon]] : [],
    ...version ? [["CFBundleShortVersionString", version], ["CFBundleVersion", version]] : [],
  ];
  const body = entries
    .map(([key, value]) => `  <key>${key}</key>\n  <string>${escapeXml(value)}</string>`)
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${body}
  <key>NSUserNotificationAlertStyle</key>
  <string>alert</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
`;
}

// Desktop Entry Specification: reserved characters oblige a quoted Exec, and
// inside the quotes " $ ` and \\ are backslash-escaped.
function desktopExec(path) {
  return /[\s"'\\`$<>~|&;*?#()]/.test(path)
    ? `"${path.replace(/(["`$\\])/g, "\\$1")}"`
    : path;
}

const DBUS_NAME = /^[A-Za-z_][A-Za-z0-9_-]*(\.[A-Za-z_][A-Za-z0-9_-]*)+$/;

function linuxIdentity(identifier) {
  if (identifier && !DBUS_NAME.test(identifier)) {
    throw new Error(`--bundle-id ${JSON.stringify(identifier)} is not a Linux D-Bus application `
      + "name: use at least two dot-separated components, each beginning with a letter or _");
  }
  return identifier;
}

const WINDOWS_IDENTITY = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

function windowsIdentity(identifier) {
  if (identifier && !WINDOWS_IDENTITY.test(identifier)) {
    throw new Error(`--bundle-id ${JSON.stringify(identifier)} is not a Windows AppUserModelID: `
      + "use at most 128 letters, numbers, dots, underscores or hyphens");
  }
  return identifier;
}

/** Stable CLSID shared with the Windows runtime's COM class registration. */
export function notificationActivatorClsid(identifier) {
  const bytes = createHash("sha256")
    .update(`blitsen-notification-activator:${identifier}`, "utf8").digest().subarray(0, 16);
  bytes[6] = (bytes[6] & 0x0f) | 0x50;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString("hex");
  return `{${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-`
    + `${hex.slice(16, 20)}-${hex.slice(20)}}`;
}

const escapePowerShellString = value => String(value).replaceAll("'", "''");

function windowsNotificationRegistration({ identifier, executable, title }) {
  const clsid = notificationActivatorClsid(identifier);
  return `$ErrorActionPreference = 'Stop'
$executable = Join-Path $PSScriptRoot '${escapePowerShellString(basename(executable))}'
$appId = 'HKCU:\\Software\\Classes\\AppUserModelId\\${identifier}'
$server = 'HKCU:\\Software\\Classes\\CLSID\\${clsid}\\LocalServer32'
New-Item -Path $appId -Force | Out-Null
New-ItemProperty -Path $appId -Name 'DisplayName' -Value '${escapePowerShellString(title)}' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $appId -Name 'CustomActivator' -Value '${clsid}' -PropertyType String -Force | Out-Null
New-Item -Path $server -Force | Out-Null
Set-Item -Path $server -Value ('"' + $executable + '" --notification-com-server')
`;
}

function desktopEntry({ name, executable, icon, identity }) {
  return [
    "[Desktop Entry]",
    "Type=Application",
    "Version=1.0",
    `Name=${name.replace(/\n/g, " ")}`,
    // `%u` preserves the existing one-URI fallback for desktops that ignore
    // DBusActivatable and for protocol registration. Notification actions on a
    // D-Bus-aware desktop use ActivateAction rather than this field (#252).
    `Exec=${desktopExec(executable)} %u`,
    ...identity ? ["DBusActivatable=true"] : [],
    ...icon ? [`Icon=${icon}`] : [],
    "Terminal=false",
    // The entry a notification's `desktop-entry` hint names is this file, and
    // the hint is the only way the notification service can tell which installed
    // application a notification belongs to. Declaring it here is what puts the
    // application in GNOME's notification settings rather than leaving the user
    // with a switch they cannot find.
    "X-GNOME-UsesNotifications=true",
    "",
  ].join("\n");
}

function dbusService({ identity, executable }) {
  return [
    "[D-BUS Service]",
    `Name=${identity}`,
    `Exec=${desktopExec(executable)}`,
    "",
  ].join("\n");
}

/**
 * The identity a notification activation for this artifact is addressed to (#252).
 *
 * `null` without an explicit `--bundle-id`, and deliberately not the
 * `com.blitsen.<title>` an `.app` falls back to: notification permission is
 * granted per identity, and an identity nobody chose is one two unrelated
 * applications could end up sharing. So an activation identity is opt-in the way
 * every other platform identity in this file is.
 *
 * `entry` is what the platform's own notification service knows the entry point
 * by. Linux deliberately uses the identity too: the desktop filename, D-Bus
 * service name and well-known bus name must all agree for activation.
 */
export function activationEntryPoint({ platform, identifier }) {
  if (!identifier) return null;
  if (platform === "linux") linuxIdentity(identifier);
  if (platform === "win32") windowsIdentity(identifier);
  return {
    identity: identifier,
    entry: identifier,
  };
}

function assemblyVersion(version) {
  const parts = (version ?? "").split(/[.+-]/).filter(part => /^\d+$/.test(part)).slice(0, 4);
  while (parts.length < 4) parts.push("0");
  return parts.join(".");
}

function windowsManifest({ name, version }) {
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="${escapeXml(slug(name))}" version="${assemblyVersion(version)}"/>
  <description>${escapeXml(name)}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">permonitorv2,permonitor</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
    </application>
  </compatibility>
</assembly>
`;
}

// Raw HID access is packaging rather than code (#247, S10). Neither a udev rule
// nor a macOS entitlement can be granted by a process that is already running,
// and a runtime that tried would be editing the system on the user's behalf. So
// what Blitsen does is write the artifact a distributor installs or signs with,
// and say what to do with it — `doctor` reports the same sentences before the
// build, which is the point at which it is still cheap to act on them.
//
// Android is the one entry with no artifact behind it (#248), and that is the
// fact worth reporting rather than an omission: its answer is a runtime dialog
// per device, so there is nothing for a distributor to install and nothing for
// a build to write. The sentence exists because a developer who has shipped the
// Linux rule and the macOS entitlement will otherwise go looking for the third
// one.
export const HID_ACCESS = {
  linux: "Access to a hidraw node is granted by a udev rule, not by the application: "
    + "`blitsen build` writes a `<name>.hid.rules` template beside the executable for the "
    + "distribution or installer to complete and install in /etc/udev/rules.d. Running the "
    + "application as root is not a substitute and Blitsen will not do it.",
  darwin: "A sandboxed macOS application needs the `com.apple.security.device.usb` entitlement: "
    + "`blitsen build` writes `<name>.app.entitlements` beside the bundle, and the --sign command "
    + "must pass it to `codesign --entitlements` for the capability to reach the signature.",
  win32: "Windows opens HID top-level collections through its own HID class driver and reserves "
    + "some system collections, which no packaging step unlocks and no driver replacement should "
    + "try to. An open refused that way rejects with NotAllowedError rather than NotFoundError.",
  android: "Android grants USB access one device at a time and at run time: the first open() of a "
    + "device raises a system dialog, the grant lasts only until that device is unplugged, and a "
    + "dismissed dialog rejects with NotAllowedError and can be asked again. There is no manifest "
    + "permission to add — `blitsen build` declares the android.hardware.usb.host feature as "
    + "optional — so nothing here is a packaging step, and a device attached over OTG has to be "
    + "attached for the application to see it at all.",
};

/// The rule a Linux distributor completes and installs, as a file.
function hidUdevRule(name) {
  return [
    `# udev rule for ${name}, which uses blitsen/hid.`,
    "#",
    "# Replace the vendor and product IDs below with the devices this application",
    "# opens — one line each — then install this file as",
    `# /etc/udev/rules.d/70-${slug(name)}.rules and run:`,
    "#",
    "#   udevadm control --reload && udevadm trigger",
    "#",
    "# TAG+=\"uaccess\" gives the user logged in at the active seat access to the",
    "# node. That is what a desktop application needs: it does not make the device",
    "# world-readable and it does not require the application to run as root.",
    'SUBSYSTEM=="hidraw", ATTRS{idVendor}=="ffff", ATTRS{idProduct}=="ffff", TAG+="uaccess"',
    "",
  ].join("\n");
}

/// The entitlements a sandboxed macOS build must be signed with.
function hidEntitlements() {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.device.usb</key>
  <true/>
</dict>
</plist>
`;
}

// The paths step ⑤ will write, so a collision is reported the way the linker
// reports one rather than silently replacing an existing bundle.
function packagePlan({ platform, executable, icon, identifier = null, hid = false }) {
  const supported = Object.keys(ICON_FORMATS);
  if (!supported.includes(platform)) {
    throw new Error(`packaging is not supported on ${platform} (expected ${supported.join(", ")})`);
  }
  const directory = dirname(executable);
  const name = basename(executable, platform === "win32" ? ".exe" : "");
  if (platform === "linux") linuxIdentity(identifier);
  if (platform === "win32") windowsIdentity(identifier);
  const resource = icon ? iconFile(platform, icon, name) : null;
  if (platform === "darwin") {
    const bundle = join(directory, `${name}.app`);
    return {
      platform,
      name,
      bundle,
      executable: join(bundle, "Contents", "MacOS", name),
      // Beside the bundle rather than inside it: entitlements are an input to
      // codesign, not a file the signed application carries.
      artifacts: [bundle, ...hid ? [join(directory, `${name}.app.entitlements`)] : []],
    };
  }
  const artifacts = platform === "win32"
    ? [
      `${executable}.manifest`,
      ...identifier ? [`${executable}.notification-register.ps1`] : [],
      ...resource ? [join(directory, resource)] : [],
    ]
    : [
      join(directory, `${identifier ?? name}.desktop`),
      ...identifier ? [join(directory, `${identifier}.service`)] : [],
      ...resource ? [join(directory, resource)] : [],
      ...hid ? [join(directory, `${name}.hid.rules`)] : [],
    ];
  return { platform, name, bundle: null, executable, artifacts };
}

// Step ⑤ Package. Windows resources are not embedded in the PE image: the icon
// and the application manifest ship beside the executable, which Windows reads
// as an external manifest, and the caller is told so.
export async function packageBuild({
  platform, executable, title, icon = null, identifier = null, version = null,
  assetDirectory = null, force = false, hid = false,
}) {
  const plan = packagePlan({ platform, executable, icon, identifier, hid });
  const resource = icon ? await iconResource(platform, icon, plan.name) : null;
  for (const artifact of plan.artifacts) {
    if (!await stat(artifact).catch(() => null)) continue;
    if (!force) throw new Error(`output already exists: ${artifact} (pass --force to replace it)`);
    await rm(artifact, { recursive: true, force: true });
  }
  const notes = [];
  const written = [];
  let assets = assetDirectory;

  if (platform === "darwin") {
    const contents = join(plan.bundle, "Contents");
    await mkdir(join(contents, "MacOS"), { recursive: true });
    await mkdir(join(contents, "Resources"), { recursive: true });
    await rename(executable, plan.executable);
    if (assets) {
      assets = join(contents, "MacOS", basename(assets));
      await rename(assetDirectory, assets);
    }
    await writeFile(join(contents, "Info.plist"), infoPlist({
      name: title,
      executable: plan.name,
      identifier: identifier ?? defaultApplicationIdentifier(title),
      icon: resource?.file ?? null,
      version,
    }));
    // Classic type/creator codes; the Finder still expects the file to exist.
    await writeFile(join(contents, "PkgInfo"), "APPL????");
    if (resource) await writeFile(join(contents, "Resources", resource.file), resource.bytes);
    written.push(plan.bundle);
    if (hid) {
      const entitlements = join(dirname(plan.bundle), `${plan.name}.app.entitlements`);
      await writeFile(entitlements, hidEntitlements());
      written.push(entitlements);
      notes.push(HID_ACCESS.darwin);
    }
  } else if (platform === "win32") {
    await writeFile(`${executable}.manifest`, windowsManifest({ name: title, version }));
    written.push(`${executable}.manifest`);
    if (identifier) {
      const registration = `${executable}.notification-register.ps1`;
      await writeFile(registration,
        windowsNotificationRegistration({ identifier, executable, title }));
      written.push(registration);
      notes.push("Run the notification registration PowerShell script after placing the executable "
        + "at its final installed path; the application refreshes the same per-user COM registration when run.");
    }
    if (resource) {
      const target = join(dirname(executable), resource.file);
      await writeFile(target, resource.bytes);
      written.push(target);
    }
    if (hid) notes.push(HID_ACCESS.win32);
    notes.push("Windows icon and version-info resources are not embedded in the executable; "
      + `the application ${resource ? "manifest and icon ship" : "manifest ships"} beside it.`);
  } else {
    let iconPath = null;
    if (resource) {
      iconPath = join(dirname(executable), resource.file);
      await writeFile(iconPath, resource.bytes);
    }
    const identity = linuxIdentity(identifier);
    const entry = join(dirname(executable), `${identity ?? plan.name}.desktop`);
    await writeFile(entry, desktopEntry({ name: title, executable, icon: iconPath, identity }));
    written.push(entry);
    if (identity) {
      const service = join(dirname(executable), `${identity}.service`);
      await writeFile(service, dbusService({ identity, executable }));
      written.push(service);
    }
    written.push(...iconPath ? [iconPath] : []);
    if (hid) {
      const rules = join(dirname(executable), `${plan.name}.hid.rules`);
      await writeFile(rules, hidUdevRule(title));
      written.push(rules);
      notes.push(HID_ACCESS.linux);
    }
  }

  return {
    platform,
    bundle: plan.bundle,
    executable: plan.executable,
    assetDirectory: assets,
    artifacts: written,
    notes,
  };
}

// Signing itself stays outside Blitsen: this is the hook, invoked with the
// artifact as its single positional argument so codesign or signtool can be
// wired without Blitsen managing certificates.
//
// The interpreter is *this machine's*, not the target's, because the hook runs
// here: packaging a Linux artifact from Windows spawned `sh`, which Windows
// does not have, and reported the missing shell as a signing failure with exit
// code 127 (#134). It is the same reason a cross-target build cannot sign at
// all — the signing tool has to exist on the host, and only the host's shell
// can start it.
export function signArgv(command, artifact) {
  return process.platform === "win32"
    ? ["cmd", "/c", `${command} "${artifact}"`]
    : ["sh", "-c", `${command} "$@"`, "sh", artifact];
}

export async function signArtifact({ command, artifact }) {
  const argv = signArgv(command, artifact);
  const child = spawn(argv[0], argv.slice(1), { stdio: "inherit" });
  const status = await new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("exit", code => resolve(code ?? 1));
  });
  if (status !== 0) {
    throw new Error(`signing command failed with exit code ${status}: ${command}`);
  }
  return { command, artifact };
}

// The development host's own macOS identity (#253).
//
// `UNUserNotificationCenter` will not talk to a process without a bundle
// identifier, and `blitsen run` is an interpreter executing a script. Rather
// than submit under an identifier some installed application owns, the
// development host is given one under Blitsen's own reverse-DNS namespace —
// and deliberately not the `com.blitsen.<title>` an export defaults to, because
// notification permission is granted per identifier: a developer allowing their
// dev host must not thereby have allowed the application they ship, and
// revoking one must not revoke the other.
export const developmentIdentifier = name => `com.blitsen.dev.${slug(name)}`;

// Ad hoc (`-`), because a development identity is not a distribution one:
// UNUserNotificationCenter asks for a signature rather than for a certificate,
// and `codesign` is part of macOS, so this needs nothing installed and nothing
// enrolled. `--force` replaces the signature the copied interpreter carries in.
// It is a default rather than a rule: `--sign` overrides it, which is the
// answer for a host binary whose entitlements an ad-hoc re-sign would drop.
export const DEVELOPMENT_SIGNATURE = "codesign --force --sign -";

/**
 * Builds and signs the `.app` a development run re-executes into.
 *
 * The same `Info.plist` an export gets, around a *copy* of the interpreter this
 * process is running. A copy rather than a link because signing rewrites the
 * file it covers, and a link would rewrite the interpreter the rest of the
 * machine runs; `COPYFILE_FICLONE` makes it a copy-on-write clone where the
 * filesystem has them, which is where the cache directory usually is.
 *
 * The bundle is reused until the interpreter, the identity or the signing
 * command changes — a stamp beside it rather than inside it, because anything
 * added under `Contents` after signing invalidates the signature it is meant to
 * describe. It is written last, so a bundle whose signing failed is rebuilt
 * rather than handed to the next run as an identity macOS will refuse.
 */
export async function developmentBundle({
  directory, name, identifier, launcher, version = null, sign = DEVELOPMENT_SIGNATURE,
}) {
  // The application names itself; the cache has to name a file, whatever it
  // chose. `Info.plist` still carries the name as written.
  const file = slug(name);
  const bundle = join(directory, `${file}.app`);
  const executable = join(bundle, "Contents", "MacOS", file);
  const stamp = `${bundle}.launcher`;
  const source = await stat(launcher).catch(() => {
    throw new Error(`the interpreter to bundle does not exist: ${launcher}`);
  });
  const built = `${identifier}\n${launcher}\n${source.size} ${source.mtimeMs}\n${sign}\n`;
  if (await readFile(stamp, "utf8").catch(() => null) === built) {
    return { bundle, executable, identifier, rebuilt: false };
  }

  await rm(stamp, { force: true });
  await rm(bundle, { recursive: true, force: true });
  await mkdir(dirname(executable), { recursive: true });
  await copyFile(launcher, executable, constants.COPYFILE_FICLONE);
  await writeFile(join(bundle, "Contents", "Info.plist"),
    infoPlist({ name, executable: file, identifier, icon: null, version }));
  await writeFile(join(bundle, "Contents", "PkgInfo"), "APPL????");
  await signArtifact({ command: sign, artifact: bundle });
  await writeFile(stamp, built);
  return { bundle, executable, identifier, rebuilt: true };
}
