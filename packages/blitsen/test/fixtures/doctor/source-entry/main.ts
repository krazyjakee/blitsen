// Source, not built output. Nothing in Blitsen transpiles this, which is what
// makes an entry point naming it a blocking diagnostic rather than a warning.
const app = document.getElementById("app") as HTMLElement;
app.textContent = "unbuilt";
