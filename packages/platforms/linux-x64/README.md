# @blitsen/linux-x64

The prebuilt Blitsen native runtime for `linux-x64`: the `blitsen.node` addon used by
`blitsen run` and addon-carrying exports, plus the `blitsen-runtime` executable used by
ordinary standalone exports.

The binaries target glibc 2.35 (the Ubuntu 22.04 baseline) and dynamically link ALSA,
OpenSSL 3 and fontconfig: `libasound.so.2`, `libssl.so.3`, `libcrypto.so.3` and
`libfontconfig.so.1` must be available on the host.

Do not install this package directly. It is an `optionalDependency` of
[`blitsen`](https://www.npmjs.com/package/blitsen), whose `os` and `cpu` fields make
your package manager install only the one matching your machine, and its version is
pinned to `blitsen`'s exactly.

Blitsen is an independent project built on Blitz. It is not an official DioxusLabs
project and is not endorsed by DioxusLabs.
