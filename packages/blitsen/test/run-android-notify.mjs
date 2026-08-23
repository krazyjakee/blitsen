// The Android notification test: install an APK, answer the permission dialog, and
// read the shade back (issue #254).
//
//     bun run --cwd packages/blitsen test:android-notify -- \
//       --apk <path> --package com.blitsen.notifyfixture
//
// # What this measures
//
// #245 implemented Android notifications through `android-activity` and `jni`, and
// the checkout it was written in had no emulator, so nothing in it had ever posted
// a notification. Source-level type checking proves the JNI signatures compile
// against `android.jar`; it cannot prove that `checkSelfPermission` returns what the
// host believes, that `blitsen.default` is the channel the shade actually files the
// notification under, or that `NotificationManager.notify` with a reused ID replaces
// rather than adds. Those are the questions here, and every one of them is answered
// by asking the platform rather than by asking the application.
//
// # The two sides, and why both are needed
//
// **The platform side** is `adb shell dumpsys notification --noredact`, which lists
// every posted record with its extras. That is where "the update replaced it", "the
// close removed one and not the other" and "the timeout expired" are read, because
// those are facts about the system's state and the application's opinion of them is
// worth nothing.
//
// **The application side** is the transcript in `android-notify-app.mjs`, which is
// what `requestPermission()` settled to, what `update` of an unknown ID returned,
// and which failures arrived as promise rejections. dumpsys cannot see any of that:
// a promise that never settles and a promise that settled to the right value leave
// the shade in the same state.
//
// The transcript travels as notification titles, because the shade is the only
// channel out of an APK — `android-notify-app.mjs` records why, and why that means
// the denial scenarios have to be granted the permission afterwards to let their
// transcript out.
//
// # The control
//
// A dumpsys parser that finds nothing looks exactly like an application that posted
// nothing, and this repository has a history of green checks that measured nothing.
// So before anything of Blitsen's is installed, the platform's own
// `cmd notification post` posts a known notification and this reads it back. If that
// fails, the run says the harness or the Android build is what to look at, and stops
// — it does not go on to blame Blitsen for an empty shade. It is the same control
// the framebuffer smoke test takes before it installs anything.
import { join } from "node:path";

import {
  adb, adbOrFail, argument, artifacts, deviceIsAlive, keep as keepIn, sleep, tap, uiHierarchy,
  uiNodes, waitForBoot,
} from "./android-device.mjs";
import { EXPIRY, PREFIX, TITLES } from "./android-notify-app.mjs";

/// The runtime permission the whole API 33+ half turns on. Spelled here rather than
/// imported from the packager, which writes it into the manifest as a string too;
/// `android-notify.test.mjs` holds the two to each other.
export const PERMISSION = "android.permission.POST_NOTIFICATIONS";

/// The API level at which posting notifications became a runtime permission. Below
/// it the host reports `granted` without asking the platform anything.
export const RUNTIME_PERMISSION_SDK = 33;

/// The one channel #245 creates, spelled the same in `notify/android.rs`.
export const CHANNEL = "blitsen.default";

const options = {
  apk: argument("apk"),
  package: argument("package"),
  activity: argument("activity", "android.app.NativeActivity"),
  out: argument("out", join(process.cwd(), "../../target/android-notify")),
  /// Which scenarios, when the device's API level is not to be trusted to choose.
  /// `persisted` reads what `deny` wrote, so the order is not free.
  scenarios: argument("scenarios", null),
  /// How long the device gets to boot.
  settle: Number(argument("settle", "45000")),
  /// How long the permission dialog gets to appear. Much longer than a boot,
  /// because it is only asked for once the engine has started and a first frame on
  /// a software-rendered emulator is tens of seconds of that on its own.
  dialogWait: Number(argument("dialog-wait", "120000")),
  /// How long one scenario gets to produce its whole transcript. Generous: a
  /// software-rendered emulator on a hosted runner needs tens of seconds to reach a
  /// first frame, and the fixture then spends eight of its own waiting out a
  /// notification timeout it has to be seen to outlive.
  deadline: Number(argument("deadline", "180000")),
  /// How long after the activity is displayed the permission is granted from adb in
  /// the scenarios that have no dialog to wait on. The fixture reads the permission
  /// three times before it waits for a grant, so granting early makes those reads
  /// wrong — which fails loudly rather than quietly, but is still a false red.
  grantDelay: Number(argument("grant-delay", "10000")),
};

const keep = (name, contents) => keepIn(options.out, name, contents);

