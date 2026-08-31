// APK entries must remain stored so assets can be memory-mapped and native
// libraries can be page-aligned for `extractNativeLibs=false`. The custom ZIP
// writer supplies that policy around aapt2's generated manifest and resources.
// Constant DOS timestamps make identical inputs reproducible byte for byte.

import { readdir, readFile } from "node:fs/promises";
import { join, posix } from "node:path";

/// 1980-01-01 00:00:00, which is the earliest instant MS-DOS date fields can
/// express: a year field of 0, month 1, day 1. Zip has no "unset".
const DOS_TIME = 0;
const DOS_DATE = (0 << 9) | (1 << 5) | 1;

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

/** CRC-32 as zip defines it, which is the same polynomial gzip and PNG use. */
export function crc32(bytes) {
  let value = 0xffffffff;
  for (let index = 0; index < bytes.length; index += 1) {
    value = CRC_TABLE[(value ^ bytes[index]) & 0xff] ^ (value >>> 8);
  }
  return (value ^ 0xffffffff) >>> 0;
}

/// What this writer refuses rather than mis-encodes. Zip64 is not implemented,
/// because an APK that needed it could not be installed anyway — Android's own
/// reader is 32-bit in these fields for the entries that matter — and silently
/// truncating a length into 32 bits produces an archive that opens and is wrong.
const ZIP_MAX = 0xffffffff;
const ZIP_MAX_ENTRIES = 0xffff;

/**
 * One zip archive, every entry stored, in the order given.
 *
 * Order is the caller's and is preserved, because it is the only control over
 * layout there is: `AndroidManifest.xml` first is what every APK does and what
 * a reader scanning for it finds fastest.
 */
export function storedZip(entries) {
  if (entries.length > ZIP_MAX_ENTRIES) {
    throw new Error(`an APK cannot hold ${entries.length} files: the archive format this writes `
      + `records at most ${ZIP_MAX_ENTRIES} entries`);
  }
  const local = [];
  const central = [];
  let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name, "utf8");
    const body = Buffer.isBuffer(entry.data) ? entry.data : Buffer.from(entry.data);
    if (body.length > ZIP_MAX || offset > ZIP_MAX) {
      throw new Error(`${entry.name} makes this APK larger than 4 GiB, which the archive format `
        + "this writes cannot address");
    }
    const sum = crc32(body);
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(0, 6);
    header.writeUInt16LE(0, 8);
    header.writeUInt16LE(DOS_TIME, 10);
    header.writeUInt16LE(DOS_DATE, 12);
    header.writeUInt32LE(sum, 14);
    header.writeUInt32LE(body.length, 18);
    header.writeUInt32LE(body.length, 22);
    header.writeUInt16LE(name.length, 26);
    local.push(header, name, body);
    const record = Buffer.alloc(46);
    record.writeUInt32LE(0x02014b50, 0);
    record.writeUInt16LE(20, 4);
    record.writeUInt16LE(20, 6);
    record.writeUInt16LE(0, 8);
    record.writeUInt16LE(0, 10);
    record.writeUInt16LE(DOS_TIME, 12);
    record.writeUInt16LE(DOS_DATE, 14);
    record.writeUInt32LE(sum, 16);
    record.writeUInt32LE(body.length, 20);
    record.writeUInt32LE(body.length, 24);
    record.writeUInt16LE(name.length, 28);
    record.writeUInt32LE(offset, 42);
    central.push(record, name);
    offset += 30 + name.length + body.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}

/// The configuration changes the activity absorbs instead of being destroyed
/// and recreated for.
///
/// This list is load-bearing and was measured, not copied. #143 ran a paired
/// control differing only in this attribute: with it, a dark-mode change moved
/// 3,333 pixels and the application kept painting; without it, the process
/// stayed alive holding its last frame and **never drew again** — no crash, no
/// tombstone, nothing in logcat. That is the failure mode this prevents, and it
/// is silent, which is why the list is here rather than left to a default.
export const CONFIG_CHANGES = ["orientation", "keyboardHidden", "keyboard", "screenSize",
  "screenLayout", "smallestScreenSize", "locale", "layoutDirection", "density", "uiMode",
  "fontScale", "navigation", "mcc", "mnc"].join("|");

const escapeXml = text => String(text)
  .replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
  .replaceAll("\"", "&quot;").replaceAll("'", "&apos;");

