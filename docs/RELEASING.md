# Releasing

Six prebuilt runtimes and one JavaScript package, published together. This is what
`.github/workflows/release.yml` does, what it deliberately does not do, and what has to exist
before it can do any of it.

## The shape of a release

`blitsen` is thin JavaScript. The runtime is one native addon per target, published as
`@blitsen/<target>` and pinned to `blitsen`'s version **exactly** — the two halves are one ABI, so
a range would allow a pair that was never built together (TECH.md §11, issue #73).

That pin is what makes the ordering matter. `blitsen`'s `optionalDependencies` name exact versions,
so the six platform packages publish **first** and `blitsen` **last**: its own publish is what makes
the release visible to a user at all, and a half-published set would resolve to nothing installable.
npm has no transaction, so ordering is the whole of the guarantee.

## Running it

Manual dispatch only, and a dry run unless asked otherwise:

```
Actions → Release → Run workflow
  publish: false   # pack and npm publish --dry-run
  tag: latest
```

A dry run is worth doing on its own. It builds all six runtimes, runs the package tests against
each freshly built addon, generates each target's notices, stages, packs, asks npm what every
tarball would contain, and stops short of the registry.

### The tag comes last, and does not trigger anything

A real publish ends by tagging the commit it built and opening a GitHub Release from
`docs/RELEASE-NOTES-<version>.md`, or from generated notes if that file is missing. A dispatch
under a dist-tag other than `latest` marks the release a prerelease, which is the same distinction
npm draws.

**Pushing a tag does not publish.** The tag is written *after* the registry has accepted all seven
packages, so it records a release rather than claiming one — it cannot name a version that failed
to publish, and it points at the commit the run built rather than wherever the branch has moved
since. The reverse arrangement, where `git push --tags` starts a release, makes a typo
irreversible against a registry whose undo is a 72-hour window.

Be precise about what a clean dry run does **not** prove. With no certificates in the repository
the two signing steps do not sign: the Windows step locates `signtool.exe` and stops, and the
macOS step ad-hoc-signs a copy in the runner's temp directory to show `codesign` accepts an
artifact of that shape. The keychain import, a real `sign`, and both `verify` calls against a
genuine certificate stay unexercised until secrets exist — which is why "the signing step was
skipped" is not the same as "signing works" (issue \#132).

Each platform package carries two artifacts, built, signed and published together: `blitsen.node`,
the addon `blitsen run` loads, and `blitsen-runtime` (`.exe` on Windows), the executable an export
links into. A package with one and not the other installs cleanly and then fails at whichever
command needs the missing half, so the staging step treats an absent file as an error rather than a
warning.

There is no third artifact. The JavaScript engine is statically linked into `blitsen-runtime`
(`LICENSING.md`), so there is no engine library to build, pin, checksum, sign or ship — which is
most of what the release process was expected to carry when `JSC.md` was written.

## What it does not do: notarisation

macOS code **signing** is in the workflow, because it is local and fast. **Notarisation is not.**

Notarisation is a submission to Apple that takes minutes to hours and needs an App Store Connect
key. Running it inline would make release wall-clock unpredictable, and it applies to a distributed
application bundle rather than to a `.node` addon inside an npm package — which is not a thing
Gatekeeper ever sees.

Where it does matter is an application a user exports with `blitsen build`. That artifact is theirs
to sign and notarise, on a macOS host, and `--sign` is the seam for it. A cross-built macOS app that
is never signed and notarised is refused by Gatekeeper on any machine that did not build it; see the
cross-target section of the package README.

There is no credential hidden behind that statement. Completing notarisation needs an Apple
Developer Program team, a Developer ID Application certificate for the final `.app`, and
credentials accepted by `notarytool` (an App Store Connect issuer/key pair or an Apple ID app
password), supplied by the account holder. The final application must be submitted and the ticket
stapled after signing. This repository has none of those account materials and the release workflow
does not claim to perform those steps. Windows distribution signing likewise still needs a real
code-signing certificate supplied as `WINDOWS_CERTIFICATE_PFX` and its password; absent it, the
workflow records that the PE files are unsigned.

## Prerequisites

None of these are in the repository, and the workflow runs without them — every signing step is
skipped with a `::warning::` naming what was missing, so an unsigned dry run still exercises the
whole path.

| Secret | Used for | Absent → |
| --- | --- | --- |
| `NPM_TOKEN` | Publishing | `publish: true` fails loudly; a dry run is unaffected |
| `APPLE_CERTIFICATE_P12` | macOS signing, base64 of the `.p12` | Unsigned macOS runtimes |
| `APPLE_CERTIFICATE_PASSWORD` | Its passphrase | — |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: … (TEAMID)` | Unsigned macOS runtimes |
| `WINDOWS_CERTIFICATE_PFX` | Windows signing, base64 of the `.pfx` | Unsigned Windows runtimes |
| `WINDOWS_CERTIFICATE_PASSWORD` | Its passphrase | — |

### Published runtimes are unsigned, and say so

No platform runtime is signed. No Apple Developer ID and no Windows code-signing
certificate exist for this project, and the workflow's signing steps are no-ops that warn per
target (issue \#132). Two things follow, and the release notes have to carry both:

- What npm delivers is a `.node` addon and an executable inside a package. Gatekeeper does not
  inspect an addon a package manager downloaded, and SmartScreen's reputation check is about what
  a user downloads and launches — so an unsigned runtime is mostly invisible at install time.
- What a user's users see is different. `blitsen build` produces an executable launched by name,
  which is exactly what an OS gatekeeper checks. That artifact is the application author's to
  sign, `--sign` is the seam for it, and the package README says so.

Revisit at the first release anyone but its author installs. Signing needs a Developer ID
Application certificate and a Windows certificate, added as the secrets below; notarisation stays
out of scope and is tracked in \#71.

### The npm scope

`blitsen` is published from a personal account; the six runtimes are `@blitsen/*` and a scoped name
needs the **scope** to exist and to be owned by the account whose token CI uses. Create the
`blitsen` organisation on npm (a free org covers unlimited public packages), confirm it is owned by
that account, and only then run a real publish — the workflow publishes all six platform packages
before `blitsen` itself, so a scope that refuses them fails the release halfway through (issue
\#131).

### The runner labels, and which of them move

GitHub's free `ubuntu-22.04-arm` and `windows-11-arm` runners are **public repositories only**.
This repository is public, so both labels schedule for it and the workflow defaults are the ones
that run.

The override survives for the case that stops being true. Point those targets at runners the
repository can use, with repository variables:

| Variable | Default |
| --- | --- |
| `LINUX_ARM64_RUNNER` | `ubuntu-22.04-arm` |
| `WIN32_ARM64_RUNNER` | `windows-11-arm` |
| `DARWIN_X64_RUNNER` | `macos-15-intel` |

The third is there for a different reason: the Intel macOS image is the one that keeps moving.
`macos-13` queued for 40 minutes without picking up a runner across two dispatches, while every
other label started inside a minute — so `darwin-x64` names the current Intel image and the
variable is how it moves again without a commit. `ci.yml` reads the same three variables for its
smoke jobs; its Linux smoke default may be newer, while the release default stays on the
compatibility floor below.

Both Linux release jobs deliberately use Ubuntu 22.04 (`ubuntu-22.04` on x64 and
`ubuntu-22.04-arm` on arm64). Its glibc 2.35 is the binary compatibility floor. After building,
the workflow reads the version requirements from both `blitsen.node` and `blitsen-runtime` and
fails if either requires anything newer than `GLIBC_2.35`; each job summary records the highest
version it found. The gate remains necessary even with the runner pinned, because a toolchain can
introduce a newer symbol requirement without changing the runner label.

The Windows x64 release job is pinned to `windows-2022`, rather than following
`windows-latest` across Visual Studio toolsets. Both Windows targets build with Rust's
`+crt-static`, and the workflow inspects both PE import tables to reject a remaining
`VCRUNTIME`, `MSVCP`, UCRT or `api-ms-win-crt` dependency. The package therefore does not require
a separately installed Visual C++ Redistributable.

## Why six native runners rather than cross-compilation

Open question 11 asks whether the matrix can be cut down. The answer *was* no, and the reason was
recorded in JSC.md: the engine's artifacts are built and tested natively per OS, and WebKit
publishes no cross-platform binary release.

**That reason is gone.** QuickJS-ng is portable C compiled by the ordinary Rust build, so the engine
no longer forces native builders. What remains is Blitz and its platform layer, which is a different
and unmeasured question — and the standing argument still holds on its own terms: a runtime that was
never executed on its own platform is not evidence that it works there. Re-open the question with a
measurement, not with the engine's old constraint.

What the choice costs is **measured rather than argued**. Each build job records its own wall clock
in the job summary. The four most recent successful releases available when this was reviewed were
[0.1.3 dry run](https://github.com/krazyjakee/blitsen/actions/runs/32075145025),
[0.1.3 publish](https://github.com/krazyjakee/blitsen/actions/runs/32076477729),
[0.2.0](https://github.com/krazyjakee/blitsen/actions/runs/32323533439), and
[0.2.1](https://github.com/krazyjakee/blitsen/actions/runs/32506681508). Their native build jobs,
including setup, tests, packing and upload, measured:

| Target | Median job time | Observed range |
| --- | ---: | ---: |
| `linux-x64` | 5m 29s | 1m 51s–9m 10s |
| `linux-arm64` | 5m 00s | 2m 24s–6m 19s |
| `darwin-x64` | 8m 19s | 3m 55s–10m 23s |
| `darwin-arm64` | 3m 41s | 2m 02s–8m 31s |
| `win32-x64` | 13m 11s | 7m 20s–15m 02s |
| `win32-arm64` | 10m 29s | 6m 12s–11m 53s |

The six jobs consumed 23m 44s–61m 18s of aggregate runner time per release. Parallelism kept the
whole workflow, including publication, between 9m 35s and 17m 47s. These are elapsed runner
minutes, not a dollar estimate: runner pricing and public-repository allowances are account policy,
not properties of this build.

The decision is therefore to **retain six native builds**. Cross-compiling QuickJS-ng is feasible,
but it does not remove the native jobs that load each addon, exercise the platform package, inspect
the host object format and run `codesign` or `signtool`; it would add target SDK/linker maintenance
while the measured release stays below eighteen minutes. Revisit if several releases exceed thirty
minutes wall-clock, paid native-runner consumption becomes material, or native validation can be
split into a demonstrably cheaper smoke job. At that point compare the complete cross-build plus
native-validation pipeline, not compiler time alone.

## Reproducibility boundary

Reproducible means the two **unsigned Cargo release outputs** — `blitsen.node` and
`blitsen-runtime` (`.exe` on Windows) — are byte-identical for the same commit, `Cargo.lock`, pinned
Rust version, runner image and native SDK/system-library floor. The check happens immediately after
Cargo and before either signing step. Apple secure timestamps and Windows RFC 3161 timestamps are
supposed to differ between invocations, so signed files are outside the boundary. npm tarballs and
GitHub's artifact zip are packaging envelopes and are outside it too; their contents still come
from the checked native bytes.

Every matrix row prints the unsigned size and SHA-256 of both artifacts. To keep the gate useful
without doubling all six builds, `linux-x64`, `darwin-x64` and `win32-x64` each compile once in the
shipping checkout and once in a second clean source and target directory, with compiler caching
disabled. That is one pinned native runner per executable format and OS; the arm64 sibling uses the
same source and platform linker family and retains its recorded hashes. A mismatch reports both
hashes and sizes, the first differing byte and surrounding bytes.

The source roots differ deliberately. `--remap-path-prefix` maps Rust paths to `/src/blitsen`, and
the native compiler gets the equivalent `-ffile-prefix-map` or MSVC `/pathmap`, because QuickJS-ng's
C `__FILE__` strings otherwise retain its Cargo output directory after symbols are stripped.
`SOURCE_DATE_EPOCH` is the commit timestamp for native build scripts that observe the standard.
Windows additionally uses MSVC `/Brepro`, alongside the existing static CRT flag, because PE linker
metadata otherwise owns a build timestamp. No post-build normalisation is allowed: a passing
comparison is evidence about the files that proceed to signing, while a failing one preserves the
difference for diagnosis.

## What CI covers, and what only a release build touches

`ci.yml` runs the full suite on `linux-x64`, `darwin-arm64` and `win32-x64`, and a smoke tier on the
other three published targets — build both artifacts, package tests against them, the native
acceptance harness, a standalone export, the layout corpus and a report-only size measurement
(issue \#133).

Android is a thinner tier again, because it is not one of the six and `release.yml` does not build
it: the `android` job cross-compiles `blitsen-android` for `arm64-v8a` and `x86_64`, checks each
`.so` is the architecture it claims and exports `android_main`, resolves the notices an APK owes,
and then packages one with `blitsen build --android` and reads the archive back (issue \#149).
That job stops before the emulator — when it was written it stopped before packaging too, and that
was because an APK carrying the engine could not be built at all until \#148.

A device now runs one thing. The `android-notifications` job takes the APKs that job packages and
boots an AVD on API 32 and API 33 with `reactivecircus/android-emulator-runner`, then runs
`bun run --cwd packages/blitsen test:android-notify -- --apk <path> --package <id>`: it answers the
runtime permission dialog, and reads delivery, same-ID replacement, timeout and close back out of
`dumpsys notification` (issue \#254). It is a separate job because an emulator boot is the flakiest
thing in the file and a cross-compile gate should not fail for one. The framebuffer smoke test
(`bun run --cwd packages/blitsen test:android --apk <path> --package <id>`) is still not wired to a
device: it and the notification job meet the same two open questions about hosted runners —
lavapipe under the emulator's Vulkan, and KVM on a standard runner — and running one of them first
is how those get an answer that is not two failures at once.

What no CI job covers on any target is the release path itself: staging, signing,
packing and publish ordering. That is what a `publish: false` dispatch is for, and it is the only
evidence those steps have.

## Before the first real release

- [x] Decide the repository's visibility, or set the runner variables — public, defaults
- [x] Decide whether releases ship signed, or unsigned and say so — unsigned, and they say so
- [x] Run once with `publish: false` and read the six job summaries (\#134) — four clean runs
- [x] Confirm `blitsen` and all six `@blitsen/*` manifests carry the same version — asserted by
      the package tests, and again by the publish job before it publishes anything
- [x] Rehearse the install against a local registry — see below; it found two release blockers
- [x] Create the `blitsen` npm organisation and confirm who owns it (\#131) — created on the free
      plan, one member: `krazyjakee`, owner, the same account that owns the package `blitsen`
- [x] Merge to `main` — npm provenance records the ref it published from (\#130, 2026-08-15)
- [x] Add `NPM_TOKEN` (\#132) — granular, `blitsen` and `@blitsen` read and write, no organisation
      access, 2FA bypassed so CI is never asked for a one-time password, expires 2026-11-13
- [ ] **Dispatch Release from `main` with `publish: true`**
- [ ] Install `blitsen` from the registry on a machine that has never built it

The publish job asks `npm whoami` first, so a token that does not authenticate fails the run
instead of the first publish. It does **not** gate on `npm access list packages @blitsen`: that
reads the *organisation*, which is a different permission from writing to the scope, and a token
scoped to publish and nothing else is refused by it with a 403. The check would fail the token that
works and pass one that does not, so it is a log note now. What protects a half-published release
is the ordering — a scope that refuses this token refuses the first platform package, with nothing
published and no version burned.

**The token expires 2026-11-13.** A release attempted after that fails at `npm whoami` with a
message naming it, which is the cheapest place for it to fail.

### Rehearsing the install without the registry

The last box is the only real check, and most of it can be had before the scope exists — which is
where the two worst first-release faults were found. Run a local registry, publish all seven packages
to it in release order, and install from it into an empty project:

```sh
npx verdaccio --config <config with max_body_size: 500mb> --listen 4873
npm config set //localhost:4873/:_authToken rehearsal
for t in darwin-arm64 darwin-x64 linux-arm64 linux-x64 win32-arm64 win32-x64; do
  npm publish --registry http://localhost:4873/ --access public ./packages/platforms/$t
done
npm publish --registry http://localhost:4873/ --access public ./packages/blitsen
mkdir /tmp/consumer && cd /tmp/consumer && npm init -y
npm i -D blitsen --registry http://localhost:4873/
npx blitsen build ./app --out MyApp        # under Node, which is what npx starts
BLITSEN_STANDALONE_CHECK=1 ./MyApp
```

What that catches, and CI does not: `npx` runs **Node**, not Bun; only the host's platform package
installs, so `os`/`cpu` are exercised; the executable bit has to survive a real `npm pack`; and a
`--target` build fetches another platform's package from a registry rather than from a seeded
cache.