/// Which scenarios an API level can answer.
///
/// Below 33 there is exactly one, and it is not a degenerate version of the others:
/// the interesting claim is that delivery works with *no* runtime permission
/// involved at all, which is a different code path in the host rather than the same
/// one pre-answered.
export function scenarioPlan(sdk) {
  return sdk < RUNTIME_PERMISSION_SDK ? ["implicit"] : ["deny", "persisted", "grant"];
}

/// What the fixture must have observed, per scenario.
///
/// `p0` is the read before anything is asked, `r0` what `requestPermission()`
/// settled to, `p1` the read after it. The one that is easy to miss is `persisted`:
/// the permission is not held and the platform cannot say whether it was ever
/// requested, so `default` and `denied` are only distinguishable because #245
/// records the request in SharedPreferences. Reading `denied` there, in a process
/// that never saw the dialog, is the whole of that claim.
export function expected(sdk, scenario) {
  if (sdk < RUNTIME_PERMISSION_SDK) {
    return { p0: "granted", r0: "granted", p1: "granted", dialog: false, answer: null };
  }
  switch (scenario) {
    case "grant":
      return { p0: "default", r0: "granted", p1: "granted", dialog: true, answer: "allow" };
    case "deny":
      return { p0: "default", r0: "denied", p1: "denied", dialog: true, answer: "deny" };
    case "persisted":
      return { p0: "denied", r0: "denied", p1: "denied", dialog: false, answer: null };
    default:
      throw new Error(`${scenario} is not a scenario this knows`);
  }
}

/// Every notification record in a `dumpsys notification --noredact` dump.
///
/// Field-by-field rather than whole-block regexes, and indentation-scoped rather
/// than run to the next header, because the sections after the record list are not
/// records and must not be read as the last one's extras.
///
/// Exported and unit-tested for the reason the smoke test's frame decoder is: if
/// this silently found nothing, "the application posted nothing" and "this cannot
/// read what it posted" would be the same result, and only one of them is a bug in
/// Blitsen.
export function notificationRecords(dump) {
  const indentation = line => line.length - line.trimStart().length;
  const records = [];
  let current = null;
  for (const line of dump.split("\n")) {
    const header = /NotificationRecord\([^:]*: pkg=(\S+)/.exec(line);
    if (header !== null) {
      const id = /\bid=(-?\d+)/.exec(line);
      current = {
        pkg: header[1],
        id: id === null ? null : Number(id[1]),
        title: null,
        channel: null,
        indent: indentation(line),
      };
      records.push(current);
      continue;
    }
    if (current === null || line.trim() === "") continue;
    if (indentation(line) <= current.indent) {
      current = null;
      continue;
    }
    const title = /android\.title=\w+ \((.*)\)/.exec(line);
    if (title !== null && current.title === null) current.title = title[1];
    const channel = /mId='([^']*)'/.exec(line) ?? /\bchannel(?:Id)?=(\S+)/.exec(line);
    if (channel !== null && current.channel === null) current.channel = channel[1];
  }
  return records.map(({ indent, ...record }) => record);
}

/// The fixture's transcript, reassembled out of the titles it posted.
///
/// `complete` is the point: a transcript missing its last batch would otherwise read
/// as a run whose later assertions simply had nothing to say about them.
export function transcript(titles) {
  const carrier = new RegExp(`^${PREFIX}-log-(\\d+) (.*)$`);
  const sentinel = new RegExp(`^${PREFIX}-done-(\\d+)$`);
  const entries = new Map();
  const batches = new Set();
  let announced = null;
  for (const title of titles) {
    const done = sentinel.exec(title);
    if (done !== null) {
      announced = Number(done[1]);
      continue;
    }
    const batch = carrier.exec(title);
    if (batch === null) continue;
    batches.add(Number(batch[1]));
    for (const pair of batch[2].split(" ; ")) {
      const split = pair.indexOf("=");
      if (split > 0) entries.set(pair.slice(0, split), pair.slice(split + 1));
    }
  }
  return { entries, announced, complete: announced !== null && batches.size === announced };
}

/// Whether `dumpsys package <id>` says the runtime permission is held.
///
/// The platform's own answer, kept beside the application's: a run in which the
/// fixture reports `granted` and the package manager disagrees has found something
/// far more interesting than a failed assertion about a title.
export function grantedInPackageDump(dump) {
  return new RegExp(`${PERMISSION.replace(/\./g, "\\.")}: granted=true`).test(dump);
}

