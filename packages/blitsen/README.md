# Blitsen

Blitsen is an experimental native runtime for applications built from static
HTML, CSS, and JavaScript output. It combines JavaScriptCore with Blitz's native
HTML/CSS renderer without embedding Chromium or an operating-system WebView.

This package is **pre-alpha**. Directory mode is available for the first runtime
milestone when a compatible native runtime package is installed:

```sh
npx blitsen . --width 800 --height 600 --title "My app"
```

It resolves `index.html`, preflights local entrypoint assets, and opens the result
in a native window. Application export is not implemented yet. Follow development
and read the feasibility results at
[github.com/krazyjakee/blitsen](https://github.com/krazyjakee/blitsen).

Blitsen is an independent project built on Blitz. It is not an official
DioxusLabs project and is not endorsed by DioxusLabs.
