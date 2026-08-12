import { moduleStep } from "./dependency.js";

const target = document.getElementById("script-target");
target.setAttribute("data-order", target.getAttribute("data-order") + `,${moduleStep}`);
target.setAttribute("data-module-url", import.meta.url);

// Unqualified, from an ES module, so `this` is undefined at the call site. A
// browser substitutes the global for a WebIDL operation on Window; an unbound
// installed function would throw inside the listener table instead.
addEventListener("load", () => {
  target.setAttribute("data-module-load", "fired");
});