/// The permission dialog's two answers, in the order preference goes: the
/// resource IDs `permission-controller` gives its buttons, then their labels for a
/// build that renamed them.
const BUTTONS = {
  allow: [/permission_allow_button$/, /^allow$/i],
  deny: [/permission_deny_button$/, /^(don.t allow|deny)$/i],
};

/** The dialog's buttons, if what is on screen is a permission dialog. */
function dialogButtons(nodes) {
  const found = {};
  for (const [answer, [byId, byText]] of Object.entries(BUTTONS)) {
    const node = nodes.find(candidate => byId.test(candidate.id))
      ?? nodes.find(candidate => byText.test(candidate.text.trim()));
    if (node !== undefined) found[answer] = node;
  }
  return Object.keys(found).length === 0 ? null : found;
}

/// Everything a run has found wrong, collected rather than thrown on.
///
/// An emulator boot costs minutes, so a run that stops at the first disagreement
/// spends them to answer one question. Every assertion below adds to this and the
/// run fails once, with all of it.
const failures = [];
const check = (held, complaint) => {
  if (!held) failures.push(complaint);
};

/** One `dumpsys notification` reading, kept whole so a failure has the raw text. */
function shade() {
  const dump = adb(["shell", "dumpsys", "notification", "--noredact"]).stdout;
  const records = notificationRecords(dump);
  return { dump, records, mine: records.filter(record => record.pkg === options.package) };
}

/** Every distinct title our package has posted, in a reading. */
const titlesOf = reading => [...new Set(reading.mine.map(record => record.title).filter(Boolean))];

/// The control: the platform posts, and this reads it back. See the header.
async function control() {
  const tag = `${PREFIX}-control`;
  const posted = adb(["shell", "cmd", "notification", "post", "-t", tag, tag, "control"]);
  if (posted.code !== 0) {
    throw new Error("`cmd notification post` failed on this device, so nothing has "
      + "established that a posted notification can be read back at all. That is the "
      + `Android build or adb, not Blitsen.\n  ${posted.stderr.trim() || posted.stdout.trim()}`);
  }
  const until = Date.now() + 20_000;
  while (Date.now() < until) {
    const reading = shade();
    if (reading.records.some(record => record.title === tag)) return;
    await sleep(1000);
  }
  await keep("dumpsys-control.txt", shade().dump);
  throw new Error(`the platform posted ${tag} and \`dumpsys notification --noredact\` did `
    + "not show it back with its title. Every assertion below reads titles out of that "
    + "dump, so they would all pass by finding nothing. Read dumpsys-control.txt: the "
    + "format this parses has changed, or extras are still redacted on this build.");
}

/// Puts the device into the state a scenario starts from.
///
/// `persisted` is the exception and the reason this is not one line: it must keep the
/// SharedPreferences the `deny` run wrote, so it stops the process and takes the
/// permission away without clearing the data that records having asked for it.
function prepare(scenario, sdk) {
  if (scenario === "persisted") {
    adbOrFail(["shell", "am", "force-stop", options.package], "stopping the application");
  } else {
    adbOrFail(["shell", "pm", "clear", options.package], "clearing the application's data");
  }
  if (sdk >= RUNTIME_PERMISSION_SDK) adb(["shell", "pm", "revoke", options.package, PERMISSION]);
}

/// Waits for the permission dialog and answers it, or reports that it never came.
async function answerDialog(scenario, answer) {
  const until = Date.now() + options.dialogWait;
  let seen = null;
  while (Date.now() < until) {
    const xml = uiHierarchy();
    if (xml !== null) {
      seen = xml;
      const buttons = dialogButtons(uiNodes(xml));
      if (buttons !== null && buttons[answer] !== undefined) {
        await keep(`ui-${scenario}.xml`, xml);
        tap(buttons[answer]);
        // Answered is not dismissed: the fixture only settles once the activity has
        // its focus back, so waiting here is waiting for the thing under test.
        const gone = Date.now() + 30_000;
        while (Date.now() < gone) {
          const after = uiHierarchy();
          if (after !== null && dialogButtons(uiNodes(after)) === null) return;
          await sleep(1000);
        }
        check(false, `the permission dialog was still on screen 30 s after "${answer}" was tapped`);
        return;
      }
    }
    await sleep(1000);
  }
  await keep(`ui-${scenario}.xml`, seen);
  check(false, `no permission dialog appeared within ${options.dialogWait} ms, so "${answer}" `
    + `was never given. ui-${scenario}.xml is the last screen this could read.`);
}

