//! QuickJS's module loader, bridged to the registry on the global object.

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{Coerced, Ctx, FromJs, Function, Module};

use crate::context::QuickJs;

struct RegistryResolver;

impl Resolver for RegistryResolver {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let resolved = call_global(ctx, "__blitsenModuleResolve", &[base, name])?;
        if resolved.contains('\0') {
            return Err(QuickJs::throw(
                ctx,
                "the resolved module name contains a NUL byte",
            ));
        }
        Ok(resolved)
    }
}

struct RegistryLoader;

impl Loader for RegistryLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        let source = call_global(ctx, "__blitsenModuleSource", &[name])?;
        if source.contains('\0') {
            return Err(QuickJs::throw(
                ctx,
                &format!("the module at {name} contains a NUL byte"),
            ));
        }
        let module = Module::declare(ctx.clone(), name, source)?;
        let meta = module.meta()?;
        meta.set("url", name)?;
        meta.set("main", false)?;
        Ok(module)
    }
}

fn call_global<'js>(ctx: &Ctx<'js>, name: &str, arguments: &[&str]) -> rquickjs::Result<String> {
    let function: Function = ctx.globals().get(name)?;
    let mut args = rquickjs::function::Args::new(ctx.clone(), arguments.len());
    for argument in arguments {
        args.push_arg(*argument)?;
    }
    let value = function.call_arg(args)?;
    Ok(Coerced::<String>::from_js(ctx, value)?.0)
}

impl QuickJs {
    /// Routes module normalization and loading through the installed registry.
    pub fn install_module_loader(&mut self) {
        self.inner
            .runtime
            .set_loader(RegistryResolver, RegistryLoader);
    }

    /// Always true: module support is part of this engine.
    pub fn supports_modules(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitsen_js::JsEngine;

    #[test]
    fn registry_loader_resolves_sources_and_sets_import_meta() {
        let mut engine = QuickJs::new().unwrap();
        engine
            .evaluate_script(
                r#"
                globalThis.__blitsenModuleResolve = (_base, name) => `blitsen://app/${name.replace('./', '')}`;
                globalThis.__blitsenModuleSource = _name => `export const answer = 42; globalThis.loadedUrl = import.meta.url`;
                "#,
                "registry.js",
            )
            .unwrap();
        engine.install_module_loader();
        engine
            .evaluate_module(
                "import { answer } from './dep.js'; globalThis.answer = answer",
                "blitsen://app/main.js",
            )
            .unwrap();
        engine.drain_microtasks().unwrap();
        let value = engine
            .evaluate_script("`${answer}:${loadedUrl}`", "result.js")
            .unwrap();
        assert_eq!(engine.to_string(&value).unwrap(), "42:blitsen://app/dep.js");
    }

    #[test]
    fn resolver_rejects_nul_without_losing_the_exception() {
        let mut engine = QuickJs::new().unwrap();
        engine
            .evaluate_script(
                "globalThis.__blitsenModuleResolve=()=>\"a\\0b\";globalThis.__blitsenModuleSource=()=>\"\"",
                "registry.js",
            )
            .unwrap();
        engine.install_module_loader();
        let error = engine
            .evaluate_module("import './x.js'", "main.js")
            .unwrap_err();
        assert_eq!(
            error.message(),
            "Error: the resolved module name contains a NUL byte"
        );
    }
}
