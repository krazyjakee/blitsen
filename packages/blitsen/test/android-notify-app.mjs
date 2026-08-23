// The application `run-android-notify.mjs` installs, in one place (issue #254).
//
// # Why the shade is the only channel out
//
// An APK's stdout and stderr go nowhere: `console.log` is `println!` in the host,
// and fd 1 of an Android application is `/dev/null`. `blitsen-android`'s `logcat`
// module writes only what `android_main` itself can say, and deliberately does not
// route the engine's records. `localStorage` is a session, not a file. `fetch`
// cannot open a socket, because the generated manifest asks for
// `POST_NOTIFICATIONS` and nothing else — an APK that reported its results over
// `adb reverse` would need `INTERNET` in every application Blitsen builds, which is
// a product change to make a test easier.
//
// So this fixture reports through the one API it is testing. Everything it observes
// is written into notification *titles*, and `adb shell dumpsys notification
// --noredact` reads them back. That has a consequence the harness has to live with:
// nothing can be reported until the permission is granted, so the transcript is
// held and flushed at the end, and the denial scenarios grant afterwards purely to
// let it out. It also has an advantage — the observation channel is the thing under
// test, so a run that reports anything at all has already proved delivery works.
//
// # Why the runtime namespace rather than `import notify from "blitsen/notify"`
//
// `blitsen build` packages this HTML as it is written. The `blitsen/notify` subpath
// is a package specifier that a bundler resolves, and `native:notify` is a spelling
// only the bundler plugins understand; neither survives into a runtime with no
// resolver and no `node_modules`. `packages/blitsen/src/native/module.mjs` proxies
// onto `globalThis[Symbol.for("blitsen.native")]`, which is what both spellings
// arrive at, so the fixture reads that directly and depends on no build step.

/// The one string the fixture and the harness both have to agree on. Everything
/// else is derived from it on both sides, so they cannot drift apart by half.
export const PREFIX = "blitsen";

/// Titles the harness looks for in `dumpsys`. `alpha` is shown under one title and
/// updated to another, which is the whole evidence that a same-ID update replaces
/// rather than adds: if it added, the first title would still be posted.
export const TITLES = {
  alpha: `${PREFIX}-alpha-1`,
  alphaUpdated: `${PREFIX}-alpha-2`,
  beta: `${PREFIX}-beta-1`,
  expiring: `${PREFIX}-expiring`,
  ongoing: `${PREFIX}-ongoing`,
  /// Asked for through an ID that was never shown. It must never reach the shade.
  ghost: `${PREFIX}-ghost`,
};

/// How long the expiring notification is given, and how long the fixture then waits
/// before it flushes. The wait is longer than the timeout because the harness has to
/// see the notification posted *and* gone, and a transcript that arrived first would
/// leave "expired" and "never delivered" looking identical.
export const EXPIRY = 6_000;

/// Log entries per carrier notification. Android drops the oldest once a package
/// holds more than twenty-five, and the probes are already using four of them.
export const LOG_BATCH = 3;