/// Waits for the package's own notifications to leave the shade.
///
/// Stopping an application cancels what it posted, but not instantly, and every
/// "was this ever posted" assertion below reads a timeline that starts here. One
/// notification left over from the previous scenario would answer that question
/// with the previous scenario's evidence, so this refuses to start until the shade
/// is the package's own and empty.
async function quiesce(scenario) {
  const until = Date.now() + 20_000;
  while (Date.now() < until) {
    const left = shade().mine;
    if (left.length === 0) return;
    await sleep(1000);
  }
  throw new Error(`${options.package} still had notifications posted twenty seconds after it `
    + `was stopped, so ${scenario} cannot tell what it posted from what the scenario before `
    + "it did.");
}

/// Polls the shade until the fixture's sentinel arrives, and samples the screen on
/// the way past.
///
/// The screen sampling is what makes "no dialog appeared" mean anything in the
/// scenarios that expect none: a window bounded by the clock could pass merely
/// because the application had not started yet, and this one is bounded by the
/// transcript being complete.
async function collect(scenario) {
  const started = Date.now();
  const until = started + options.deadline;
  const timeline = [];
  let dialog = null;
  let polls = 0;
  let reading = { dump: "", records: [], mine: [] };
  while (Date.now() < until) {
    if (!deviceIsAlive()) {
      throw new Error("the emulator stopped answering adb while the fixture was running. "
        + "#139 saw exactly this when the application initialised wgpu under a software "
        + "Vulkan; check the GPU mode this ran under before looking at notifications.");
    }
    reading = shade();
    timeline.push({ at: Date.now() - started, titles: titlesOf(reading) });
    if (transcript(titlesOf(reading)).complete) break;
    if (polls % 4 === 0) {
      const xml = uiHierarchy();
      if (xml !== null && dialogButtons(uiNodes(xml)) !== null) dialog = xml;
    }
    polls += 1;
    await sleep(1000);
  }
  await keep(`timeline-${scenario}.json`, `${JSON.stringify(timeline, null, 2)}\n`);
  await keep(`dumpsys-${scenario}.txt`, reading.dump);
  if (dialog !== null) await keep(`ui-${scenario}-unexpected.xml`, dialog);
  return { reading, timeline, dialog };
}

/// Everything the shade has to show once the fixture has finished, as complaints.
///
/// `final` is the last reading; `ever` is every title seen across the whole poll,
/// which is what makes a timeout assertable at all. Pure and exported so that
/// `android-notify.test.mjs` can hold it to a shade that is wrong in each of the
/// ways it claims to detect — an assertion nobody has watched fail is a comment.
export function shadeFailures({ final, ever, dump, channels }) {
  const failures = [];
  const check = (held, complaint) => {
    if (!held) failures.push(complaint);
  };
  const posted = (title, why) => check(ever.has(title),
    `${title} was never seen in the shade at any point, so ${why}`);
  const gone = (title, why) => check(!final.has(title), `${title} is still posted, so ${why}`);

  // Show, and then the replacement. The first title having been seen and then gone
  // is what separates "replaced in place" from "a second notification was added".
  posted(TITLES.alpha, "the first notification never reached the shade");
  posted(TITLES.beta, "the second notification never reached the shade");
  gone(TITLES.alpha, "the same-ID update added a notification rather than replacing one");
  check(final.has(TITLES.alphaUpdated),
    `${TITLES.alphaUpdated} is not posted, so the same-ID update did not reach the shade`);

  // Close, and that it addressed one ID. `ongoing` is the control for it: it was
  // posted before the close and must survive it.
  gone(TITLES.beta, "closing its ID left it on screen");
  check(final.has(TITLES.ongoing),
    `${TITLES.ongoing} is not posted, so closing another notification took it away too`);

  // The timeout, which is the one assertion that needs two points in time.
  posted(TITLES.expiring, `nothing distinguishes a ${EXPIRY} ms timeout from a notification `
    + "that was never delivered");
  gone(TITLES.expiring, `its ${EXPIRY} ms timeout did not expire it`);

  // Unknown IDs and rejected submissions reach the shade in exactly one way: they
  // do not.
  check(!ever.has(TITLES.ghost),
    `${TITLES.ghost} was posted, so updating an ID that was never shown created one`);
  for (const title of [`${PREFIX}-icon`, `${PREFIX}-actions`]) {
    check(!ever.has(title), `${title} was posted, and the submission that asked for it was `
      + "rejected — a rejected show must leave nothing behind");
  }

  // The channel. Every record of ours has to name the one the host creates, and the
  // dump has to contain it at all: a delivery filed under a channel nobody created
  // is how a notification silently becomes unblockable by the user.
  check(dump.includes(CHANNEL),
    `${CHANNEL} does not appear anywhere in the notification dump, so the channel #245 `
    + "creates before every submission was not created");
  // `channels` is empty when no record named one, which is a fact about this
  // Android build's dumpsys format rather than about Blitsen. The caller reports
  // that; asserting `size === 1` on nothing would report it as agreement.
  if (channels.length > 0) {
    check(new Set(channels).size === 1 && channels[0] === CHANNEL,
      `the package's notifications are filed under ${[...new Set(channels)].join(", ")}, and `
      + `every one of them has to be ${CHANNEL} — a second channel is a channel created per `
      + "submission rather than once");
  }
  return failures;
}

