# Blitsen

Blitsen is an experimental native runtime for applications built from static
HTML, CSS, and JavaScript output. It combines JavaScriptCore with Blitz's native
HTML/CSS renderer; it does not embed Chromium or an operating-system WebView.

The project is pre-alpha. Its Linux x64 feasibility review concluded **go,
re-scoped**; Windows and macOS validation is deferred. See the
[M0 decision](docs/M0.md), [product specification](docs/PRODUCT.md),
[technical specification](docs/TECH.md), and
[licensing/export requirements](docs/LICENSING.md).

Blitsen is an independent project built on Blitz. It is not an official
DioxusLabs project and is not endorsed by DioxusLabs.

## Licence

Blitsen source is dual-licensed under Apache-2.0 or MIT. Exported applications
also contain third-party components with their own terms. In particular,
JavaScriptCore includes LGPL-family code: closed-source applications are
supported, but exporters must preserve notices/source offers and a recipient's
ability to replace or relink JSC. Read [docs/LICENSING.md](docs/LICENSING.md)
before distributing an application.
