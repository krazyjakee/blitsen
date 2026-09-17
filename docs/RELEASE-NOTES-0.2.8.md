# 0.2.8 — macOS text input fix

Blitsen 0.2.8 fixes a macOS failure that closed every application as soon as a text
control gained focus. Upgrading is recommended for all applications that run on macOS.

## Fixed

- **macOS applications no longer exit when a text control is focused.** 0.2.7 printed
  `blitsen: could not enable IME: ime is already enabled.` and exited when an `<input>`
  or `<textarea>` gained focus, including through `autofocus` at startup. The pinned
  winit beta could not turn the platform input method off once it was on, so moving it
  to the newly focused control failed. Windows and Linux were not affected.
- A new native test moves focus between text fields, a textarea, a button and no focus
  in a real macOS window, including on Intel Macs using the software-rendered window.
  `blitsen/test` replaces the window, so it cannot catch this failure.

## Platform layer

- winit moves to `0.31.0-beta.3` and Blitz to the revision built against it.
- File drops use winit's new data-transfer API. The document receives no drag events
  until the dragged files are known. Drags that name no local file (a web link, for
  example) are refused and never reach the document. Finder file-reference URLs
  resolve to real paths. The `dragenter`, `dragover`, `dragleave` and `drop` contract
  is unchanged.

## Distribution

Release artifacts remain unsigned and are not notarised. Application authors are
responsible for signing their exports and sidecars and, on macOS, notarising them.
See [Packaging and distribution](PACKAGING.md).
