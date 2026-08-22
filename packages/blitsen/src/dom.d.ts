// `blitsen/dom`: the web surface where it differs from `lib.dom.d.ts`.
//
// Almost all of Blitsen's DOM is the DOM, and `lib.dom.d.ts` already describes
// it correctly — redeclaring it here would only create a second version to go
// stale. What this file carries is the part TypeScript cannot know:
//
//  - `<blitsen-view>`, which is Blitsen's own element and is in no browser lib.
//  - The tag-name maps and JSX namespaces, so `document.createElement` and a
//    framework's markup both type it as itself rather than as `HTMLElement`.
//
// What it deliberately does *not* do is redeclare the absent APIs as `never`.
// `lib.dom.d.ts` will still offer `IndexedDB` and `OffscreenCanvas`, because
// removing a global from an ambient lib is not something a package can do. The
// list of what is genuinely absent is generated from the runtime — see
// `src/api-manifest.json` and the capability tiers in COMPATIBILITY.md — and
// `blitsen doctor` is what checks a build against it. Types cannot replace that
// check, so this file does not pretend to.
//
// Reference it once, anywhere in the project:
//
// ```ts
// /// <reference types="blitsen/dom" />
// ```
//
// or add `"blitsen/dom"` to `compilerOptions.types` — the recommended
// `tsconfig` fragment at `blitsen/tsconfig.json` already does.

/**
 * Pixels handed to a `<blitsen-view>`, in the layout the surface reports.
 *
 * A surface is live for as long as it is acquired: `width`, `height` and
 * `devicePixelRatio` are read back each time, because the window can resize
 * under it. `generation` changes when the backing store is replaced, which is
 * what a `resize` event on the element announces.
 */
export interface BlitsenViewSurface {
  /** Width in physical pixels — what must be filled, not the CSS box. */
  readonly width: number;
  /** Height in physical pixels. */
  readonly height: number;
  /** Physical pixels per CSS pixel for this surface. */
  readonly devicePixelRatio: number;
  /** Changes when the backing store is replaced; a `resize` event follows. */
  readonly generation: number;
  /** Bytes one full frame occupies: `width * height * 4`. */
  readonly byteLength: number;
  /** Uploads one frame of 8-bit RGBA, row-major, `byteLength` bytes. */
  write(pixels: ArrayBufferView): void;
  /** Releases the claim; the element may then be acquired again. */
  release(): void;
}

/**
 * `<blitsen-view>`: a rectangle the application paints itself.
 *
 * Laid out and composited as an ordinary element — the DOM draws over and under
 * it — but its interior is uploaded pixels rather than a rendered subtree.
 * Acquiring it twice without releasing throws `InvalidStateError`.
 */
export interface BlitsenViewElement extends HTMLElement {
  /** Claims the surface. Listen for `resize` on the element to learn it changed. */
  acquireSurface(): BlitsenViewSurface;
}

declare global {
  // eslint-disable-next-line vars-on-top, no-var
  var BlitsenViewElement: { prototype: BlitsenViewElement; new (): BlitsenViewElement };
  // eslint-disable-next-line vars-on-top, no-var
  var BlitsenViewSurface: { prototype: BlitsenViewSurface };

  interface HTMLElementTagNameMap {
    "blitsen-view": BlitsenViewElement;
  }

  // `createElement("blitsen-view")` resolves through the map above; this is the
  // deprecated-but-still-consulted twin that some framework typings read.
  interface HTMLElementDeprecatedTagNameMap {
    "blitsen-view": BlitsenViewElement;
  }
}

// JSX, for the frameworks that route unknown tags through their own namespace
// rather than through `HTMLElementTagNameMap`. Declared loosely on purpose: the
// attribute set is whatever the framework's own element props are, and pinning
// it to one framework's shape would be wrong in the other two.
declare global {
  namespace JSX {
    interface IntrinsicElements {
      "blitsen-view": Record<string, unknown>;
    }
  }
}

export {};
