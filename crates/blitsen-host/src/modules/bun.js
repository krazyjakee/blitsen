(() => {
  const state = globalThis[Symbol.for("blitsen.bun.modules")] ??= { generation: 0 };
  state.generation++;
  state.inline = new Map();
  if (state.plugin) return;
  state.plugin = Bun.plugin({ name: "blitsen-application-modules", setup(build) {
    const applicationUrl = path => path.slice(path.indexOf("|") + 1);
    build.onResolve({ filter: /^\/\/app\//, namespace: "blitsen" }, ({ path }) => ({
      path: `${state.generation}|blitsen:${path}`, namespace: "blitsen",
    }));
    // Bun's runtime plugins resolve relative imports in the default namespace.
    build.onResolve({ filter: /^[./]/ }, args => {
      if (!args.importer.startsWith("blitsen:")) return;
      return { path: `${state.generation}|${__blitsenModuleResolve(applicationUrl(args.importer), args.path)}`,
        namespace: "blitsen" };
    });
    build.onLoad({ filter: /.*/, namespace: "blitsen" }, ({ path }) => {
      const url = applicationUrl(path);
      return { contents: `import.meta.url = ${JSON.stringify(url)};${state.inline.get(url) ?? __blitsenModuleSource(url)}`,
        loader: "js" };
    });
  }});
})();
