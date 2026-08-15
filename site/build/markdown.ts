// A small CommonMark-shaped renderer, covering exactly the syntax docs/*.md uses:
// ATX headings, fenced code, pipe tables, blockquotes, thematic breaks, ordered and
// unordered lists (including task lists), and inline emphasis/code/links.
//
// It is deliberately not a general Markdown implementation. The corpus was surveyed
// before this was written — there are no nested lists, no setext headings, no
// footnotes and no inline HTML outside fences — so the parser rejects nothing and
// guesses at nothing. If a doc later grows a construct this does not know, it falls
// through to a paragraph rather than silently dropping the text.

export interface Heading {
  depth: number;
  text: string;
  slug: string;
}

export interface RenderResult {
  html: string;
  headings: Heading[];
  /** First paragraph, stripped to plain text — used for meta descriptions. */
  summary: string;
}

export type LinkRewriter = (href: string) => string;

const HTML_ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]!);
}

/**
 * GitHub's heading-anchor algorithm. Matching it matters: docs already link to each
 * other with anchors like `#development-your-own-dev-server`, and those links must
 * keep working once the same headings are rendered here.
 */
export function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/<[^>]+>/g, "")
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .trim()
    .replace(/\s+/g, "-");
}

function uniqueSlug(base: string, seen: Map<string, number>): string {
  const n = seen.get(base) ?? 0;
  seen.set(base, n + 1);
  return n === 0 ? base : `${base}-${n}`;
}

// ── inline ────────────────────────────────────────────────────────────────────

const CODE_SLOT = "\u0000";

/**
 * Code spans are replaced by placeholders rather than split out, then restored after
 * the emphasis and link passes have run.
 *
 * Splitting was the obvious implementation and it was wrong: the docs write links
 * whose text is itself code — [`JSC.md`](JSC.md) — and splitting on backticks first
 * cuts the link in half, so the bracket syntax leaks into the page as literal text.
 * Placeholders keep the surrounding structure intact while still guaranteeing that
 * nothing inside backticks is ever interpreted as markup.
 */
