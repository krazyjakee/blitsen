# Licensing Blitsen and exported applications

Blitsen source is available under either Apache-2.0 or MIT, at your option. An exported application
also contains third-party runtime components with their own licenses. This page describes the
packaging behavior of the current release; it is not legal advice.

## Default desktop exports

The standard runtime includes Bun/JavaScriptCore and the Blitz native addon. Your
HTML, CSS and JavaScript remain application payload rather than part of that native link.

Every dependency's terms still apply. In particular:

- MIT and Apache-2.0 components require their notices to travel with the software.
- Stylo contains MPL-2.0-covered files. Binary distribution requires the corresponding covered
  source to remain available under the MPL terms.
- The `hidapi` crate behind `blitsen/hid` is MIT, and on Linux and Windows that is the whole of it:
  Blitsen selects the crate's pure-Rust backends, so no C is compiled and its MIT text is what the
  generated notices carry. macOS is the exception. There the crate compiles the vendored HIDAPI C
  library, which its author offers under GPL-3.0, BSD-3-Clause, or the original HIDAPI license, at
  the recipient's choice; Blitsen takes BSD-3-Clause, and nothing in a Blitsen export is
  distributed under GPL-3.0. The notice generator collects licence files that sit beside a crate's
  `Cargo.toml`, and that library's are two directories further down, so a macOS distributor should
  add `etc/hidapi/LICENSE-bsd.txt` from the `hidapi` crate to the notices the artifact carries.

The platform runtime package carries an audited notice set. `blitsen build` compresses those
notices into the exported executable so they travel with the application.

Inspect the finished artifact rather than relying on build logs:

```sh
./MyApp --licenses
```

On Windows:

```powershell
.\MyApp.exe --licenses
```

Retain the embedded notices and any source offer when redistributing the artifact. If the command
reports that no notices were embedded, do not assume the export is cleared for distribution.

## Bun and JavaScriptCore

Every desktop export now includes Bun. Bun's source is MIT-licensed and it links components with
other terms, including JavaScriptCore under LGPL-family terms. The pinned upstream inventory is
carried in `BUN-LICENSE.md` and printed by `--licenses` alongside the native addon notices.

That inventory and the Cargo notice audit do not certify the whole exported application for
redistribution. Distributors must retain the required license texts, covered source and relinking
materials for their exact Bun version, application and addons. See
[Bun's license documentation](https://bun.sh/docs/project/license) and the corresponding tagged
Bun source. Application assets and third-party Node-API addons retain their own obligations.

Android APK packaging is no longer supported.

## Distribution checklist

- Run the final artifact with `--licenses` and archive the output with the release.
- Keep required license text and covered source available for the period its terms require.
- Review licenses for application dependencies, fonts, images, media and native addons separately.
- Do not remove notices while signing, packaging or wrapping the Blitsen artifact.
- Obtain qualified legal review for commercial distribution or any addon-based export.

The complete third-party manifest is tied to the exact runtime version and platform. Re-run this
check for every target and every Blitsen upgrade.
