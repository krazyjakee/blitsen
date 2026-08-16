// Asset-reference syntax shared by export reachability, root-relative rewriting,
// and the generated compatibility manifest. This module imports none of those
// consumers, so all three can depend on it without a cycle.

export const HTML_ASSET_ATTRIBUTES = Object.freeze([
  { element: "script", attribute: "src", remote: "script" },
  { element: "img", attribute: "src", remote: "asset" },
  { element: "source", attribute: "src", remote: "asset" },
  { element: "audio", attribute: "src", remote: "asset" },
  { element: "video", attribute: "src", remote: "asset" },
  { element: "track", attribute: "src", remote: "asset" },
  { element: "embed", attribute: "src", remote: "asset" },
  { element: "input", attribute: "src", remote: "asset" },
  { element: "link", attribute: "href", remote: "asset" },
  { element: "video", attribute: "poster", remote: "asset" },
  { element: "object", attribute: "data", remote: "asset" },
].map(Object.freeze));

export const CSS_ASSET_REFERENCES = Object.freeze([
  Object.freeze({
    syntax: "url",
    prefix: "url\\(\\s*[\"']?",
    value: "[^\"')]*",
    suffix: "[\"']?\\s*\\)",
    rewriteRoot: true,
    remote: true,
  }),
  // Kept as today's behavior: @import participates in reachability, but only
  // url() is root-rewritten and diagnosed when remote.
  Object.freeze({
    syntax: "import",
    prefix: "@import\\s+[\"']",
    value: "[^\"']*",
    suffix: "[\"']",
    rewriteRoot: false,
    remote: false,
  }),
]);

function groupedAttributes(rules) {
  const groups = new Map();
  for (const rule of rules) {
    const elements = groups.get(rule.attribute) ?? [];
    elements.push(rule.element);
    groups.set(rule.attribute, elements);
  }
  return [...groups].map(([attribute, elements]) => ({ attribute, elements }));
}

const elementsPattern = elements => elements.length === 1
  ? elements[0]
  : `(?:${elements.join("|")})`;

const htmlScanPattern = ({ elements, attribute }) =>
  `<${elementsPattern(elements)}\\b[^>]*?\\b${attribute}\\s*=\\s*["']([^"']*)["']`;
const htmlRewritePattern = ({ elements, attribute }) =>
  `(<${elementsPattern(elements)}\\b[^>]*\\b${attribute}\\s*=\\s*["'])(\\/(?!\\/)[^"']*)(["'])`;
const htmlRemotePattern = ({ elements, attribute }) =>
  `<${elementsPattern(elements)}\\b[^>]*\\b${attribute}\\s*=\\s*["'](?:https?:)?//`;

const htmlGroups = groupedAttributes(HTML_ASSET_ATTRIBUTES);
export const HTML_REFERENCE_PATTERNS = Object.freeze(
  htmlGroups.map(rule => new RegExp(htmlScanPattern(rule), "gi")));
export const HTML_ROOT_REFERENCE_PATTERNS = Object.freeze(
  htmlGroups.map(rule => new RegExp(htmlRewritePattern(rule), "gi")));

function remoteHtmlPattern(kind) {
  return groupedAttributes(HTML_ASSET_ATTRIBUTES.filter(rule => rule.remote === kind))
    .map(htmlRemotePattern).join("|");
}

export const REMOTE_HTML_SCRIPT_PATTERN = remoteHtmlPattern("script");
export const REMOTE_HTML_ASSET_PATTERN = remoteHtmlPattern("asset");

const cssScanPattern = rule => `${rule.prefix}(${rule.value})${rule.suffix}`;
const cssRewritePattern = rule =>
  `(${rule.prefix})(\\/(?!\\/)${rule.value})(${rule.suffix})`;
const cssRemotePattern = rule => `${rule.prefix}(?:https?:)?//`;

export const CSS_REFERENCE_PATTERNS = Object.freeze(
  CSS_ASSET_REFERENCES.map(rule => new RegExp(cssScanPattern(rule), "gi")));
export const CSS_ROOT_REFERENCE_PATTERNS = Object.freeze(CSS_ASSET_REFERENCES
  .filter(rule => rule.rewriteRoot)
  .map(rule => new RegExp(cssRewritePattern(rule), "gi")));
export const REMOTE_CSS_ASSET_PATTERN = CSS_ASSET_REFERENCES
  .filter(rule => rule.remote).map(cssRemotePattern).join("|");
