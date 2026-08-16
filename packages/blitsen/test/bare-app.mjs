// The bare application P1's size figure is written against, in one place.
//
// An HTML file that renders and nothing else. Anything larger measures the
// application rather than the runtime, and two size scripts measuring two
// different "bare" applications would produce two numbers nobody could put
// beside each other. `run-phase2-size.mjs` (desktop, issue #89) and
// `run-android-size.mjs` (Android, issue #150) both import this, which is what
// makes the desktop executable and the APK comparable at all.
export const BARE_APP = `<!doctype html><html><head><meta charset="utf-8"><title>Bare</title>
<style>html,body{margin:0;height:100%}body{display:grid;place-items:center;background:#101820;color:#f5f7fa;font:16px sans-serif}</style>
</head><body><main id="app">bare</main>
<script>document.querySelector("#app").textContent = "ready";</script>
</body></html>
`;
