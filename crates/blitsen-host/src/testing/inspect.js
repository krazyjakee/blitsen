// Retained by the test host. No inspection or injection global is published.
(() => {
  const normalize = value => String(value ?? "").replace(/\s+/g, " ").trim();
  const role = element => element.getAttribute("role") || ({
    button: "button", textarea: "textbox", select: "combobox", a: element.hasAttribute("href") ? "link" : null,
    input: ({ checkbox: "checkbox", radio: "radio", submit: "button", button: "button", reset: "button", hidden: null })[element.type]
      ?? (element.type === "hidden" ? null : "textbox"),
  })[element.localName] || null;
  const name = element => {
    const labelled = element.getAttribute("aria-labelledby");
    if (labelled) return normalize(labelled.split(/\s+/).map(id => document.getElementById(id)?.textContent).join(" "));
    if (element.hasAttribute("aria-label")) return normalize(element.getAttribute("aria-label"));
    const labels = [...document.querySelectorAll("label")].filter(label =>
      label.getAttribute("for") === element.id && element.id || label.contains(element));
    if (labels.length) return normalize(labels.map(label => label.textContent).join(" "));
    return normalize(element.textContent || element.getAttribute("alt") ||
      (element.localName === "input" && role(element) === "button" ? element.value : "") || element.getAttribute("title"));
  };
  return serialized => {
    const locator = JSON.parse(serialized);
    const spec = typeof locator === "string" ? { selector: locator } : locator;
    return JSON.stringify([...document.querySelectorAll(spec.selector || "*")].filter(element =>
      (spec.text === undefined || normalize(element.textContent) === normalize(spec.text)) &&
      (spec.role === undefined || role(element) === spec.role) &&
      (spec.name === undefined || name(element) === normalize(spec.name)))
      .map(element => {
        const rect = element.getBoundingClientRect();
        return { selector: spec.selector, tag: element.localName, id: element.id,
          // The bridge's handle is deliberately obtained only in this private
          // host closure, never exposed as an application injection function.
          handle: String(element[Object.getOwnPropertySymbols(element).find(symbol => symbol.description === "Blitsen node handle")]),
          text: normalize(element.textContent), role: role(element), name: name(element),
          box: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
          devicePixelRatio, viewport: { width: innerWidth, height: innerHeight },
          disabled: element.matches(":disabled"), value: element.value ?? null,
          checked: element.checked ?? null, focused: document.activeElement === element,
          scrollLeft: element.scrollLeft, scrollTop: element.scrollTop };
      }));
  };
})()