/**
 * The whole of the Java side of a Blitsen application.
 *
 * NativeActivity still owns the application lifecycle, but #252 needs two
 * a callback the platform class does not provide: a private receiver that can
 * persist notification activation before optionally launching NativeActivity.
 * The exported launcher never sees the trusted envelope, so another package
 * cannot forge one by explicitly starting that public component.
 *
 * `android:extractNativeLibs="false"` is the reason the archive is written the
 * way it is. It tells the installer to leave the `.so` inside the APK and map
 * it, which halves the installed footprint of a 35 MB library and is only legal
 * for an entry that is stored and page-aligned.
 *
 * The two capability declarations are not symmetrical, and the asymmetry is the
 * platform's. `POST_NOTIFICATIONS` is a permission, granted to the application
 * once. USB host is a *feature*: `blitsen/hid` needs no manifest permission at
 * all, because access is granted one device at a time, at run time, by a person
 * answering a system dialog (#248). It is declared `required="false"` so that a
 * device with no USB host controller still installs the application — every
 * other capability it has works there, and `hid.devices()` simply answers an
 * empty list, which is the same answer as "nothing is plugged in".
 */
export function androidManifest({
  applicationId,
  label,
  versionCode,
  versionName,
  library,
  minSdk,
  targetSdk,
  debuggable = false,
}) {
  // The comment says "for Android" rather than naming the flag, because a flag
  // begins with two hyphens and two hyphens cannot appear inside an XML
  // comment — aapt2 refuses the whole manifest with "not well-formed (invalid
  // token)" and a line number, which is measured rather than guessed.
  return `<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by \`blitsen build\` for Android. Edits are discarded on the next build. -->
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="${escapeXml(applicationId)}"
    android:versionCode="${versionCode}"
    android:versionName="${escapeXml(versionName)}">

    <uses-sdk android:minSdkVersion="${minSdk}" android:targetSdkVersion="${targetSdk}" />
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
    <uses-feature android:name="android.hardware.usb.host" android:required="false" />

    <application
        android:label="${escapeXml(label)}"
        android:hasCode="true"${debuggable ? "\n        android:debuggable=\"true\"" : ""}
        android:extractNativeLibs="false">
        <activity
            android:name="android.app.NativeActivity"
            android:exported="true"
            android:launchMode="singleTop"
            android:theme="@android:style/Theme.DeviceDefault.NoActionBar.Fullscreen"
            android:configChanges="${CONFIG_CHANGES}">
            <meta-data android:name="android.app.lib_name" android:value="${escapeXml(library)}" />
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
        <receiver
            android:name="com.blitsen.runtime.NotificationBridge$ActivationReceiver"
            android:exported="false" />
    </application>
</manifest>
`;
}

/// Every file under a directory, as archive-relative POSIX paths, sorted.
///
/// Sorted rather than in readdir order because readdir order is the
/// filesystem's and differs between machines, and a build whose output depends
/// on that is not reproducible.
async function archiveTree(directory, prefix) {
  const found = [];
  const walk = async (at, relative) => {
    const entries = await readdir(at, { withFileTypes: true });
    for (const entry of entries.sort((left, right) => (left.name < right.name ? -1 : 1))) {
      const child = relative === "" ? entry.name : posix.join(relative, entry.name);
      if (entry.isDirectory()) await walk(join(at, entry.name), child);
      else found.push({ name: posix.join(prefix, child), source: join(at, entry.name) });
    }
  };
  await walk(directory, "");
  return found.sort((left, right) => (left.name < right.name ? -1 : 1));
}

/**
 * Everything that goes into the APK, in the order it is written.
 *
 * `linked` is the directory `aapt2 link --output-to-dir` wrote, which holds the
 * binary manifest and `resources.arsc`; D8 separately supplies `classes.dex`.
 * This build compiles no resources, because there are none: no layouts, no strings and, until
 * `--icon` grows an Android answer, no drawables.
 */
export async function apkEntries({ linked, dex, libraries, assets }) {
  const entries = [];
  for (const name of ["AndroidManifest.xml", "resources.arsc"]) {
    const data = await readFile(join(linked, name)).catch(() => null);
    if (data === null) {
      throw new Error(`aapt2 produced no ${name} in ${linked}, so there is nothing to package`);
    }
    entries.push({ name, data });
  }
  const classes = await readFile(dex).catch(() => null);
  if (classes === null) {
    throw new Error(`d8 produced no classes.dex at ${dex}, so notification activation callbacks `
      + "would be absent from the APK");
  }
  entries.push({ name: "classes.dex", data: classes });
  for (const library of [...libraries].sort((left, right) =>
    (left.entry < right.entry ? -1 : 1))) {
    const data = await readFile(library.source).catch(() => null);
    if (data === null) {
      throw new Error(`the cross-compile left no ${library.source}, so the ${library.abi} `
        + "slice of this APK would be empty");
    }
    entries.push({ name: library.entry, data });
  }
  for (const file of await archiveTree(assets, "assets")) {
    entries.push({ name: file.name, data: await readFile(file.source) });
  }
  return entries;
}
