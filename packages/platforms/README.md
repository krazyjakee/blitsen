# Platform runtime packages

One package per target from TECH.md §11, each shipping a single `blitsen.node` addon.
`blitsen` lists all six as `optionalDependencies` pinned to its own version exactly;
the `os` and `cpu` fields here are what make npm, pnpm, yarn and Bun install only the
host's binary. `packages/blitsen/src/runtime.mjs` resolves the installed one.

**None of these is published yet.** What is committed here is the manifest and nothing
else: each package's `blitsen.node`, its `blitsen-runtime` executable and the
`NOTICES.txt`/`NOTICES.json` pair are produced on a matching runner and staged into the
directory at release time — see `.github/workflows/release.yml`, which is manual-dispatch
only and defaults to packing rather than publishing. All six targets have a runner now:
the release workflow builds every one natively, and `ci.yml` exercises three of them in
full and the other three as a smoke tier on every push (issue #133).

Because the names are declared but unpublished, every install in this repository
prints six registry 404 warnings and then succeeds: that is what an *optional*
dependency does when it cannot be fetched, and it leaves the same empty
`node_modules/@blitsen` a host with no matching package will have once they ship.
`bun install --frozen-lockfile` is unaffected — verified, since CI runs it.

Verifying the install itself on npm, pnpm, yarn and Bun needs published packages, so
that is still outstanding. What is verified today is narrower and worth stating as
such: resolution finds an installed platform package through node's own resolver,
refuses one whose version is not the CLI's, and refuses one that carries no addon
(`packages/blitsen/test/cli.test.mjs`).

These directories are deliberately *not* workspace members: `packages/*` would make
them one, and adding six unpublished names to the root lockfile breaks
`bun install --frozen-lockfile`. Nesting them a level down keeps the repository's own
install untouched while the published layout stays exactly as documented.
