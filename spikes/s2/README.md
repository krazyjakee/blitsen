# S2 — arbitrary external DOM mutation

Status: complete. The bridge path is workable against Blitz commit
`1efe22d2524d71ede5b94592204c21f0de644219`; it does not require a fork or a
full-relayout v0 fallback for the tested mutation surface.

## Result

The harness loads a styled HTML document and drives only Blitz's public Rust
`DocumentMutator` API. It runs the same sequence with incremental layout enabled
and disabled, then requires the complete geometry traces to match.

| Mutation | Before | After |
|---|---:|---:|
| `setAttribute("class", "item wide")` | target width 100 px | 240 px |
| `setAttribute("title", "tall")` with `[title]` CSS | target height 20 px | 60 px |
| `style.height = "80px"` | target height 60 px | 80 px; following sibling y 80 px |
| insert a 30 px node before target | target y 0 px | target y 30 px; sibling y 110 px |
| remove and drop inserted node | target y 30 px | target y 0 px; sibling y 80 px |

Every incremental result matched the forced full-layout control. This confirms
that style traversal and layout invalidation actually visit the dirty subtree;
the result is not being rescued by a hidden full-layout call. Blitz defaults to
incremental layout, propagates dirty-descendant and damage flags, and reconstructs
the affected box tree. Attribute invalidation currently marks a whole subtree,
so it can become more selective later, but the fine-grained machinery is reachable
and correct for this v0 surface.

Programmatic mutations also request a redraw when the `DocumentMutator`
transaction is dropped. The key upstream fixes are already merged in Blitz:

- [#580 — request redraw after programmatic mutations](https://github.com/DioxusLabs/blitz/pull/580)
- [#582 — propagate dirty descendants after inline-style mutation](https://github.com/DioxusLabs/blitz/pull/582)

## Handle model

Blitz stores nodes in a `SlotMap`. `NodeId` packs a 32-bit slot index and a
32-bit version. The harness confirms that a surviving target keeps the same handle
across sibling insertion/removal, while a dropped handle immediately stops
resolving and does not alias a subsequently allocated node.

The bridge should therefore expose an opaque `(document, NodeId)` handle. Calls
must resolve it through `get_node`/`get_node_mut` on every boundary crossing and
return a detached/stale-node error when lookup returns `None`; it must never cache
a raw `Node` pointer. A document identity is required because `NodeId` is only
unique within one document.

## Upstream gap

Changing or clearing the `id` attribute on a live node updates selector-visible
element data but not Blitz's `nodes_to_id` lookup index. The measured trace records
`id_index_consistent: false`. This does not block tree/style/layout mutation, but
it would make a bridge's `getElementById` semantics incorrect.

Filed upstream as
[#677 — DocumentMutator id changes leave get_element_by_id index stale](https://github.com/DioxusLabs/blitz/issues/677).
Blitsen can pin the eventual fix or maintain a small bridge-side ID index until it
lands. No maintained Blitz fork is justified by this result.

## Reproduce

```sh
cargo run --manifest-path spikes/s2/Cargo.toml --release
```

The executable asserts all expected geometry, handle safety, and equality between
incremental and full-layout traces. Its final diagnostic intentionally exposes the
known ID-index result rather than hiding it.
