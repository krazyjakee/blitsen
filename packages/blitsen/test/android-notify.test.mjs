// The parts of the Android notification coverage (#254) that can be measured
// without an emulator, and the ones most worth measuring.
//
// The harness reads two pieces of device text and asserts against nothing else: the
// records in `dumpsys notification --noredact` and the nodes in a `uiautomator`
// dump. Both parsers fail the same way when they are wrong — they find nothing —
// and finding nothing is indistinguishable from an application that posted nothing
// and a permission dialog that never appeared, which are the two failures the whole
// job exists to detect. A silent parser would turn every one of them into a pass.
//
// So both are held here to real device text, and the fixture and the harness are
// held to each other: they agree on notification titles across a template string and
// a set of regexes, with no build step in between.
import { describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { uiNodes } from "./android-device.mjs";
import { EXPIRY, LOG_BATCH, NOTIFY_APP, PREFIX, TITLES } from "./android-notify-app.mjs";
import {
  CHANNEL, PERMISSION, RUNTIME_PERMISSION_SDK, expected, grantedInPackageDump,
  notificationRecords, scenarioPlan, shadeFailures, transcript, transcriptFailures,
} from "./run-android-notify.mjs";

/// A `dumpsys notification --noredact` extract, in the shape the tool prints: a
/// header line carrying the package and the ID, fields indented under it, and
/// sections after the list that are indented no further than the header and are not
/// part of the last record.
const DUMP = `Current Notification Manager state (NotificationRecords):
  Notification List:
    NotificationRecord(0x4d0f: pkg=com.blitsen.notifyfixture user=0 id=1 tag=null importance=3 key=0|com.blitsen.notifyfixture|1|null|10152: )
      uid=10152 userId=0
      icon=0x1080077
      channel=NotificationChannel{mId='blitsen.default', mName=General, mImportance=3}
      extras={
        android.title=String (${PREFIX}-alpha-2)
        android.text=String (alpha)
        android.showChronometer=Boolean (false)
      }
    NotificationRecord(0x4d10: pkg=com.blitsen.notifyfixture user=0 id=4 tag=null importance=3 key=0|com.blitsen.notifyfixture|4|null|10152: )
      uid=10152 userId=0
      channel=NotificationChannel{mId='blitsen.default', mName=General, mImportance=3}
      extras={
        android.title=String (${PREFIX}-ongoing)
      }
    NotificationRecord(0x4d11: pkg=com.android.shell user=0 id=2 tag=null importance=3 key=0|com.android.shell|2|null|2000: )
      uid=2000 userId=0
      extras={
        android.title=String (${PREFIX}-control)
      }
  mUseAttentionLight=true
  mSystemReady=true
`;

describe("the notification records the harness reads out of dumpsys", () => {
  test("reads the package, the ID, the title and the channel of each record", () => {
    const records = notificationRecords(DUMP);
    expect(records).toEqual([
      { pkg: "com.blitsen.notifyfixture", id: 1, title: `${PREFIX}-alpha-2`,
        channel: "blitsen.default" },
      { pkg: "com.blitsen.notifyfixture", id: 4, title: `${PREFIX}-ongoing`,
        channel: "blitsen.default" },
      { pkg: "com.android.shell", id: 2, title: `${PREFIX}-control`, channel: null },
    ]);
  });

  test("does not read the sections after the list as the last record's extras", () => {
    // The failure this guards against is quiet: a block reader that ran to the next
    // header would attribute every line of the rest of the dump to the last record,
    // and the rest of the dump mentions channel IDs the harness then asserts on.
    const trailing = `${DUMP}  Notification Preferences:\n    android.title=String (mistake)\n`;
    const last = notificationRecords(trailing).at(-1);
    expect(last.title).toBe(`${PREFIX}-control`);
  });

  test("finds nothing in text that holds no records, rather than inventing one", () => {
    expect(notificationRecords("")).toEqual([]);
    expect(notificationRecords("Current Notification Manager state:\n  none\n")).toEqual([]);
  });

  test("keeps a title that contains the separators the transcript is built from", () => {
    // Transcript titles carry "key=value ; key=value" and end at the extra's closing
    // bracket, so both have to survive being read back off one line.
    const dump = DUMP.replace(`${PREFIX}-ongoing`, `${PREFIX}-log-1 p0=granted ; r0=granted`);
    expect(notificationRecords(dump)[1].title).toBe(`${PREFIX}-log-1 p0=granted ; r0=granted`);
  });
});

describe("the fixture's transcript, reassembled out of titles", () => {
  const titles = [
    `${PREFIX}-alpha-2`,
    `${PREFIX}-log-1 p0=default ; r0=denied ; p1=denied`,
    `${PREFIX}-log-2 d.show=err:notification permission has not been granted ; w=4210`,
    `${PREFIX}-done-2`,
  ];

  test("splits entries, keeps values whole, and knows when it has all of them", () => {
    const decoded = transcript(titles);
    expect(decoded.complete).toBe(true);
    expect(decoded.announced).toBe(2);
    expect(Object.fromEntries(decoded.entries)).toEqual({
      p0: "default",
      r0: "denied",
      p1: "denied",
      "d.show": "err:notification permission has not been granted",
      w: "4210",
    });
  });

  test("a transcript missing a batch is incomplete rather than partly believed", () => {
    // The assertions downstream all read `entries`, and a missing batch would make
    // them read `undefined` for steps that simply had not arrived yet.
    const short = transcript(titles.filter(title => !title.startsWith(`${PREFIX}-log-2`)));
    expect(short.complete).toBe(false);
    expect(short.entries.get("p0")).toBe("default");
    expect(transcript(titles.filter(title => !title.includes("done"))).complete).toBe(false);
    expect(transcript([]).announced).toBe(null);
  });
});

/// One real `uiautomator dump` of an API 33 POST_NOTIFICATIONS dialog, trimmed to
/// the nodes that matter and keeping the attribute order and escaping the tool
/// emits.
const HIERARCHY = `<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy rotation="0">
  <node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.android.permissioncontroller" content-desc="" bounds="[0,0][1080,2280]">
    <node index="1" text="Allow Blitsen &amp; Co to send you notifications?" resource-id="com.android.permissioncontroller:id/permission_message" class="android.widget.TextView" package="com.android.permissioncontroller" content-desc="" bounds="[54,908][1026,1050]" />
    <node index="2" text="Allow" resource-id="com.android.permissioncontroller:id/permission_allow_button" class="android.widget.Button" package="com.android.permissioncontroller" content-desc="" bounds="[100,1200][980,1340]" />
    <node index="3" text="Don&apos;t allow" resource-id="com.android.permissioncontroller:id/permission_deny_button" class="android.widget.Button" package="com.android.permissioncontroller" content-desc="" bounds="[100,1360][980,1500]" />
  </node>
</hierarchy>`;

describe("the screen the harness taps", () => {
  test("reads every node, its text and the point that taps it", () => {
    const nodes = uiNodes(HIERARCHY);
    expect(nodes).toHaveLength(4);
    const allow = nodes.find(node => node.id.endsWith("permission_allow_button"));
    expect(allow.text).toBe("Allow");
    // The centre of [100,1200][980,1340]. A tap computed from the wrong corner lands
    // on the other button, which would answer the dialog the other way and pass.
    expect([allow.x, allow.y]).toEqual([540, 1270]);
    expect(allow.width).toBe(880);
  });

  test("decodes the entities uiautomator escapes, so a label still matches", () => {
    const deny = uiNodes(HIERARCHY).find(node => node.id.endsWith("permission_deny_button"));
    expect(deny.text).toBe("Don't allow");
    expect(/^(don.t allow|deny)$/i.test(deny.text)).toBe(true);
    expect(uiNodes(HIERARCHY)[1].text).toBe("Allow Blitsen & Co to send you notifications?");
  });

  test("finds nothing in a dump with no nodes, rather than one node of nothing", () => {
    // `uiautomator dump` fails with "could not get idle state" while a window is
    // animating, and the caller polls; a parser that produced a node from that
    // output would tap a point on an unrelated screen.
    expect(uiNodes("ERROR: could not get idle state.")).toEqual([]);
    expect(uiNodes("<hierarchy rotation=\"0\" />")).toEqual([]);
  });
});

describe("the scenarios each API level can answer", () => {
  test("below 33 there is one, and it is delivery with no runtime permission at all", () => {
    expect(scenarioPlan(32)).toEqual(["implicit"]);
    expect(expected(32, "implicit")).toMatchObject({ p0: "granted", r0: "granted", dialog: false });
  });

  test("at 33 and above, denial is answered before it is read back and then granted", () => {
    // The order is not free: `persisted` asserts that a *new process* still reads
    // `denied`, which is only true because `deny` ran first and #245 recorded the
    // request. Running it first, or after the grant, would assert nothing.
    expect(scenarioPlan(RUNTIME_PERMISSION_SDK)).toEqual(["deny", "persisted", "grant"]);
    expect(scenarioPlan(34)).toEqual(["deny", "persisted", "grant"]);
    expect(expected(33, "deny")).toMatchObject({ p0: "default", r0: "denied", answer: "deny" });
    expect(expected(33, "persisted")).toMatchObject({ p0: "denied", r0: "denied", dialog: false });
    expect(expected(33, "grant")).toMatchObject({ p0: "default", r0: "granted", answer: "allow" });
    expect(() => expected(33, "invented")).toThrow("is not a scenario this knows");
  });
});

describe("the permission state the package manager reports", () => {
  test("reads granted and not-granted apart", () => {
    const dump = at => `    requested permissions:\n      ${PERMISSION}\n`
      + `    runtime permissions:\n      ${PERMISSION}: granted=${at}, flags=[ USER_SET ]\n`;
    expect(grantedInPackageDump(dump("true"))).toBe(true);
    expect(grantedInPackageDump(dump("false"))).toBe(false);
    // A package that never declared it is not granted it, and the dots in the
    // permission name are dots rather than "any character" — a dump listing some
    // other vendor's near-identical name must not answer for this one.
    expect(grantedInPackageDump("androidXpermissionXPOST_NOTIFICATIONS: granted=true"))
      .toBe(false);
    expect(grantedInPackageDump(`${PERMISSION}X: granted=true`)).toBe(false);
    expect(grantedInPackageDump("")).toBe(false);
  });
});

/// A stand-in for `notify/android.rs`, answering exactly as that host does.
///
/// Not a model of Android: a model of the *host's* answers, which is what the
/// fixture is written against. `show` refuses without the permission and refuses
/// actions the way #245 refuses them; an icon that resolves to no drawable comes
/// back as the JNI failure the host formats; `update` and `close` of an ID that was
/// never shown resolve `false` rather than throwing.
///
/// `grantAfter` is the adb grant the denial scenarios perform, counted in permission
/// reads: the fixture polls until it is granted, so the poll is what advances it.
function fakeHost({ initial, request, grantAfter = null }) {
  const posted = new Map();
  const shown = [];
  const listeners = [];
  let permission = initial;
  let reads = 0;
  let next = 1;
  return {
    shown,
    posted,
    notify: {
      permission: async () => {
        reads += 1;
        if (grantAfter !== null && reads >= grantAfter) permission = "granted";
        return permission;
      },
      requestPermission: async () => {
        permission = request;
        return request;
      },
      show: async given => {
        // `dom_bridge/bootstrap/native.js` normalises before the host ever sees the
        // options, and the defaults it fills in are what `notify/android.rs` reads.
        // A fake that skipped this would make the fixture's omissions look like
        // deliberate `undefined`s.
        const options = { icon: null, actions: [], timeout: null, ...given };
        if (permission !== "granted") {
          throw new Error("notification permission has not been granted");
        }
        if (options.actions.length > 0) {
          throw new Error("notification actions require Android activation routing, "
            + "tracked by issue #252");
        }
        if (options.icon !== null) {
          throw new Error("Android notification API failed: JNI call failed: "
            + "null pointer: notification icon resource");
        }
        // `dom_bridge/notify.rs` spells a session-scoped public ID `n<counter>`,
        // and the fixture puts it in the transcript, so the shape matters.
        const id = `n${next}`;
        next += 1;
        posted.set(id, options.title);
        shown.push(options.title);
        return id;
      },
      update: async (id, patch) => {
        if (!posted.has(id)) return false;
        posted.set(id, patch.title ?? posted.get(id));
        if (patch.title !== undefined) shown.push(patch.title);
        return true;
      },
      close: async id => {
        if (!posted.has(id)) return false;
        posted.delete(id);
        for (const listener of listeners) listener({ type: "close", id, reason: "closed" });
        return true;
      },
      onEvent: listener => {
        listeners.push(listener);
        return () => {};
      },
    },
  };
}

/// Runs the fixture's own script, with the runtime it reaches for replaced.
///
/// The host is what the fixture calls; `setTimeout` is passed in so the eight
/// seconds it spends waiting out a notification timeout cost nothing here. Its
/// completion is the sentinel it posts, because the script is an IIFE and returns
/// before any of its work is done — which is exactly how it behaves on a device.
async function runFixture(host) {
  const body = /<script>([\s\S]*?)<\/script>/.exec(NOTIFY_APP)[1];
  const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;
  const script = new AsyncFunction("globalThis", "document", "setTimeout", body);
  const element = { textContent: "" };
  const finished = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`the fixture never posted its sentinel; `
      + `it said ${JSON.stringify(element.textContent)}`)), 5_000);
    const shown = host.notify.show;
    host.notify.show = async options => {
      const id = await shown(options);
      if (new RegExp(`^${PREFIX}-done-\\d+$`).test(options.title)) {
        clearTimeout(timer);
        resolve();
      }
      return id;
    };
  });
  await script(
    { [Symbol.for("blitsen.native")]: { notify: host.notify } },
    { querySelector: () => element },
    callback => setTimeout(callback, 0),
  );
  await finished;
  // The sentinel resolves from inside `show`, so the fixture's own line after it
  // has not run yet. One turn, and then what is on screen is its last word.
  await new Promise(done => setTimeout(done, 0));
  return { titles: host.shown, status: element.textContent };
}

