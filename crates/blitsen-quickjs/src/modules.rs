//! QuickJS's module loader, bridged to the registry on the global object.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use rquickjs_sys as q;

use crate::context::{QuickJs, borrowed, throw};

/// Resolves a specifier through the registry's `__blitsenModuleResolve`.
///
/// QuickJS asks for the normalized name first and the source second, which is
/// the same two-step `blitsen-host`'s registry already exposes to JavaScript —
/// so this bridges to those globals rather than reaching into Rust a second
/// time. The returned string must be allocated by QuickJS, because QuickJS
/// frees it.
unsafe extern "C" fn normalize_module(
    ctx: *mut q::JSContext,
    base: *const c_char,
    name: *const c_char,
    _opaque: *mut c_void,
) -> *mut c_char {
    match unsafe { call_global(ctx, "__blitsenModuleResolve", &[base, name]) } {
        Ok(text) => {
            let owned = CString::new(text).unwrap_or_default();
            unsafe { q::js_strdup(ctx, owned.as_ptr()) }
        }
        // The exception QuickJS raised is still on the context, which is
        // where it looks for the reason this returned nothing.
        Err(Pending) => std::ptr::null_mut(),
    }
}

/// A QuickJS exception is pending on the context, and is the caller's answer.
///
/// Returning null from a module-loader callback means "there is an exception to
/// report", and QuickJS reports whatever is pending. So nothing below may take
/// it out — which is why this carries no error value at all. It used to: the
/// resolver was called through the engine wrapper, whose job is to turn a pending
/// exception into a `JsError`, and *taking* it is what cleared it. The
/// loader then returned null with the context clean, and a failed `import()`
/// rejected with QuickJS's uninitialized marker — no name, no message, no
/// stack, and the same for every possible cause.
struct Pending;

/// Compiles the module `name` from source the registry supplies.
unsafe extern "C" fn load_module(
    ctx: *mut q::JSContext,
    name: *const c_char,
    _opaque: *mut c_void,
) -> *mut q::JSModuleDef {
    let Ok(source) = (unsafe { call_global(ctx, "__blitsenModuleSource", &[name]) }) else {
        return std::ptr::null_mut();
    };
    let Ok(code) = CString::new(source.clone()) else {
        unsafe {
            throw(
                ctx,
                &format!("the module at {} contains a NUL byte", name_of(name)),
            )
        };
        return std::ptr::null_mut();
    };
    unsafe {
        // Compile only: QuickJS links and evaluates the graph itself once every
        // dependency has been handed back.
        let compiled = q::JS_Eval(
            ctx,
            code.as_ptr(),
            source.len() as q::size_t,
            name,
            (q::JS_EVAL_TYPE_MODULE | q::JS_EVAL_FLAG_COMPILE_ONLY) as c_int,
        );
        if q::JS_IsException(compiled) {
            return std::ptr::null_mut();
        }
        let module = q::JS_VALUE_GET_PTR(compiled).cast::<q::JSModuleDef>();
        q::JS_FreeValue(ctx, compiled);
        set_import_meta(ctx, module, name);
        module
    }
}

/// Fills in `import.meta` for a module the loader just compiled.
///
/// QuickJS creates the object and leaves it empty; nothing else will populate
/// it. `import.meta.url` is the module's own address by definition, and it is
/// what `new Worker(new URL("./work.js", import.meta.url))` — the way every
/// bundler emits a worker — resolves against. Left undefined, that idiom
/// silently names the wrong file.
pub(crate) unsafe fn set_import_meta(
    ctx: *mut q::JSContext,
    module: *mut q::JSModuleDef,
    name: *const c_char,
) {
    unsafe {
        let meta = q::JS_GetImportMeta(ctx, module);
        if q::JS_IsException(meta) {
            return;
        }
        let url = q::JS_NewStringLen(
            ctx,
            name,
            CStr::from_ptr(name).to_bytes().len() as q::size_t,
        );
        q::JS_SetPropertyStr(ctx, meta, c"url".as_ptr(), url);
        // False for every module here: an application is entered through its
        // document, so none of them is a main module in the Node sense.
        q::JS_SetPropertyStr(ctx, meta, c"main".as_ptr(), q::JS_FALSE);
        q::JS_FreeValue(ctx, meta);
    }
}

/// Reads a C string the loader was handed, for a diagnostic.
unsafe fn name_of(name: *const c_char) -> String {
    if name.is_null() {
        return "<unnamed>".to_owned();
    }
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// Calls a global function with C string arguments and returns its result.
///
/// Deliberately the raw API rather than the [`JsEngine`] wrapper the rest of
/// this crate uses: the wrapper converts a pending exception into a `JsError`,
/// and here the exception has to stay exactly where QuickJS left it, keeping its
/// own type, message and stack for the import that failed to report.
unsafe fn call_global(
    ctx: *mut q::JSContext,
    name: &str,
    arguments: &[*const c_char],
) -> Result<String, Pending> {
    unsafe {
        let global = q::JS_GetGlobalObject(ctx);
        // Every allocation below is freed on both paths; the exception, if any,
        // lives on the context rather than in a value being carried out.
        let key = CString::new(name).map_err(|_| Pending)?;
        let function = q::JS_GetPropertyStr(ctx, global, key.as_ptr());
        let mut values = Vec::with_capacity(arguments.len());
        let mut failed = q::JS_IsException(function);
        if !failed {
            for argument in arguments {
                let text = if argument.is_null() {
                    c"".to_owned()
                } else {
                    CStr::from_ptr(*argument).to_owned()
                };
                let bytes = text.as_bytes();
                let value = q::JS_NewStringLen(
                    ctx,
                    bytes.as_ptr().cast::<c_char>(),
                    bytes.len() as q::size_t,
                );
                failed |= q::JS_IsException(value);
                values.push(value);
            }
        }
        let result = if failed {
            q::JS_EXCEPTION
        } else {
            q::JS_Call(
                ctx,
                function,
                global,
                values.len() as c_int,
                values.as_mut_ptr(),
            )
        };
        for value in values {
            q::JS_FreeValue(ctx, value);
        }
        q::JS_FreeValue(ctx, function);
        q::JS_FreeValue(ctx, global);
        if q::JS_IsException(result) {
            return Err(Pending);
        }
        let text = borrowed(ctx).text(result);
        q::JS_FreeValue(ctx, result);
        text.ok_or(Pending)
    }
}

impl QuickJs {
    /// Points QuickJS's module loader at the registry already installed on the
    /// global object.
    ///
    /// This is the stock public API. The JavaScriptCore host it replaced needed
    /// a patched engine to expose the same hook at all (`docs/JSC.md`), which is
    /// one of the reasons the swap was worth making.
    pub fn install_module_loader(&mut self) {
        unsafe {
            q::JS_SetModuleLoaderFunc(
                self.inner.runtime,
                Some(normalize_module),
                Some(load_module),
                std::ptr::null_mut(),
            );
        }
    }

    /// Always true: module support is not optional in this engine.
    pub fn supports_modules(&self) -> bool {
        true
    }
}
