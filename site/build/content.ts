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
    name: "Start here",
    note: "Get an application running, then learn the runtime model.",
    pages: [
      {
        file: "GETTING-STARTED.md",
        slug: "getting-started",
        nav: "Getting started",
        title: "Getting started",
        blurb: "Install Blitsen, open built web output in a native window, check it and export it.",
      },
      {
        file: "CORE-CONCEPTS.md",
        slug: "core-concepts",
        nav: "Core concepts",
        title: "Core concepts",
        blurb: "Static output, web API boundaries, modules, assets, storage and native capabilities.",
      },
    ],
  },
  {
    name: "Build your app",
    note: "Configure a project and connect it to native capabilities.",
    pages: [
      {
        file: "CONFIGURATION.md",
        slug: "configuration",
        nav: "Configuration",
        title: "Configuration",
        blurb: "The package.json contract, build wrapping, application names and native addons.",
      },
      {
        file: "NATIVE-APIS.md",
        slug: "native-apis",
        nav: "Native APIs",
        title: "Native APIs",
        blurb: "Use window controls, dialogs, the clipboard, application directories and operating-system data.",
      },
      {
        file: "RECIPES.md",
        slug: "recipes",
        nav: "Recipes",
        title: "Recipes",
        blurb: "Common patterns for bundlers, hot reload, assets, data, dialogs and cross-builds.",
      },
    ],
  },
  {
    name: "Ship",
    note: "Package, test and distribute for the platforms you support.",
    pages: [
      {
        file: "PACKAGING.md",
        slug: "packaging",
        nav: "Packaging",
        title: "Packaging and distribution",
        blurb: "Build desktop executables and Android APKs, add metadata, sign and prepare a release.",
      },
      {
        file: "PLATFORM-SUPPORT.md",
        slug: "platform-support",
        nav: "Platform support",
        title: "Platform support",
        blurb: "Desktop targets, operating-system requirements, Android status and important limitations.",
      },
      {
        file: "LICENSING.md",
        slug: "licensing",
        nav: "Licensing",
        title: "Licensing Blitsen and exported applications",
        blurb: "Notices, source availability and the additional obligations of native-addon exports.",
      },
    ],
  },
  {
    name: "Reference",
    note: "Look up commands, supported APIs and fixes for common failures.",
    pages: [
      {
        file: "CLI.md",
        slug: "cli",
        nav: "CLI reference",
        title: "CLI reference",
        blurb: "Run, doctor and build commands, every option, target spelling and environment variable.",
      },
      {
        file: "WEB-APIS.md",
        slug: "web-apis",
        nav: "Web API support",
        title: "Web API support",
        blurb: "Supported areas, important absences, feature detection and how to interpret doctor.",
      },
      {
        file: "TROUBLESHOOTING.md",
        slug: "troubleshooting",
        nav: "Troubleshooting",
        title: "Troubleshooting",
        blurb: "Fix entrypoint, build-output, asset, runtime, platform, native API and signing failures.",
      },
    ],
  },
];

// Kept buildable so old links continue to resolve, but intentionally absent from
// the user documentation navigation. These are contributor specifications,
// historical decisions and milestone records rather than instructions for using
// the current product.
export const INTERNAL_PAGES: DocPage[] = [
  { file: "PRODUCT.md", slug: "product", nav: "Product specification", title: "Product specification", blurb: "Contributor product specification." },
  { file: "TECH.md", slug: "tech", nav: "Technical specification", title: "Technical specification", blurb: "Contributor architecture specification." },
  { file: "MODULES.md", slug: "modules", nav: "Module resolution record", title: "Module resolution in the shipped binary", blurb: "Internal module-resolution design record." },
  { file: "CONFORMANCE.md", slug: "conformance", nav: "Layout conformance", title: "Layout conformance corpus", blurb: "Contributor conformance record." },
  { file: "BLITZ-GAPS.md", slug: "blitz-gaps", nav: "Blitz gaps", title: "Blitz rendering gaps", blurb: "Upstream renderer issue record." },
  { file: "MIGRATION.md", slug: "migration", nav: "Runtime migration", title: "Runtime migration record", blurb: "Historical runtime migration note." },
  { file: "RELEASING.md", slug: "releasing", nav: "Maintainer release process", title: "Maintainer release process", blurb: "Instructions for Blitsen maintainers publishing the runtime." },
  { file: "RELEASE-NOTES-0.1.0.md", slug: "release-notes-0-1-0", nav: "0.1.0 notes", title: "0.1.0 release notes", blurb: "Release-specific notes for Blitsen 0.1.0." },
  { file: "M0.md", slug: "m0", nav: "M0 record", title: "M0 feasibility record", blurb: "Historical milestone record." },
  { file: "M2.md", slug: "m2", nav: "M2 record", title: "M2 interactive record", blurb: "Historical milestone record." },
  { file: "M3.md", slug: "m3", nav: "M3 record", title: "M3 Pong record", blurb: "Historical milestone record." },
  { file: "M3B.md", slug: "m3b", nav: "M3b record", title: "M3b adoption record", blurb: "Historical milestone record." },
  { file: "JSC.md", slug: "jsc", nav: "Engine choice record", title: "JavaScript engine choice record", blurb: "Historical engine acquisition and replacement record." },
  { file: "COMPATIBILITY.md", slug: "compatibility", nav: "Generated compatibility matrix", title: "Generated compatibility matrix", blurb: "Generated detailed API and diagnostic matrix." },
];

export const GUIDE_PAGES: DocPage[] = GROUPS.flatMap((g) => g.pages);
export const ALL_PAGES: DocPage[] = [...GUIDE_PAGES, ...INTERNAL_PAGES];

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
