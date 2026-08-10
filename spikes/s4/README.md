# S4 — HTML fragment parsing

This spike tests whether the pinned Blitz revision can implement `innerHTML` by
parsing fragments against a live context element, adopting the result into the
document, and invalidating layout correctly.

## Run

```sh
cargo run --manifest-path spikes/s4/Cargo.toml --release
```

The harness runs the same mutations with incremental layout enabled and
disabled and requires identical traces.

## Result

Fragment parsing is reachable through `DocumentMutator::set_inner_html` when
`blitz_html::HtmlProvider` is explicitly installed as
`DocumentConfig::html_parser_provider`. The default provider is a silent no-op,
so Blitsen must set this provider when constructing documents.

The pinned revision successfully:

- replaces a live element's children and drops stale handles and ID-map entries;
- adopts new IDs into the live document;
- applies style and layout invalidation immediately;
- uses the correct HTML5 context for table and select fragments; and
- produces the same layout with incremental and full resolution.

The measured layout after replacing the host contents was a 240×30 first span,
a second span at y=30, and a 50px-tall host.

## Upstream defect

Repeated replacement revealed one retained detached node per call. With an
equal-size two-child replacement, `document.tree().len()` was:

```text
27, 28, 29, 30, 31, 32
```

The parser removes the temporary fragment root after moving its children, but
does not drop it. This is tracked as
[DioxusLabs/blitz#678](https://github.com/DioxusLabs/blitz/issues/678).

## Decision

Use Blitz's native fragment parser; no fallback parser is needed. Blitsen must
install `HtmlProvider`, and should carry or await the small fragment-root cleanup
fix before relying on repeated `innerHTML` mutations in production.
