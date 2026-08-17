// The site's table of contents, and the rules for turning repo-relative links into
// links that work once the docs are lifted out of GitHub.
//
// Every page here is generated from a file in docs/. Nothing is transcribed: if a
// claim appears on the site it is because it appears in the markdown, so the site
// cannot drift from the repository.

export const REPO = "https://github.com/krazyjakee/blitsen";
export const REPO_BLOB = `${REPO}/blob/main`;

/** The custom domain serves from the root, so the sub-path is empty unless overridden. */
export const BASE = (process.env.SITE_BASE ?? "").replace(/\/$/, "");

export interface DocPage {
  /** Filename inside docs/. */
  file: string;
  /** URL segment under /docs/. */
  slug: string;
  /** Short nav label. */
  nav: string;
  /** Full page title. */
  title: string;
  /** One line, written from the document's own opening — used in nav and meta tags. */
  blurb: string;
}

export interface DocGroup {
  name: string;
  note: string;
  pages: DocPage[];
}

export const GROUPS: DocGroup[] = [
  {
    name: "Specification",
    note: "What Blitsen is meant to be, and how it is built.",
    pages: [
      {
        file: "PRODUCT.md",
        slug: "product",
        nav: "Product",
        title: "Product specification",
        blurb: "The problem, the positioning, the principles, and the size budget treated as a product commitment.",
      },
      {
        file: "TECH.md",
        slug: "tech",
        nav: "Technical",
        title: "Technical specification",
        blurb: "Architecture, the host phase reversal, threading, the DOM–JS bridge, the frame pipeline and the export path.",
      },
      {
        file: "MODULES.md",
        slug: "modules",
        nav: "Module resolution",
        title: "Module resolution in the shipped binary",
        blurb: "How an import is resolved once there is no Node and no bundler left in the picture.",
      },
    ],
  },
  {
    name: "The boundary",
    note: "Blitsen renders less of the web than a browser does. This is where that line is drawn.",
    pages: [
      {
        file: "COMPATIBILITY.md",
        slug: "compatibility",
        nav: "Compatibility profile",
        title: "v1 compatibility profile",
        blurb: "The accepted surface, generated from the runtime rather than hand-maintained, with capability tiers and diagnostic severities.",
      },
      {
        file: "CONFORMANCE.md",
        slug: "conformance",
        nav: "Layout conformance",
        title: "Layout conformance corpus",
        blurb: "The corpus behind requirement P6 — that an application lays out the same way on every platform.",
      },
      {
        file: "BLITZ-GAPS.md",
        slug: "blitz-gaps",
        nav: "Blitz gaps",
        title: "Blitz rendering gaps — standing list",
        blurb: "Standing list of what the pinned Blitz revision does not yet render, each reported upstream with a reproduction.",
      },
    ],
  },
  {
    name: "Using it",
    note: "Getting an application in, and an executable out.",
    pages: [
      {
        file: "GETTING-STARTED.md",
        slug: "getting-started",
        nav: "Run and export",
        title: "Run and export an app",
        blurb: "Install Blitsen, check a static build against the compatibility profile, and export a native executable.",
      },
      {
        file: "MIGRATION.md",
        slug: "migration",
        nav: "Migration",
        title: "Migrating to the Phase 2 runtime",
        blurb: "Nothing changes and your application gets smaller — unless it carries a .node addon.",
      },
      {
        file: "LICENSING.md",
        slug: "licensing",
        nav: "Licensing",
        title: "Licensing Blitsen and exported applications",
        blurb: "What an exported application owes, what it carries, and why closed-source applications are supported.",
      },
      {
        file: "RELEASING.md",
        slug: "releasing",
        nav: "Releasing",
        title: "Releasing",
        blurb: "Six prebuilt runtimes and one JavaScript package, published together.",
      },
      {
        file: "RELEASE-NOTES-0.1.0.md",
        slug: "release-notes-0-1-0",
        nav: "0.1.0 notes",
        title: "0.1.0 — first cross-platform release",
        blurb: "Draft notes for the first release that publishes prebuilt runtimes.",
      },
    ],
  },
  {
    name: "Decisions and evidence",
    note: "Milestones are declared on measurements. These are the measurements.",
    pages: [
      {
        file: "M0.md",
        slug: "m0",
        nav: "M0 — feasibility",
        title: "M0 — feasibility decision",
        blurb: "The measurement that withdrew the original 25–50 MB size target rather than restating it.",
      },
      {
        file: "M2.md",
        slug: "m2",
        nav: "M2 — interactive",
        title: "M2 — interactive acceptance",
        blurb: "Input, animation and restyle proven together through the window's own hit test.",
      },
      {
        file: "M3.md",
        slug: "m3",
        nav: "M3 — Pong",
        title: "M3 — Pong architecture proof",
        blurb: "Three files, one executable, 0.809 ms median frame cost against a 16.7 ms budget.",
      },
      {
        file: "M3B.md",
        slug: "m3b",
        nav: "M3b — adoption",
        title: "M3b — compatible adoption proof",
        blurb: "Six applications written by other people, rendered from their own unmodified build output.",
      },
      {
        file: "JSC.md",
        slug: "jsc",
        nav: "Engine choice",
        title: "Phase 2 JavaScriptCore acquisition",
        blurb: "The engine decision, and the spike that superseded it in favour of QuickJS-ng.",
      },
    ],
  },
];

export const ALL_PAGES: DocPage[] = GROUPS.flatMap((g) => g.pages);

const BY_FILE = new Map(ALL_PAGES.map((p) => [p.file, p]));

/**
 * Rewrites a link found inside a markdown doc so it resolves on the built site.
 *
 * Three cases: a sibling doc becomes a site page; anything else in the repo becomes
 * a GitHub blob link, because the site ships documentation and not the source tree;
 * an image is served from the site's own asset directory.
 */
export function rewriteDocLink(href: string): string {
  if (/^(https?:|mailto:|#)/.test(href)) return href;

  const [pathPart, hash] = splitHash(href);
  const clean = pathPart.replace(/^\.\//, "").replace(/^docs\//, "");

  if (/\.(png|gif|jpe?g|svg|webp)$/i.test(clean)) {
    return `${BASE}/assets/${clean.split("/").pop()}`;
  }

  // ../spikes/s8/README.md and similar — outside docs/, so send it to the repo.
  if (clean.startsWith("../")) {
    return `${REPO_BLOB}/${clean.replace(/^\.\.\//, "")}${hash}`;
  }

  const page = BY_FILE.get(clean);
  if (page) return `${BASE}/docs/${page.slug}/${hash}`;

  if (clean.endsWith(".md")) return `${REPO_BLOB}/docs/${clean}${hash}`;

  // A path into the source tree: examples/, packages/, crates/, spikes/.
  return `${REPO_BLOB}/${clean}${hash}`;
}

/** Same rules, but for links written from the repo root (the README's frame). */
export function rewriteRootLink(href: string): string {
  if (/^(https?:|mailto:|#)/.test(href)) return href;
  const [pathPart, hash] = splitHash(href);
  if (/\.(png|gif|jpe?g|svg|webp)$/i.test(pathPart)) {
    return `${BASE}/assets/${pathPart.split("/").pop()}`;
  }
  if (pathPart.startsWith("docs/")) {
    const page = BY_FILE.get(pathPart.slice("docs/".length));
    if (page) return `${BASE}/docs/${page.slug}/${hash}`;
  }
  return `${REPO_BLOB}/${pathPart}${hash}`;
}

function splitHash(href: string): [string, string] {
  const at = href.indexOf("#");
  return at === -1 ? [href, ""] : [href.slice(0, at), href.slice(at)];
}