// The fixture is a template string that is packaged into an APK and never runs
// anywhere else, so nothing else would ever execute it. Running it against the
// host's answers and then putting the result through the harness's own assertions
// is what shows the two halves agree — a fixture that reported `u.missing` under
// another name would pass every test above and fail only on an emulator, an hour
// into a CI run.
describe("the fixture, driven end to end against the answers the host gives", () => {
  test("API 32: granted from the start, and every delivery assertion satisfied", async () => {
    const host = fakeHost({ initial: "granted", request: "granted" });
    const { titles, status } = await runFixture(host);
    expect(status).toBe("done");
    const decoded = transcript(titles);
    expect(decoded.complete).toBe(true);
    expect(transcriptFailures(32, "implicit", decoded.entries)).toEqual([]);
    // And the shade it would leave behind, read the way the harness reads it. The
    // one thing the fake cannot do is expire a notification — `setTimeoutAfter` is
    // the platform's, not the host's, and it is the emulator's job to prove it — so
    // the expiring one is taken out of the final state here by hand.
    expect(shadeFailures({
      final: new Set([...host.posted.values()].filter(title => title !== TITLES.expiring)),
      ever: new Set(titles),
      dump: `channel=NotificationChannel{mId='${CHANNEL}'}`,
      channels: [CHANNEL],
    })).toEqual([]);
  });

  test("API 33 denial: the refusal is reported, and reported before the grant", async () => {
    // `grantAfter` is the fourth read: p0, p1, one poll that is still denied, then
    // the grant the harness performs from adb.
    const host = fakeHost({ initial: "default", request: "denied", grantAfter: 4 });
    const { titles } = await runFixture(host);
    const decoded = transcript(titles);
    expect(decoded.complete).toBe(true);
    expect(transcriptFailures(33, "deny", decoded.entries)).toEqual([]);
    // The rejection a denied submission has to produce, which is the only evidence
    // that `show` refused rather than posted nothing quietly.
    expect(decoded.entries.get("d.show")).toBe("err:notification permission has not been granted");
    expect(transcriptFailures(33, "grant", decoded.entries)).not.toEqual([]);
  });

  test("the titles it posts are the ones the harness looks for, and no others", async () => {
    const host = fakeHost({ initial: "granted", request: "granted" });
    const { titles } = await runFixture(host);
    expect(titles).toContain(TITLES.alpha);
    expect(titles).toContain(TITLES.alphaUpdated);
    expect(titles).toContain(TITLES.beta);
    expect(titles).toContain(TITLES.ongoing);
    // Nothing rejected reached the shade, and neither did the update of an ID that
    // was never shown.
    expect(titles).not.toContain(TITLES.ghost);
    expect(titles).not.toContain(`${PREFIX}-icon`);
    expect(titles).not.toContain(`${PREFIX}-actions`);
    // Under twenty-five, which is where Android starts dropping a package's oldest.
    expect(titles.length).toBeLessThan(25);
  });
});

