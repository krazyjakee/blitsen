# 0.1.2 — superseded Intel macOS hardware block

Blitsen 0.1.2 blocked window creation on `MacBookPro14,3` after a second GPU-reset report was
incorrectly attributed to 0.1.1. The command run inside the example workspace had resolved the
local 0.1.0 package and its Vello/Metal renderer instead of the globally installed 0.1.1 package.

0.1.1's CPU renderer was subsequently confirmed working on the affected machine. 0.1.3 restores
that renderer and supersedes this release. The CPU path is safe from Blitsen's Vello/Metal compute
failure, but is substantially slower than GPU rendering.

Published macOS artifacts remain unsigned and are not notarised. See
[`docs/RELEASING.md`](RELEASING.md) for the distribution and signing model.