/// Everything the fixture has to have observed from inside the application, as
/// complaints. Pure and exported for the reason [`shadeFailures`] is.
export function transcriptFailures(sdk, scenario, entries) {
  const failures = [];
  const check = (held, complaint) => {
    if (!held) failures.push(complaint);
  };
  const is = (key, want) => check(entries.get(key) === want,
    `${key} was ${JSON.stringify(entries.get(key) ?? null)}, and the platform's answer to `
    + `that step is ${JSON.stringify(want)}`);
  const rejects = (key, fragment) => {
    const value = entries.get(key) ?? "";
    check(value.startsWith("err:") && value.includes(fragment),
      `${key} was ${JSON.stringify(value)}, and it has to be a rejection mentioning `
      + `${JSON.stringify(fragment)} — a failure the host swallowed would leave the promise `
      + "pending and the application with no way to report it");
  };

  const want = expected(sdk, scenario);
  is("p0", want.p0);
  is("r0", want.r0);
  is("p1", want.p1);
  // A submission made without the permission is the reachable half of "the
  // notification service is unavailable to this application": there is no adb that
  // takes NotificationManager away, and this is the same rejection path.
  if (want.p1 !== "granted") rejects("d.show", "permission");

  for (const key of ["s.alpha", "s.beta", "s.expiring", "s.ongoing"]) {
    check(!(entries.get(key) ?? "err:missing").startsWith("err"),
      `${key} did not settle to an ID: ${JSON.stringify(entries.get(key) ?? null)}`);
  }
  is("u.alpha", "true");
  is("u.missing", "false");
  is("c.beta", "true");
  is("c.beta2", "false");
  is("c.missing", "false");
  // A failure inside the JNI call, and one the host refuses in front of it. Both
  // have to arrive as a rejected promise; #245's contract is that no platform
  // callback ever enters JavaScript, so a swallowed error has nowhere else to go.
  rejects("e.icon", "Android notification API failed");
  rejects("e.actions", "#252");
  is("ev", `close:${entries.get("s.beta")}:closed`);
  return failures;
}

