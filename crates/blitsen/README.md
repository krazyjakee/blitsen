# Blitsen

Blitsen is an experimental native runtime for applications built from static
HTML, CSS, and JavaScript output. It combines a statically linked QuickJS-ng
with Blitz's native HTML/CSS renderer, without embedding Chromium or an
operating-system WebView.

This Rust facade crate is intentionally minimal and currently exposes only its
package version. The pre-alpha runtime, CLI and platform binaries are distributed
through the [`blitsen` npm package](https://www.npmjs.com/package/blitsen); the
implementation and compatibility profile live at
[github.com/krazyjakee/blitsen](https://github.com/krazyjakee/blitsen).

Blitsen is an independent project built on Blitz. It is not an official
DioxusLabs project and is not endorsed by DioxusLabs.
