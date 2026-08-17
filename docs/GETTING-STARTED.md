# Run and export an app

Install Blitsen as a development dependency, run your existing web output in a native window,
then export it as an executable. Blitsen consumes static HTML, CSS and JavaScript; it does not
replace your framework or build tool.

Blitsen is pre-alpha. Check the [compatibility profile](COMPATIBILITY.md) before treating a browser
build as a supported Blitsen application. The 0.1.0 runtimes and the executables built from them
are unsigned.

## Install Blitsen

```sh
npm install -D blitsen
```

The package manager installs the CLI and the runtime for your current desktop platform. There is
no post-install compilation step or Rust toolchain requirement. Linux, macOS and Windows are
available on x64 and arm64; see the [0.1.0 release notes](RELEASE-NOTES-0.1.0.md) for the tested
tier and operating-system requirements of each target.

## Run static output

Point Blitsen at a directory containing `index.html`:

```sh
npx blitsen dist
```

For an application with no build step, use its source directory instead:

```sh
npx blitsen .
```

To use an existing development server and keep its hot reload connection, pass its URL:

```sh
npx blitsen http://localhost:5173
```

## Check compatibility

Run `doctor` against the static output, not the source directory or development-server URL:

```sh
npx blitsen doctor dist
```

Compatibility errors fail an export. Warnings identify behaviour that needs review but do not stop
the build. The report follows the same capability profile published in this repository.

## Export an executable

Build directly from a static directory:

```sh
npx blitsen build dist --out MyApp
```

The default embedded-assets mode produces one executable containing the runtime and application.
Run that artifact on the target platform to verify the native window, input and application state.

For an existing project, put the build command and output directory in the `blitsen` key of
`package.json`:

```json
{
  "scripts": {
    "native": "blitsen build"
  },
  "blitsen": {
    "build": "vite build",
    "output": "dist",
    "name": "My App"
  }
}
```

Then build the web output and native executable in one step:

```sh
npm run native
```

Blitsen runs the configured command from the directory containing `package.json`, ingests
`output`, and uses `name` for the window title and default artifact name. A directory passed on the
command line skips this configured build step.

## Build for another desktop target

Pass one of the six desktop target triples:

```sh
npx blitsen build dist --target win32-x64 --out MyApp.exe
```

Blitsen downloads and caches that target's runtime. Cross-building can create the files for another
platform, but signing and notarisation still require that platform or an external signing service.
Read [Licensing Blitsen and exported applications](LICENSING.md) before distributing an
application.