export const NOTIFY_APP = `<!doctype html><html><head><meta charset="utf-8"><title>Notify</title>
<style>html,body{margin:0;height:100%}body{display:grid;place-items:center;background:#101820;color:#f5f7fa;font:16px sans-serif}</style>
</head><body><main id="app">notify</main>
<script>
(async () => {
  const notify = globalThis[Symbol.for("blitsen.native")]?.notify;
  const say = text => { document.querySelector("#app").textContent = text; };
  const wait = ms => new Promise(done => setTimeout(done, ms));
  if (!notify || !notify.show) { say("blitsen/notify is absent"); return; }

  // A transcript entry is "key=value", entries are joined with " ; ", and neither
  // separator may appear in a value — the harness splits on both. Values are
  // therefore reduced to a charset that excludes them, and to one that excludes
  // ")" as well, because dumpsys prints an extra as "android.title=String (...)"
  // and a stray bracket would end the title early.
  const log = [];
  const clean = value => String(value).replace(/[^A-Za-z0-9 ._:,#-]+/g, " ").slice(0, 150).trim();
  const record = (key, value) => { log.push(key + "=" + clean(value)); };
  const settled = async (key, work) => {
    try {
      const value = await work;
      record(key, value);
      return value;
    } catch (failure) {
      record(key, "err:" + (failure && failure.message));
      return null;
    }
  };

  // Lifecycle events, in the order the frame turn delivered them. Android emits
  // only close today; tap, action and dismissal are #252.
  const events = [];
  notify.onEvent(event => {
    events.push(event.type + ":" + event.id + ":" + (event.reason || "none"));
  });

  // ① What the platform says before anything is asked of it, what asking settles
  //    to, and what it says afterwards. On API 32 all three are "granted" and no
  //    dialog is involved; on API 33+ the first is "default" on a clean install and
  //    "denied" once a previous launch has recorded that it asked.
  record("p0", await notify.permission());
  await settled("r0", notify.requestPermission());
  const decided = await notify.permission();
  record("p1", decided);

  // ② A submission made while the permission is not held has to reject rather than
  //    disappear. This is the reachable half of "the notification service is not
  //    available to this application": the host checks before it builds anything.
  if (decided !== "granted") {
    await settled("d.show", notify.show({ title: "${PREFIX}-denied", body: "must reject" }));
  }

  // ③ Nothing can be reported until the permission is held, so wait for it. The
  //    denial scenarios grant it from adb once they have seen what they came for.
  const waiting = Date.now();
  while (await notify.permission() !== "granted") {
    if (Date.now() - waiting > 150000) { say("no permission"); return; }
    say("waiting for permission");
    await wait(500);
  }
  record("w", Date.now() - waiting);

  // ④ Four notifications, then the operations that must touch one and not another.
  say("posting");
  const alpha = await settled("s.alpha", notify.show({ title: "${TITLES.alpha}", body: "alpha" }));
  const beta = await settled("s.beta", notify.show({ title: "${TITLES.beta}", body: "beta" }));
  await settled("s.expiring",
    notify.show({ title: "${TITLES.expiring}", body: "expiring", timeout: ${EXPIRY} }));
  await settled("s.ongoing",
    notify.show({ title: "${TITLES.ongoing}", body: "ongoing", timeout: 0 }));
  const posted = Date.now();
  // Long enough for the harness's one-second poll to see all four together, so
  // "expired" is later distinguishable from "never arrived".
  await wait(2500);

  await settled("u.alpha", notify.update(alpha, { title: "${TITLES.alphaUpdated}" }));
  await settled("u.missing", notify.update("no-such-id", { title: "${TITLES.ghost}" }));
  await settled("c.beta", notify.close(beta));
  await settled("c.beta2", notify.close(beta));
  await settled("c.missing", notify.close("no-such-id"));

  // ⑤ The two failures the host raises from inside the JNI call and from in front
  //    of it. A drawable that no package owns resolves to resource 0, which is the
  //    error NotificationManager would otherwise throw on at post time; actions are
  //    refused outright until #252 supplies an intent entry point.
  await settled("e.icon",
    notify.show({ title: "${PREFIX}-icon", body: "icon", icon: "blitsen_absent_drawable" }));
  await settled("e.actions", notify.show({
    title: "${PREFIX}-actions", body: "actions", actions: [{ id: "open", title: "Open" }],
  }));

  // ⑥ Past the expiring notification's own timeout, measured from when it was
  //    posted rather than from here, so the steps above are not counted twice.
  say("waiting for the timeout");
  await wait(Math.max(0, ${EXPIRY} + 2000 - (Date.now() - posted)));
  record("ev", events.join(","));

  // ⑦ The transcript, and then the sentinel that says how much of it there was.
  const batches = [];
  for (let at = 0; at < log.length; at += ${LOG_BATCH}) {
    batches.push(log.slice(at, at + ${LOG_BATCH}).join(" ; "));
  }
  for (let at = 0; at < batches.length; at += 1) {
    await notify.show({ title: "${PREFIX}-log-" + (at + 1) + " " + batches[at], body: "log" });
  }
  await notify.show({ title: "${PREFIX}-done-" + batches.length, body: "done" });
  say("done");
})().catch(failure => {
  document.querySelector("#app").textContent = "fixture failed: " + failure.message;
});
</script>
</body></html>
`;
