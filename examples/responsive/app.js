// Everything reported here is read back from the renderer rather than tracked
// alongside it. A demo about responsive layout that kept its own idea of the
// breakpoint would agree with itself no matter what the cascade did, which
// would make it evidence of nothing.

const viewport = document.getElementById("viewport");
const breakpointOut = document.getElementById("breakpoint");
const orientationOut = document.getElementById("orientation");
const columnsOut = document.getElementById("columns");
const observedOut = document.getElementById("observed");
const changesOut = document.getElementById("changes");
const grid = document.querySelector(".grid");

let resizes = 0;
let flips = 0;

// The same queries the stylesheet declares. `matchMedia` runs them through the
// parser and evaluator the cascade uses, so a flip here is the flip that
// changed the layout — not a second guess at it.
const narrow = matchMedia("(max-width: 620px)");
const medium = matchMedia("(max-width: 900px)");
const landscape = matchMedia("(orientation: landscape)");

const breakpoint = () => {
  if (narrow.matches) return "narrow · one column";
  if (medium.matches) return "medium · two columns";
  return "wide · three columns";
};

// Counted from where the boxes actually landed: how many cards share the top
// edge of the first one. Not from `getComputedStyle(grid).gridTemplateColumns`,
// which reports the computed *declaration* — `repeat(3, 1fr)` stays
// `repeat(3, 1fr)` at every width, so counting what it says would produce a
// number that never changed and looked like it had. Only `width` and `height`
// come back as used values (COMPATIBILITY.md), and geometry is what this is
// asking about, so geometry is what it reads.
const columns = () => {
  const cards = [...grid.children];
  if (cards.length === 0) return "0";
  const first = cards[0].getBoundingClientRect().top;
  const across = cards.filter(card =>
    Math.abs(card.getBoundingClientRect().top - first) < 1).length;
  const declared = getComputedStyle(grid).gridTemplateColumns.trim();
  return `${across} laid out · ${declared} declared`;
};

const report = () => {
  viewport.textContent = `${innerWidth} x ${innerHeight}`;
  breakpointOut.textContent = breakpoint();
  orientationOut.textContent = landscape.matches ? "landscape" : "portrait";
  columnsOut.textContent = columns();
  changesOut.textContent = `${resizes} resizes · ${flips} query flips`;
};

// A resize event per window resize, and a change event only when a query
// actually flips — the two counters are there to show that those are different
// numbers. Both land at the start of the frame turn.
addEventListener("resize", () => {
  resizes += 1;
  report();
});

for (const query of [narrow, medium, landscape]) {
  query.addEventListener("change", () => {
    flips += 1;
    report();
  });
}

// The renderer measures this box; nothing here does. The first observation is
// guaranteed, so this readout fills in without waiting for a resize.
new ResizeObserver(entries => {
  for (const entry of entries) {
    const { width, height } = entry.contentRect;
    observedOut.textContent = `${Math.round(width)} x ${Math.round(height)} content box`;
  }
}).observe(document.querySelector(".card"));

report();
