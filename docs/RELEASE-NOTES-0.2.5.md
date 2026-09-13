# 0.2.5 — reliable clicks and headless application tests

Blitsen 0.2.5 fixes clicks after document scrolling and makes script-dispatched clicks
run control activation. It also adds a Node/Bun driver for testing packaged applications
without a display server or GPU. The JavaScript package and all six native runtimes
remain one exactly versioned release.

## Fixed

- Coordinate hit testing accounts for document scroll offsets and rejects points outside
  the viewport. A submit button reached by scrolling now receives its native mouse
  events at the position shown on screen.
- `MouseEvent('click')` dispatch runs activation through the same event algorithm as a
  native click. Submit buttons submit, checkboxes and radio buttons preactivate before
  listeners and restore state when cancelled, and labels forward clicks to their controls.
- `HTMLElement.click()` is implemented with disabled-control handling and a recursion
  guard. Activation reuses the native propagation path without extra parent lookups.

## Application testing

- `blitsen/test` launches a packaged QuickJS export from Node or Bun with coordinate
  pointer and wheel input, keyboard events, and text committed through the native IME path.
- Tests can query the live document by selector, text, role or accessible name, inspect
  layout and control state, wait for application conditions, and capture screenshots and
  dispatched-event logs on failure. Normal launches expose no test injection or inspection
  globals.
- The headless surface uses CPU rendering at device pixel ratio 1 and deterministic
  window/dialog fakes. It does not exercise the OS compositor or native dialogs. Legacy
  Bun-hosted exports retain document-script checks; the UI driver targets QuickJS exports.
- A packaged React regression covers scrolling, focus, text entry, disabled removal,
  rerendering between mouse down/up, submission, and covered or offscreen controls.

See [Testing an application](TESTING.md) for the driver API, examples and limits.

## Standalone checks

`BLITSEN_STANDALONE_CHECK=1` now explicitly identifies itself as a document-script
harness in CLI help, documentation and its own output. It does not exercise native input
or hit testing. Window and dialog members are feature-detectably absent in this mode;
startup code can guard access before calling them. Script-dispatched clicks run activation,
and other input-like dispatches warn when native default actions are skipped. Use
`blitsen/test` to prove a real interaction sequence.

## Distribution and signing

The npm packages contain a native addon and executable. Release artifacts remain unsigned
and are not notarised; these packaged runtimes are generally not checked by OS gatekeepers
during package-manager installation. Exported applications are different: application
authors are responsible for signing them and, on macOS, notarising them. `blitsen build
--sign` is the integration point. See [Releasing](RELEASING.md) and
[Packaging and distribution](PACKAGING.md#sign-the-artifact).
