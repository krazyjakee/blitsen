import { moduleStep } from "./dependency.js";

const target = document.getElementById("script-target");
target.setAttribute("data-order", target.getAttribute("data-order") + `,${moduleStep}`);
target.setAttribute("data-module-url", import.meta.url);