describe("the shade assertions, held to shades that are wrong", () => {
  const shade = changes => {
    const ever = new Set([TITLES.alpha, TITLES.beta, TITLES.expiring, TITLES.ongoing,
      TITLES.alphaUpdated]);
    const final = new Set([TITLES.alphaUpdated, TITLES.ongoing]);
    return shadeFailures({
      final, ever, dump: `mId='${CHANNEL}'`, channels: [CHANNEL], ...changes(final, ever),
    });
  };

  test("a shade that is right produces nothing", () => {
    expect(shade(() => ({}))).toEqual([]);
  });

  test("an update that added rather than replaced is caught", () => {
    // Both notifications posted, which is what a `notify` on a fresh ID would leave.
    expect(shade(final => ({ final: new Set([...final, TITLES.alpha]) })))
      .toEqual([expect.stringContaining("added a notification rather than replacing")]);
  });

  test("a close that took the wrong notification, or none, is caught", () => {
    expect(shade(final => ({ final: new Set([...final, TITLES.beta]) })))
      .toEqual([expect.stringContaining("closing its ID left it on screen")]);
    expect(shade(final => ({ final: new Set([...final].filter(t => t !== TITLES.ongoing)) })))
      .toEqual([expect.stringContaining("closing another notification took it away too")]);
  });

  test("a timeout that never expired, and one that never arrived, differ", () => {
    expect(shade(final => ({ final: new Set([...final, TITLES.expiring]) })))
      .toEqual([expect.stringContaining("did not expire it")]);
    expect(shade((final, ever) => ({
      ever: new Set([...ever].filter(title => title !== TITLES.expiring)),
    }))).toEqual([expect.stringContaining("was never seen in the shade")]);
  });

  test("a notification an unknown ID created, or a rejected show left, is caught", () => {
    expect(shade((final, ever) => ({ ever: new Set([...ever, TITLES.ghost]) })))
      .toEqual([expect.stringContaining("updating an ID that was never shown created one")]);
    expect(shade((final, ever) => ({ ever: new Set([...ever, `${PREFIX}-icon`]) })))
      .toEqual([expect.stringContaining("rejected show must leave nothing behind")]);
  });

  test("a missing channel and a second channel are both caught", () => {
    expect(shade(() => ({ dump: "" })))
      .toEqual([expect.stringContaining("was not created")]);
    expect(shade(() => ({ channels: [CHANNEL, "blitsen.other"] })))
      .toEqual([expect.stringContaining("channel created per submission rather than once")]);
    // A dumpsys that named no channel at all is not agreement, and must not be
    // reported as a second channel either.
    expect(shade(() => ({ channels: [] }))).toEqual([]);
  });
});

