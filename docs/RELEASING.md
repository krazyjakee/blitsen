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

### 0.1.0 ships unsigned, and says so

**Decided for 0.1.0: no platform is signed.** No Apple Developer ID and no Windows code-signing
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

### arm64 runners, and this repository

GitHub's free `ubuntu-24.04-arm` and `windows-11-arm` runners are **public repositories only**.
This repository is public, so both labels schedule for it and the workflow defaults are the ones
that run.

The override survives for the case that stops being true. Point the two arm64 targets at runners
the repository can use, with repository variables:

| Variable | Default |
| --- | --- |
| `LINUX_ARM64_RUNNER` | `ubuntu-24.04-arm` |
| `WIN32_ARM64_RUNNER` | `windows-11-arm` |

Set them to larger-runner or self-hosted labels. `ci.yml` reads the same two variables for its
smoke jobs, so both files follow one decision.

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
in the job summary:

| Target | Runner | Wall clock |
| --- | --- | --- |

Note that macOS runners bill at a multiplier, so wall clock is not the same as cost; multiply before
comparing. Revisit cross-compilation when there are numbers from a few real releases to argue with.

## What CI covers, and what only a release build touches

`ci.yml` runs the full suite on `linux-x64`, `darwin-arm64` and `win32-x64`, and a smoke tier on the
other three published targets — build both artifacts, package tests against them, the native
acceptance harness, a standalone export, the layout corpus and a report-only size measurement
(issue \#133). What no CI job covers on any target is the release path itself: staging, signing,
packing and publish ordering. That is what a `publish: false` dispatch is for, and it is the only
evidence those steps have.

## Before the first real release

- [x] Decide the repository's visibility, or set the two arm64 runner variables — public, defaults
- [ ] Create the `blitsen` npm organisation and confirm who owns it (\#131)
- [ ] Add `NPM_TOKEN`, and the signing secrets for whichever platforms are to be signed (\#132)
- [ ] Decide whether 0.1.0 ships signed, or ships unsigned and says so in the release notes
- [ ] Run once with `publish: false` and read the six job summaries (\#134)
- [ ] Confirm `blitsen` and all six `@blitsen/*` manifests carry the same version
- [ ] Publish, then install `blitsen` from the registry on a machine that has never built it

The last one is the only real check: everything before it tests the workflow, and only that tests
the release.
