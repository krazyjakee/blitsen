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
each freshly built addon, packs every package and stops short of the registry — which is the only
way the signing and staging steps get exercised before a real release depends on them.

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

### arm64 runners, and this repository

GitHub's free `ubuntu-24.04-arm` and `windows-11-arm` runners are **public repositories only**.
This repository is private, so those two jobs will not schedule as written.

Either make the repository public, or point the two arm64 targets at runners this repository can
use, with repository variables:

| Variable | Default |
| --- | --- |
| `LINUX_ARM64_RUNNER` | `ubuntu-24.04-arm` |
| `WIN32_ARM64_RUNNER` | `windows-11-arm` |

Set them to larger-runner or self-hosted labels. The defaults are left as the free labels so the
matrix becomes correct the moment the repository is public.

## Why six native runners rather than cross-compilation

Open question 11 asks whether the matrix can be cut down. The answer so far is no, and the reason is
recorded in JSC.md: the engine's artifacts are built and tested natively per OS, and WebKit
publishes no cross-platform binary release. A runtime that was never executed on its own platform is
not evidence that it works there.

What the choice costs is **measured rather than argued**. Each build job records its own wall clock
in the job summary:

| Target | Runner | Wall clock |
| --- | --- | --- |

Note that macOS runners bill at a multiplier, so wall clock is not the same as cost; multiply before
comparing. Revisit cross-compilation when there are numbers from a few real releases to argue with.

## Before the first real release

- [ ] Decide the repository's visibility, or set the two arm64 runner variables
- [ ] Add `NPM_TOKEN`, and the signing secrets for whichever platforms are to be signed
- [ ] Run once with `publish: false` and read the six job summaries
- [ ] Confirm `blitsen` and all six `@blitsen/*` manifests carry the same version
- [ ] Publish, then install `blitsen` from the registry on a machine that has never built it

The last one is the only real check: everything before it tests the workflow, and only that tests
the release.