async function runScenario(scenario, sdk) {
  const want = expected(sdk, scenario);
  console.log(`\n${scenario}: ${want.p0} -> requestPermission() -> ${want.r0}`
    + `${want.dialog ? `, answering "${want.answer}"` : ", with no dialog"}`);
  prepare(scenario, sdk);
  await quiesce(scenario);
  adb(["logcat", "-c"]);

  const started = adb(["shell", "am", "start", "-W", "-n",
    `${options.package}/${options.activity}`]);
  if (started.code !== 0 || /Error/.test(started.stdout)) {
    throw new Error(`am start ${options.package}/${options.activity} failed\n  `
      + started.stdout.trim());
  }

  if (want.dialog) await answerDialog(scenario, want.answer);
  const packages = adb(["shell", "dumpsys", "package", options.package]).stdout;
  await keep(`package-${scenario}.txt`, packages);
  if (sdk >= RUNTIME_PERMISSION_SDK && want.dialog) {
    check(grantedInPackageDump(packages) === (want.answer === "allow"),
      `the package manager reports POST_NOTIFICATIONS granted=${grantedInPackageDump(packages)} `
      + `after "${want.answer}" was tapped, which is the opposite of what was tapped`);
  }
  // The transcript cannot leave the device without the permission, so a scenario
  // that ends without it is granted it from here. `--grant-delay` is why this is
  // safe: the fixture has already made all three of its permission reads.
  if (sdk >= RUNTIME_PERMISSION_SDK && want.p1 !== "granted") {
    await sleep(options.grantDelay);
    adbOrFail(["shell", "pm", "grant", options.package, PERMISSION],
      "granting the permission so the transcript can be posted");
  }

  const { reading, timeline, dialog } = await collect(scenario);
  await keep(`logcat-${scenario}.txt`, deviceIsAlive() ? adb(["logcat", "-d"]).stdout : null);
  const decoded = transcript(titlesOf(reading));
  console.log(`  ${timeline.length} readings, ${decoded.entries.size} transcript entries`);
  for (const [key, value] of decoded.entries) console.log(`    ${key}=${value}`);

  if (!decoded.complete) {
    check(false, `the fixture's transcript never completed: ${decoded.entries.size} entries and `
      + `${decoded.announced === null ? "no" : decoded.announced} announced batches after `
      + `${options.deadline} ms. logcat-${scenario}.txt and dumpsys-${scenario}.txt are what `
      + "it managed before it stopped.");
    return;
  }
  if (!want.dialog) {
    check(dialog === null, "a permission dialog appeared in a scenario that must not prompt: "
      + `on API ${sdk} with p0=${want.p0} the host answers from what it already knows`);
  }
  failures.push(...transcriptFailures(sdk, scenario, decoded.entries));

  const channels = reading.mine.map(record => record.channel).filter(Boolean);
  if (channels.length === 0) {
    console.warn(`  ${scenario}: no record in this dump named its channel, so only the `
      + `presence of ${CHANNEL} was checked. The field this reads has moved.`);
  }
  failures.push(...shadeFailures({
    final: new Set(titlesOf(reading)),
    ever: new Set(timeline.flatMap(snapshot => snapshot.titles)),
    dump: reading.dump,
    channels,
  }));

  const crash = adb(["logcat", "-d", "-b", "crash"]).stdout.trim();
  check(crash === "", `the crash log is not empty:\n${crash}`);
  check(adb(["shell", "pidof", options.package]).stdout.trim() !== "",
    `${options.package} is not running any more once its transcript was posted`);
}

async function main() {
  console.log(`device: ${await waitForBoot(options.settle)}`);
  const sdk = Number(adb(["shell", "getprop", "ro.build.version.sdk"]).stdout.trim());
  if (!Number.isInteger(sdk)) throw new Error("the device did not report an API level");
  console.log(`API level ${sdk}`);

  await control();

  const install = adb(["install", "-r", options.apk]);
  if (install.code !== 0 || !/Success/.test(install.stdout)) {
    throw new Error(`installing ${options.apk} failed\n  `
      + `${install.stdout.trim()}\n  ${install.stderr.trim()}`);
  }
  // Without `-g`: granting every permission at install is what the framebuffer smoke
  // test wants and the opposite of what this one does.
  const path = adb(["shell", "pm", "path", options.package]).stdout.trim();
  if (!path.startsWith("package:")) {
    throw new Error(`the APK installed, but nothing is registered as ${options.package}. `
      + "Pass the application id the build reported to --package.");
  }
  console.log(`installed: ${path}`);

  const scenarios = options.scenarios === null
    ? scenarioPlan(sdk)
    : options.scenarios.split(",").map(name => name.trim());
  console.log(`scenarios: ${scenarios.join(", ")}`);
  for (const scenario of scenarios) await runScenario(scenario, sdk);

  if (failures.length > 0) {
    throw new Error(`${failures.length} assertion${failures.length === 1 ? "" : "s"} failed`);
  }
  console.log(`\n${scenarios.length} scenario${scenarios.length === 1 ? "" : "s"} passed on `
    + `API ${sdk}.`);
}

// Guarded so the parsers above can be imported and tested without this reaching for
// an emulator.
if (import.meta.main) {
  if (!options.apk || !options.package) {
    console.error("usage: run-android-notify.mjs --apk <path> --package <application id> "
      + "[--activity <name>] [--serial <device>] [--out <dir>] [--scenarios <list>] "
      + "[--settle <ms>] [--deadline <ms>] [--grant-delay <ms>]");
    process.exit(2);
  }
  try {
    await main();
    for (const artifact of artifacts) console.log(`  wrote ${artifact}`);
  } catch (failure) {
    console.error(`android notify: ${failure.message}`);
    // The assertions collected before the run stopped, which the throw above is not
    // necessarily one of: a scenario that ends early still has whatever it learned.
    for (const collected of failures) console.error(`  - ${collected}`);
    for (const artifact of artifacts) console.error(`  wrote ${artifact}`);
    process.exit(1);
  }
}
