# Testing an application

`blitsen/test` drives a packaged desktop export from Node or Bun without a display
server, GPU, or OS input injection. Build the application with `blitsen build`,
then launch the executable from a test:

```js
import { launch } from 'blitsen/test';

const app = await launch('./MyApp', {
  width: 1180,
  height: 780,
  env: { APP_TEST_DATA: '/tmp/test-data' },
  artifactsDir: './test-results',
});
try {
  await app.waitFor(() => document.querySelector('form'));
  await app.click({ role: 'textbox', name: 'Local folder' });
  await app.type('/path/to/repository');
  await app.wheel({ x: 400, y: 500, deltaY: 500 });
  await app.click({ role: 'button', name: 'Save repository' });
  await app.assert(() => document.querySelector('[data-saved]') !== null);
} finally {
  await app.close();
}
```

The application runs in its own Bun process, including when the test driver
is Bun. Use the executable inside a macOS `.app`, or the exported `.exe` on
Windows. During
runtime development, `launch(runtimePath, { args: [builtDirectory] })` runs a
built directory through the same path. CI runs the packaged React acceptance
fixture with both drivers on Linux, macOS and Windows (`bun run --cwd
packages/blitsen test:ui`).

Input enters through the native host's retained pointer, keyboard and IME
callbacks. Pointer commands hit-test coordinates against the live, flushed
layout; they do not dispatch to the element returned by a query. The application
loader, DOM, focus on mousedown, click activation, editing, default scrolling,
asynchronous handoff and layout are shared with native windows. Presentation
uses Blitz's CPU rasterizer, replacing the OS window and GPU surface. This does
not test the platform compositor, native dialogs or OS input translation.

`click(locator)` requires exactly one enabled element whose box center is in the
viewport and is not covered. It sends pointer move/down/up at that center, with a
frame turn between them. Scroll explicitly before clicking below the fold. A
locator can be a CSS selector, `{ selector, text }`, or `{ role, name }`. Text
matches normalize whitespace. Names recognize `aria-labelledby`, `aria-label`,
labels, element text, image alt text and button values. Roles cover explicit
`role` attributes and common HTML controls; this is not a complete accessibility
tree or implementation of the accessible-name specification.

`query(locator)` returns detached JSON snapshots: layout boxes in window CSS
pixels, device pixel ratio, viewport size, disabled/value/checked state, focus,
and scroll offsets. The headless surface currently uses device pixel ratio 1.
`evaluate(expression)` evaluates an assertion expression in the live document
and returns its JSON value. Expressions and predicates must be synchronous;
use `waitFor` to observe asynchronous work. Functions passed to `evaluate`, `assert` and
`waitFor` are serialized into that process and cannot capture driver variables.
Inspection through `query` is read-only; assertion scripts are trusted test code
and can modify the document.

For lower-level input use `pointer('pointerdown', { x, y, button, ...modifiers })`,
`pointer('pointerup', ...)`, `key('keydown', { key, code, ...modifiers })`, and
`key('keyup', ...)`. `type(text)` commits text through the native IME path into
the focused control. Pointer buttons use DOM numbering (primary 0, auxiliary 1,
secondary 2); modifiers are `ctrlKey`, `shiftKey`, `altKey`, and `metaKey`.
Wheel deltas are CSS pixels, positive down/right.

`settle(ms = 50)` advances timers, microtasks, native completions, animation frames
and layout for a bounded duration. It does not promise network idleness or wait
for repeating timers to disappear. Use `waitFor(predicate, { timeout })` for
application-specific readiness. The process continues frame turns while waiting
for the next driver command. Commands time out and terminate the child instead
of allowing a late action to affect another step. Always close the session.

`screenshot(path?)` returns PNG bytes and optionally writes them.
`eventLog()` returns and drains the last 1,000 dispatched events, including type,
target, coordinates and `defaultPrevented` after listeners. Failed interaction
and assertion steps attach `screenshot` and `events` to their Error; supplying
`artifactsDir` also writes a PNG and JSON log. `screenshots: 'steps'` additionally
records successful interaction and assertion steps into that directory.

`launch` explicitly sets `BLITSEN_TEST_MODE=1` on the child process. Only this
mode reads the private stdin command protocol; normal launches install no test
injection or inspection globals. Test mode supplies an in-memory window fake
before startup scripts run. Size changes update the live viewport; fullscreen,
decoration and maximization setters round-trip their state. Other window actions
are no-ops. File/folder dialogs resolve to `null`; message dialogs resolve to
`'cancel'`. No native dialog opens. Application environment and storage are real;
set isolated data directories in `env` when a test needs separate state.

## Document-script checks

`BLITSEN_STANDALONE_CHECK=1 ./MyApp` remains a lightweight document-script harness
on Bun exports. It loads scripts, advances asynchronous
work, optionally evaluates `BLITSEN_STANDALONE_CHECK_SCRIPT` and
`BLITSEN_STANDALONE_CHECK_ASSERT`, and renders a frame. It does **not** prove a
native click, hit testing, focus, or scroll-dependent interaction. CLI help and
each check's stderr state this limitation.

Window and dialog members are absent in this mode, allowing feature detection:

```js
import windowApi from 'blitsen/window';
import dialog from 'blitsen/dialog';
if (windowApi.setDecorations) windowApi.setDecorations(false);
if (dialog.openFolder) await dialog.openFolder();
```

`element.click()` and dispatched `MouseEvent('click', ...)` run activation on
both hosts: submit buttons submit, checkables preactivate before listeners and
restore their state on cancellation, and labels forward clicks. Native clicks
use that same dispatch algorithm. Other script-dispatched input-like events do
not run native default actions; a document check warns once per event type.
Dispatching `submit` directly proves only the submit handler. Use the UI driver
to prove the interaction that reaches it.