describe("the fixture and the harness agree with the rest of the repository", () => {
  test("every title the harness asserts on is one the fixture posts", () => {
    for (const title of Object.values(TITLES)) expect(NOTIFY_APP).toContain(title);
    // The carrier and sentinel titles are built from PREFIX on both sides, so the
    // shape is checked rather than the string.
    expect(NOTIFY_APP).toContain(`"${PREFIX}-log-"`);
    expect(NOTIFY_APP).toContain(`"${PREFIX}-done-"`);
    expect(transcript([`${PREFIX}-done-0`]).complete).toBe(true);
  });

  test("the fixture waits out the timeout it asks for", () => {
    // A fixture that flushed its transcript before its own `timeout` had elapsed
    // would leave the harness unable to tell an expiry from a notification that was
    // never delivered — which is a pass, because both end with nothing on screen.
    expect(NOTIFY_APP).toContain(`timeout: ${EXPIRY}`);
    expect(NOTIFY_APP).toContain(`${EXPIRY} + 2000`);
    expect(LOG_BATCH).toBeGreaterThan(0);
  });

  test("the permission the harness grants is the one the packager asks for", () => {
    // Two spellings of one string with no build step between them, which is the
    // shape of thing `cli-android.test.mjs` already guards for the asset index.
    return readFile(join(import.meta.dir, "../src/android-apk.mjs"), "utf8")
      .then(source => expect(source).toContain(`android:name="${PERMISSION}"`));
  });

  test("the CI job runs the emulator coverage on both sides of the API 33 split", async () => {
    // 32 and 33 are not two runs of one test: below 33 the permission does not
    // exist and delivery has no prompt in it at all, which is a different path
    // through the host. A matrix that lost either side would still be green.
    const workflow = await readFile(join(import.meta.dir, "../../../.github/workflows/ci.yml"),
      "utf8");
    const levels = /api-level:\s*\[([^\]]*)\]/.exec(workflow);
    expect(levels).not.toBe(null);
    const matrix = levels[1].split(",").map(level => Number(level.trim()));
    expect(matrix.some(level => level < RUNTIME_PERMISSION_SDK)).toBe(true);
    expect(matrix.some(level => level >= RUNTIME_PERMISSION_SDK)).toBe(true);
  });

  test("the package the emulator job installs is the one the build step named", async () => {
    // The application ID is given to `blitsen build` in one job and to the harness
    // in another. They are two literals in one file with nothing deriving either,
    // and a mismatch is an hour of CI reporting that the APK installed and nothing
    // is registered under that name.
    const workflow = await readFile(join(import.meta.dir, "../../../.github/workflows/ci.yml"),
      "utf8");
    const built = /--android-package (com\.\S+)/.exec(workflow);
    const installed = /--package (com\.\S+)/.exec(workflow);
    expect(built).not.toBe(null);
    expect(installed).not.toBe(null);
    expect(installed[1]).toBe(built[1]);
    expect(workflow).toContain("test:android-notify");
  });
});
