# Licensing Blitsen and exported applications

Blitsen source is available under either Apache-2.0 or MIT, at your option. An exported application
also contains third-party runtime components with their own licenses. This page describes the
packaging behavior of the current release; it is not legal advice.

## Default desktop exports

The standard runtime statically links QuickJS-ng (MIT), Blitz and its Rust dependency tree. Your
HTML, CSS and JavaScript remain application payload rather than part of that native link.

Every dependency's terms still apply. In particular:

- MIT and Apache-2.0 components require their notices to travel with the software.
- Stylo contains MPL-2.0-covered files. Binary distribution requires the corresponding covered
  source to remain available under the MPL terms.

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

## Exports carrying a `.node` addon

A `.node` addon selects the Bun-based host because the standard runtime does not implement
Node-API. That host contains Bun and JavaScriptCore and therefore has additional license and
relinking obligations, including LGPL-family requirements.

The default Blitsen notice flow does not automate that complete obligation set. Before distributing
an addon-based export, arrange an independent licensing review and supply all required notices,
source offers, relink material and terms. The addon's own license and linked dependencies must also
be handled.

## Android APKs

Android currently builds from a source checkout rather than a published platform package, so there
is no package-provided notice file to copy. Generate and audit the Android runtime's `NOTICES.txt`,
then provide it through `BLITSEN_NOTICES_PATH` when building:

```sh
BLITSEN_NOTICES_PATH=/path/to/NOTICES.txt \
  npx blitsen build dist --android --out MyApp.apk
```

Without an audited notice file, the build reports that the APK is not cleared for redistribution.
Signing an APK does not satisfy licensing requirements by itself.

## Distribution checklist

- Run the final artifact with `--licenses` and archive the output with the release.
- Keep required license text and covered source available for the period its terms require.
- Review licenses for application dependencies, fonts, images, media and native addons separately.
- Do not remove notices while signing, packaging or wrapping the Blitsen artifact.
- Obtain qualified legal review for commercial distribution or any addon-based export.

The complete third-party manifest is tied to the exact runtime version and platform. Re-run this
check for every target and every Blitsen upgrade.
