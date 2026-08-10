const __blitsenReloadScoped = "second-document";
if ("__blitsenReloadLeak" in globalThis) throw new Error("document global state leaked across reload");
throw new Error("intentional script fixture failure");