function renderInline(src: string, link: LinkRewriter): string {
  const codes: string[] = [];
  const masked = src.replace(/(`+)([\s\S]*?)\1/g, (_m, _fence: string, body: string) => {
    const index = codes.push(`<code>${escapeHtml(body.replace(/^ (.*) $/, "$1"))}</code>`) - 1;
    return `${CODE_SLOT}${index}${CODE_SLOT}`;
  });
  const rendered = renderInlineText(masked, link);
  return rendered.replace(
    new RegExp(`${CODE_SLOT}(\\d+)${CODE_SLOT}`, "g"),
    (_m, index: string) => codes[Number(index)] ?? "",
  );
}

function renderInlineText(src: string, link: LinkRewriter): string {
  let out = escapeHtml(src);

  // Images before links — the syntaxes differ only by a leading `!`.
  out = out.replace(/!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;([^&]*)&quot;)?\)/g,
    (_m, alt: string, src2: string, title?: string) => {
      const t = title ? ` title="${title}"` : "";
      return `<img src="${link(src2)}" alt="${alt}" loading="lazy" decoding="async"${t}>`;
    });

  out = out.replace(/\[([^\]]+)\]\(([^)\s]+)(?:\s+&quot;([^&]*)&quot;)?\)/g,
    (_m, text: string, href: string, title?: string) => {
      const resolved = link(href);
      const external = /^https?:/.test(resolved);
      const attrs = external ? ' target="_blank" rel="noopener noreferrer"' : "";
      const t = title ? ` title="${title}"` : "";
      return `<a href="${resolved}"${attrs}${t}>${text}</a>`;
    });

  // Bare autolinks, but never inside an href/src we just produced.
  out = out.replace(/(^|[\s(])(https?:\/\/[^\s<>()]+)/g,
    (_m, lead: string, url: string) =>
      `${lead}<a href="${url}" target="_blank" rel="noopener noreferrer">${url}</a>`);

  out = out.replace(/\*\*\*([^*]+)\*\*\*/g, "<strong><em>$1</em></strong>");
  out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/(^|[^*\w])\*([^*\n]+)\*(?![*\w])/g, "$1<em>$2</em>");
  out = out.replace(/(^|[^_\w])_([^_\n]+)_(?![_\w])/g, "$1<em>$2</em>");

  out = out.replace(/ {2,}$/, "<br>");
  out = out.replace(/(\S)--(\S)/g, "$1—$2");
  return out;
}

// ── blocks ────────────────────────────────────────────────────────────────────

const HR = /^\s{0,3}(?:-{3,}|\*{3,}|_{3,})\s*$/;
const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const FENCE = /^\s{0,3}(`{3,}|~{3,})\s*([\w-]*)\s*$/;
const UL_ITEM = /^\s{0,3}([-*+])\s+(.*)$/;
const OL_ITEM = /^\s{0,3}(\d+)[.)]\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const TABLE_DELIM = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;

export function renderMarkdown(source: string, link: LinkRewriter = (h) => h): RenderResult {
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  const headings: Heading[] = [];
  const seen = new Map<string, number>();
  const out: string[] = [];
  let summary = "";
  let i = 0;

  const paragraphText = (text: string) => {
    if (!summary) summary = text.replace(/[*`_[\]]/g, "").replace(/\(.*?\)/g, "").trim();
  };

  while (i < lines.length) {
    const line = lines[i]!;

    if (!line.trim()) {
      i += 1;
      continue;
    }

    // HTML comments — the generated-block markers in COMPATIBILITY.md. Kept in the
    // output so the generator's fingerprints survive "view source", but invisible.
    if (line.trimStart().startsWith("<!--")) {
      const buf: string[] = [];
      while (i < lines.length) {
        buf.push(lines[i]!);
        if (lines[i]!.includes("-->")) { i += 1; break; }
        i += 1;
      }
      out.push(`<!-- ${escapeHtml(buf.join(" ").replace(/<!--|-->/g, "").trim())} -->`);
      continue;
    }

    const fence = line.match(FENCE);
    if (fence) {
      const marker = fence[1]!;
      const lang = fence[2] ?? "";
      const buf: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i]!.trimStart().startsWith(marker)) {
        buf.push(lines[i]!);
        i += 1;
      }
      i += 1; // closing fence
      const cls = lang ? ` class="language-${lang}"` : "";
      const label = lang ? `<span class="code-lang">${escapeHtml(lang)}</span>` : "";
      out.push(
        `<figure class="code">${label}<pre><code${cls}>${escapeHtml(buf.join("\n"))}</code></pre></figure>`,
      );
      continue;
    }

    const heading = line.match(HEADING);
    if (heading) {
      const depth = heading[1]!.length;
      const raw = heading[2]!;
      const slug = uniqueSlug(slugify(raw), seen);
      headings.push({ depth, text: raw.replace(/[*`]/g, ""), slug });
      const inner = renderInline(raw, link);
      out.push(
        `<h${depth} id="${slug}">${inner}` +
        `<a class="anchor" href="#${slug}" aria-label="Link to this section">#</a></h${depth}>`,
      );
      i += 1;
      continue;
    }

    if (HR.test(line)) {
      out.push("<hr>");
      i += 1;
      continue;
    }

    // Tables: a header row followed by a delimiter row of dashes.
    if (line.includes("|") && i + 1 < lines.length && TABLE_DELIM.test(lines[i + 1]!)) {
      const cells = (row: string) => {
        const trimmed = row.trim().replace(/^\|/, "").replace(/\|$/, "");
        return trimmed.split("|").map((c) => c.trim());
      };
      const header = cells(line);
      const aligns = cells(lines[i + 1]!).map((spec) => {
        const left = spec.startsWith(":");
        const right = spec.endsWith(":");
        if (left && right) return " style=\"text-align:center\"";
        if (right) return " style=\"text-align:right\"";
        return "";
      });
      i += 2;
      const body: string[][] = [];
      while (i < lines.length && lines[i]!.includes("|") && lines[i]!.trim()) {
        body.push(cells(lines[i]!));
        i += 1;
      }
      const head = header
        .map((c, n) => `<th${aligns[n] ?? ""}>${renderInline(c, link)}</th>`)
        .join("");
      const rows = body
        .map((r) => `<tr>${r.map((c, n) => `<td${aligns[n] ?? ""}>${renderInline(c, link)}</td>`).join("")}</tr>`)
        .join("");
      out.push(`<div class="table-scroll"><table><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table></div>`);
      continue;
    }

    if (QUOTE.test(line)) {
      const buf: string[] = [];
      while (i < lines.length && (QUOTE.test(lines[i]!) || (buf.length > 0 && lines[i]!.trim()))) {
        const m = lines[i]!.match(QUOTE);
        buf.push(m ? m[1]! : lines[i]!.trim());
        i += 1;
      }
      const inner = renderMarkdown(buf.join("\n"), link);
      out.push(`<blockquote>${inner.html}</blockquote>`);
      continue;
    }

    if (UL_ITEM.test(line) || OL_ITEM.test(line)) {
      const ordered = OL_ITEM.test(line) && !UL_ITEM.test(line);
      const items: string[] = [];
      let loose = false;

      while (i < lines.length) {
        const m = ordered ? lines[i]!.match(OL_ITEM) : lines[i]!.match(UL_ITEM);
        if (!m) {
          // A blank line is only a break if the next line is not another item.
          if (!lines[i]!.trim()) {
            const next = lines[i + 1];
            const stillList = next !== undefined &&
              (ordered ? OL_ITEM.test(next) : UL_ITEM.test(next));
            if (!stillList) break;
            loose = true;
            i += 1;
            continue;
          }
          break;
        }
        const parts = [m[2]!];
        i += 1;
        // Lazy continuation: indented or plain lines belong to the item above.
        while (
          i < lines.length && lines[i]!.trim() &&
          !UL_ITEM.test(lines[i]!) && !OL_ITEM.test(lines[i]!) &&
          !HEADING.test(lines[i]!) && !FENCE.test(lines[i]!) && !HR.test(lines[i]!)
        ) {
          parts.push(lines[i]!.trim());
          i += 1;
        }
        items.push(parts.join(" "));
      }

      const rendered = items.map((raw) => {
        const task = raw.match(/^\[([ xX])\]\s+(.*)$/);
        if (task) {
          const done = task[1]!.toLowerCase() === "x";
          return `<li class="task"><input type="checkbox" disabled${done ? " checked" : ""}>` +
            `<span>${renderInline(task[2]!, link)}</span></li>`;
        }
        const body = renderInline(raw, link);
        return `<li>${loose ? `<p>${body}</p>` : body}</li>`;
      }).join("");

      const tag = ordered ? "ol" : "ul";
      const hasTasks = items.some((r) => /^\[[ xX]\]/.test(r));
      out.push(`<${tag}${hasTasks ? ' class="tasks"' : ""}>${rendered}</${tag}>`);
      continue;
    }

    // Paragraph: everything up to a blank line or the start of another block.
    const buf: string[] = [];
    while (
      i < lines.length && lines[i]!.trim() &&
      !HEADING.test(lines[i]!) && !FENCE.test(lines[i]!) && !HR.test(lines[i]!) &&
      !QUOTE.test(lines[i]!) && !UL_ITEM.test(lines[i]!) && !OL_ITEM.test(lines[i]!)
    ) {
      buf.push(lines[i]!);
      i += 1;
    }
    const text = buf.join("\n");
    paragraphText(text);
    // A paragraph that is nothing but an image becomes a figure, so the docs'
    // screenshot-plus-caption pattern renders as one.
    const onlyImage = text.trim().match(/^!\[([^\]]*)\]\(([^)\s]+)\)$/);
    if (onlyImage) {
      out.push(`<figure class="shot">${renderInline(text.trim(), link)}</figure>`);
    } else {
      out.push(`<p>${renderInline(text, link)}</p>`);
    }
  }

  return { html: out.join("\n"), headings, summary };
}
