# @blitsen/darwin-x64

The prebuilt Blitsen native runtime for `darwin-x64`: the `blitsen.node` addon used by
`blitsen run` and addon-carrying exports, plus the `blitsen-runtime` executable used by
ordinary standalone exports.

Do not install this package directly. It is an `optionalDependency` of
[`blitsen`](https://www.npmjs.com/package/blitsen), whose `os` and `cpu` fields make
your package manager install only the one matching your machine, and its version is
pinned to `blitsen`'s exactly.

Blitsen is an independent project built on Blitz. It is not an official DioxusLabs
project and is not endorsed by DioxusLabs.
