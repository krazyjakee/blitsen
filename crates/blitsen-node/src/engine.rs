//! The [`blitsen_js::JsEngine`] implementation over Node-API.
//!
//! Everything that converts between Rust values and `napi` values lives here,
//! so the rest of the addon can speak in engine terms rather than in handles.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

use blitsen_core::parse_inline_script_identifier;
use blitsen_host::modules::APP_ORIGIN;
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    TypedArray, TypedArrayKind,
};
use napi::bindgen_prelude::{FromNapiValue, Unknown};
use napi::{Env, JsValue, Status, ValueType, sys};
use url::Url;

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

pub(crate) fn js_error(error: napi::Error) -> JsError {
    JsError::new(error.reason)
}

pub(crate) fn napi_error(error: JsError) -> napi::Error {
    napi::Error::new(Status::GenericFailure, error.to_string())
}

pub(crate) fn check(status: sys::napi_status, operation: &str) -> Result<(), JsError> {
    if status == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(JsError::new(format!(
            "{operation} failed with Node-API status {status}"
        )))
    }
}

pub(crate) fn check_call(
    env: sys::napi_env,
    status: sys::napi_status,
    operation: &str,
) -> Result<(), JsError> {
    if status != sys::Status::napi_pending_exception {
        return check(status, operation);
    }

    let mut exception = ptr::null_mut();
    // SAFETY: Node-API reported a pending exception for this environment.
    check(
        unsafe { sys::napi_get_and_clear_last_exception(env, &mut exception) },
        "capture JavaScript exception",
    )?;
    let mut string = ptr::null_mut();
    check(
        unsafe { sys::napi_coerce_to_string(env, exception, &mut string) },
        "stringify JavaScript exception",
    )?;
    let message = unsafe { String::from_napi_value(env, string) }.map_err(js_error)?;
    Err(JsError::new(message))
}

pub(crate) fn unknown(env: sys::napi_env, value: sys::napi_value) -> Unknown<'static> {
    // SAFETY: every value passed here was returned by Node-API for this env.
    unsafe { Unknown::from_raw_unchecked(env, value) }
}

pub(crate) fn raw(value: &Unknown<'static>) -> sys::napi_value {
    value.raw()
}

/// A weak Node-API reference. A zero refcount does not keep its target alive.
pub struct NodeWeakRef {
    env: sys::napi_env,
    reference: sys::napi_ref,
}

impl Drop for NodeWeakRef {
    fn drop(&mut self) {
        // SAFETY: the reference belongs to this environment and is deleted once.
        unsafe { sys::napi_delete_reference(self.env, self.reference) };
    }
}

/// Persistent handle to a registered native constructor.
pub struct NodeClass {
    env: sys::napi_env,
    reference: sys::napi_ref,
}

impl NodeClass {
    fn value(&self) -> Result<sys::napi_value, JsError> {
        let mut value = ptr::null_mut();
        // SAFETY: the class owns a live strong reference in this environment.
        check(
            unsafe { sys::napi_get_reference_value(self.env, self.reference, &mut value) },
            "read native class reference",
        )?;
        Ok(value)
    }
}

impl Drop for NodeClass {
    fn drop(&mut self) {
        // SAFETY: the reference belongs to this environment and is deleted once.
        unsafe { sys::napi_delete_reference(self.env, self.reference) };
    }
}

pub(crate) struct InstanceData {
    id: ExternalId,
    finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
}

unsafe extern "C" fn finalize_instance(_env: sys::napi_env, data: *mut c_void, _hint: *mut c_void) {
    // SAFETY: `instantiate` gives ownership of exactly one boxed InstanceData
    // to napi_wrap, which invokes this callback at most once.
    let mut data = unsafe { Box::from_raw(data.cast::<InstanceData>()) };
    if let Some(finalizer) = data.finalizer.take() {
        finalizer(data.id);
    }
}

/// [`JsEngine`] implementation backed exclusively by ABI-stable Node-API.
///
/// Values are scoped Node-API handles. Persistent bridge state must retain
/// objects through [`NodeWeakRef`] or a JavaScript-owned property.
///
/// Cloning produces another view of the same environment. The environment
/// pointer outlives every addon call, so a clone may be stored; the handles it
/// produces may not.
#[derive(Clone, Copy)]
pub struct NodeApiEngine {
    env: Env,
}

impl NodeApiEngine {
    /// Creates an engine view for the current addon environment.
    pub fn new(env: Env) -> Self {
        // Every entry point into this addon comes through here, which makes it
        // the one place a worker launcher can be registered before any document
        // could construct a `Worker`. Registration is idempotent.
        blitsen_host::worker::register_launcher(Box::new(crate::workers::Workers));
        Self { env }
    }

    pub(crate) fn raw_env(&self) -> sys::napi_env {
        self.env.raw()
    }

    fn value_from_raw(&self, value: sys::napi_value) -> Unknown<'static> {
        unknown(self.raw_env(), value)
    }
}

impl JsEngine for NodeApiEngine {
    type Value = Unknown<'static>;
    type WeakRef = NodeWeakRef;
    type Class = NodeClass;

    fn from_value(value: &Self::Value) -> Self {
        // Every Node-API handle records the environment that produced it, and a
        // callback argument is by definition live in the current one.
        Self::new(Env::from_raw(value.value().env))
    }

    fn undefined(&mut self) -> Self::Value {
        let mut value = ptr::null_mut();
        // SAFETY: the output pointer is valid and the environment is current.
        unsafe { sys::napi_get_undefined(self.raw_env(), &mut value) };
        self.value_from_raw(value)
    }

    fn null(&mut self) -> Self::Value {
        let mut value = ptr::null_mut();
        // SAFETY: the output pointer is valid and the environment is current.
        unsafe { sys::napi_get_null(self.raw_env(), &mut value) };
        self.value_from_raw(value)
    }

    fn boolean(&mut self, boolean: bool) -> Self::Value {
        let mut value = ptr::null_mut();
        // SAFETY: the output pointer is valid and the environment is current.
        unsafe { sys::napi_get_boolean(self.raw_env(), boolean, &mut value) };
        self.value_from_raw(value)
    }

    fn number(&mut self, number: f64) -> Self::Value {
        let mut value = ptr::null_mut();
        // SAFETY: the output pointer is valid and the environment is current.
        unsafe { sys::napi_create_double(self.raw_env(), number, &mut value) };
        self.value_from_raw(value)
    }

    fn string(&mut self, string: &str) -> Result<Self::Value, JsError> {
        let value = self.env.create_string(string).map_err(js_error)?;
        Ok(self.value_from_raw(value.raw()))
    }

    fn object(&mut self) -> Result<Self::Value, JsError> {
        let mut value = ptr::null_mut();
        check(
            unsafe { sys::napi_create_object(self.raw_env(), &mut value) },
            "create object",
        )?;
        Ok(self.value_from_raw(value))
    }

    fn array(&mut self, values: &[Self::Value]) -> Result<Self::Value, JsError> {
        let mut array = ptr::null_mut();
        // SAFETY: output and element handles belong to the current environment.
        check(
            unsafe { sys::napi_create_array_with_length(self.raw_env(), values.len(), &mut array) },
            "create array",
        )?;
        for (index, value) in values.iter().enumerate() {
            check(
                unsafe { sys::napi_set_element(self.raw_env(), array, index as u32, raw(value)) },
                "set array element",
            )?;
        }
        Ok(self.value_from_raw(array))
    }

    fn typed_array(&mut self, typed: &TypedArray) -> Result<Self::Value, JsError> {
        let mut buffer = ptr::null_mut();
        let mut data = ptr::null_mut();
        // SAFETY: Node-API initializes the buffer and its writable data pointer.
        check(
            unsafe {
                sys::napi_create_arraybuffer(
                    self.raw_env(),
                    typed.bytes.len(),
                    &mut data,
                    &mut buffer,
                )
            },
            "create typed-array buffer",
        )?;
        if !typed.bytes.is_empty() {
            // SAFETY: the arraybuffer allocation is exactly bytes.len() bytes.
            unsafe {
                ptr::copy_nonoverlapping(typed.bytes.as_ptr(), data.cast(), typed.bytes.len())
            };
        }
        let mut value = ptr::null_mut();
        check(
            unsafe {
                sys::napi_create_typedarray(
                    self.raw_env(),
                    typed_array_type(typed.kind),
                    typed.len(),
                    buffer,
                    0,
                    &mut value,
                )
            },
            "create typed array",
        )?;
        Ok(self.value_from_raw(value))
    }

    fn value_type(&mut self, value: &Self::Value) -> Result<JsType, JsError> {
        let mut value_type = -1;
        check(
            unsafe { sys::napi_typeof(self.raw_env(), raw(value), &mut value_type) },
            "read value type",
        )?;
        match ValueType::from(value_type) {
            ValueType::Undefined => Ok(JsType::Undefined),
            ValueType::Null => Ok(JsType::Null),
            ValueType::Boolean => Ok(JsType::Boolean),
            ValueType::Number => Ok(JsType::Number),
            ValueType::String => Ok(JsType::String),
            ValueType::Function => Ok(JsType::Function),
            ValueType::Object => {
                let mut yes = false;
                check(
                    unsafe { sys::napi_is_array(self.raw_env(), raw(value), &mut yes) },
                    "check array type",
                )?;
                if yes {
                    return Ok(JsType::Array);
                }
                check(
                    unsafe { sys::napi_is_typedarray(self.raw_env(), raw(value), &mut yes) },
                    "check typed-array type",
                )?;
                Ok(if yes {
                    JsType::TypedArray
                } else {
                    JsType::Object
                })
            }
            other => Err(JsError::new(format!(
                "unsupported JavaScript value type {other}"
            ))),
        }
    }

    fn to_boolean(&mut self, value: &Self::Value) -> Result<bool, JsError> {
        value.coerce_to_bool().map_err(js_error)
    }

    fn to_number(&mut self, value: &Self::Value) -> Result<f64, JsError> {
        let coerced = value.coerce_to_number().map_err(js_error)?;
        // SAFETY: coercion returned a number in this environment.
        unsafe { f64::from_napi_value(self.raw_env(), coerced.raw()) }.map_err(js_error)
    }

    fn to_string(&mut self, value: &Self::Value) -> Result<String, JsError> {
        let coerced = value.coerce_to_string().map_err(js_error)?;
        // SAFETY: coercion returned a string in this environment.
        unsafe { String::from_napi_value(self.raw_env(), coerced.raw()) }.map_err(js_error)
    }

    fn to_array(&mut self, value: &Self::Value) -> Result<Vec<Self::Value>, JsError> {
        if self.value_type(value)? != JsType::Array {
            return Err(JsError::new("value is not an array"));
        }
        let mut length = 0;
        check(
            unsafe { sys::napi_get_array_length(self.raw_env(), raw(value), &mut length) },
            "read array length",
        )?;
        (0..length)
            .map(|index| {
                let mut element = ptr::null_mut();
                check(
                    unsafe {
                        sys::napi_get_element(self.raw_env(), raw(value), index, &mut element)
                    },
                    "read array element",
                )?;
                Ok(self.value_from_raw(element))
            })
            .collect()
    }

    fn to_typed_array(&mut self, value: &Self::Value) -> Result<TypedArray, JsError> {
        let mut kind = 0;
        let mut length = 0;
        let mut data = ptr::null_mut();
        let mut buffer = ptr::null_mut();
        let mut offset = 0;
        check(
            unsafe {
                sys::napi_get_typedarray_info(
                    self.raw_env(),
                    raw(value),
                    &mut kind,
                    &mut length,
                    &mut data,
                    &mut buffer,
                    &mut offset,
                )
            },
            "read typed array",
        )?;
        let kind = from_typed_array_type(kind)?;
        let byte_length = length * kind.element_size();
        let bytes = if byte_length == 0 {
            Vec::new()
        } else {
            // SAFETY: Node-API returned a live view covering this many bytes.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), byte_length) }.to_vec()
        };
        TypedArray::new(kind, bytes)
    }

    fn get_property(&mut self, object: &Self::Value, name: &str) -> Result<Self::Value, JsError> {
        let key = self.string(name)?;
        let mut result = ptr::null_mut();
        check(
            unsafe { sys::napi_get_property(self.raw_env(), raw(object), raw(&key), &mut result) },
            "read object property",
        )?;
        Ok(self.value_from_raw(result))
    }

    fn set_property(
        &mut self,
        object: &Self::Value,
        name: &str,
        value: &Self::Value,
    ) -> Result<(), JsError> {
        let key = self.string(name)?;
        check(
            unsafe { sys::napi_set_property(self.raw_env(), raw(object), raw(&key), raw(value)) },
            "write object property",
        )
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), JsError> {
        let global = self.env.get_global().map_err(js_error)?.to_unknown();
        self.set_property(&global, name, value)
    }

    fn define_function(
        &mut self,
        name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<Self::Value, JsError> {
        let callback = RefCell::new(callback);
        let env = self.raw_env();
        let value = self
            .env
            .create_function_from_closure::<Unknown<'static>, Unknown<'static>, _>(
                name,
                move |context| {
                    let this = context.this::<Unknown<'static>>()?;
                    let arguments = context.arguments::<Unknown<'static>>()?;
                    let external = external_from_raw(context.env.raw(), this.raw()).ok();
                    callback.borrow_mut()(NativeCall {
                        this,
                        arguments,
                        external,
                    })
                    .map_err(napi_error)
                },
            )
            .map_err(js_error)?;
        Ok(unknown(env, value.raw()))
    }

    fn call(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError> {
        let receiver = this.copied().unwrap_or_else(|| self.undefined());
        let argument_values: Vec<_> = arguments.iter().map(raw).collect();
        let mut result = ptr::null_mut();
        check_call(
            self.raw_env(),
            unsafe {
                sys::napi_call_function(
                    self.raw_env(),
                    raw(&receiver),
                    raw(function),
                    argument_values.len(),
                    argument_values.as_ptr(),
                    &mut result,
                )
            },
            "call JavaScript function",
        )?;
        Ok(self.value_from_raw(result))
    }

    fn register_class(
        &mut self,
        definition: NativeClass<Self::Value>,
    ) -> Result<Self::Class, JsError> {
        let constructor = self.define_function(&definition.name, Box::new(|call| Ok(call.this)))?;
        let prototype = self.get_property(&constructor, "prototype")?;
        for method in definition.methods {
            let function = self.define_function(&method.name, method.callback)?;
            self.set_property(&prototype, &method.name, &function)?;
        }
        let mut reference = ptr::null_mut();
        check(
            unsafe {
                sys::napi_create_reference(self.raw_env(), raw(&constructor), 1, &mut reference)
            },
            "retain native class",
        )?;
        Ok(NodeClass {
            env: self.raw_env(),
            reference,
        })
    }

    fn instantiate(
        &mut self,
        class: &Self::Class,
        external: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    ) -> Result<Self::Value, JsError> {
        let mut instance = ptr::null_mut();
        check(
            unsafe {
                sys::napi_new_instance(
                    self.raw_env(),
                    class.value()?,
                    0,
                    ptr::null(),
                    &mut instance,
                )
            },
            "instantiate native class",
        )?;
        let data = Box::into_raw(Box::new(InstanceData {
            id: external,
            finalizer,
        }));
        let status = unsafe {
            sys::napi_wrap(
                self.raw_env(),
                instance,
                data.cast(),
                Some(finalize_instance),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if let Err(error) = check(status, "attach native instance data") {
            // SAFETY: napi_wrap rejected ownership of the allocation.
            drop(unsafe { Box::from_raw(data) });
            return Err(error);
        }
        Ok(self.value_from_raw(instance))
    }

    fn external_id(&mut self, value: &Self::Value) -> Result<ExternalId, JsError> {
        external_from_raw(self.raw_env(), raw(value))
    }

    fn detach_array_buffer(&mut self, buffer: &Self::Value) -> Result<(), JsError> {
        // Node-API refuses a value that is not an ArrayBuffer, so the status is
        // the check: a transfer list that quietly detached nothing is exactly
        // the failure this call exists to prevent.
        check(
            unsafe { sys::napi_detach_arraybuffer(self.raw_env(), raw(buffer)) },
            "detach an ArrayBuffer",
        )
    }

    fn downgrade(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        let mut reference = ptr::null_mut();
        check(
            unsafe { sys::napi_create_reference(self.raw_env(), raw(value), 0, &mut reference) },
            "create weak reference",
        )?;
        Ok(NodeWeakRef {
            env: self.raw_env(),
            reference,
        })
    }

    fn upgrade(&mut self, reference: &Self::WeakRef) -> Result<Option<Self::Value>, JsError> {
        let mut value = ptr::null_mut();
        check(
            unsafe {
                sys::napi_get_reference_value(self.raw_env(), reference.reference, &mut value)
            },
            "upgrade weak reference",
        )?;
        Ok((!value.is_null()).then(|| self.value_from_raw(value)))
    }

    fn evaluate_script(&mut self, source: &str, filename: &str) -> Result<Self::Value, JsError> {
        let source = format!("{source}\n//# sourceURL={filename}");
        // A document's own script goes through indirect eval, so its top-level
        // `const` lands in a scope that is thrown away with the document rather
        // than in the realm's permanent lexical scope — where a second load
        // would find it already declared. The runtime's own scripts are named
        // `blitsen:<something>` with no authority, and are not documents; the
        // application origin is `blitsen://app/…`, and is.
        let source = if uses_document_script_scope(filename) {
            let source =
                serde_json::to_string(&source).map_err(|error| JsError::new(error.to_string()))?;
            format!("(0, eval)({source})")
        } else {
            source
        };
        self.env
            .run_script::<_, Unknown<'static>>(source)
            .map_err(js_error)
    }

    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        // A module is named by its application URL on both hosts (#126), and
        // this host's module loader is Bun's, which resolves one module's import
        // of the next against the filesystem. So the URL is turned back into the
        // path behind it before the loader sees it; an application with no path
        // behind it — a bundle — never reaches this host, which cannot run one.
        let on_disk = application_path(identifier);
        let path = on_disk.as_deref().unwrap_or_else(|| Path::new(identifier));
        let loader_base = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(document) = application_document_path(identifier) {
            // An inline module: not a file, but its imports resolve against the
            // document that carried it, which is one.
            document
        } else {
            std::env::current_dir()
                .map_err(|error| JsError::new(error.to_string()))?
                .join("blitsen-inline-module.js")
        };
        let loader_base = serde_json::to_string(&loader_base.to_string_lossy())
            .map_err(|error| JsError::new(error.to_string()))?;
        if path.is_absolute() && path.is_file() {
            let specifier = serde_json::to_string(&path.to_string_lossy())
                .map_err(|error| JsError::new(error.to_string()))?;
            return self.evaluate_script(
                &format!(
                    "process.getBuiltinModule('module').createRequire({loader_base})({specifier})"
                ),
                "blitsen:external-module-loader",
            );
        }
        // An inline module's `import.meta.url` is the document's URL, exactly as
        // it is in a browser — not the `data:` URL Bun evaluates it under, which
        // `new URL('./sound.wav', import.meta.url)` resolves against and lands
        // nowhere useful. That was what stopped an application loading its own
        // files from an inline script (issue #125).
        //
        // Assigned in the module body rather than arranged through the loader,
        // because Bun chooses the specifier and the specifier is what it derives
        // `import.meta.url` from. `import.meta` is an ordinary extensible object
        // whose `url` the HTML specification defines as writable, so the
        // assignment is the whole of it; it is guarded anyway, because an engine
        // that disagreed should not take the document down with it.
        let source = match document_url(identifier) {
            Some(url) => {
                let url =
                    serde_json::to_string(&url).map_err(|error| JsError::new(error.to_string()))?;
                format!("try {{ import.meta.url = {url}; }} catch {{}}\n{source}")
            }
            None => source.to_owned(),
        };
        let source = format!("{source}\n//# sourceURL={identifier}");
        let source =
            serde_json::to_string(&source).map_err(|error| JsError::new(error.to_string()))?;
        // Bun accepts a short `data:` module through `createRequire`, but starts
        // treating a production-sized one as a package path and asks the
        // filesystem to resolve its entire base64 body. Linux then answers
        // `NameTooLong` before a byte of the application runs. A Blob URL is
        // still an engine-owned module with no temporary file, and Bun's
        // createRequire evaluates it synchronously, preserving document order.
        self.evaluate_script(
            &format!(
                // The host's own `URL`, not the application's: the DOM bootstrap
                // installs Blitsen's `URL` over whatever the host supplied, and
                // Blitsen's has no object URLs — there is no origin behind an
                // application to hang a `blob:` on. The bootstrap keeps the
                // host's class aside for exactly this, because Bun's is a global
                // and not on `node:url`.
                "(() => {{ \
                   const NativeBlob = process.getBuiltinModule('buffer').Blob; \
                   const NativeURL = globalThis.__blitsenHostUrl ?? URL; \
                   const url = NativeURL.createObjectURL(new NativeBlob([{source}], \
                     {{ type: 'text/javascript' }})); \
                   try {{ \
                     return process.getBuiltinModule('module') \
                       .createRequire({loader_base})(url); \
                   }} finally {{ NativeURL.revokeObjectURL(url); }} \
                 }})()"
            ),
            identifier,
        )
    }

    fn drain_microtasks(&mut self) -> Result<usize, JsError> {
        // Bun drains its microtask queue when control returns from the addon.
        // Node-API intentionally exposes no nested checkpoint operation.
        Ok(0)
    }

    fn pump_event_loop(&mut self) -> Result<LoopTurn, JsError> {
        // Bun is the outer event loop (S1). It invokes the addon again for the
        // next turn; attempting to call uv_run from Bun aborts.
        Ok(LoopTurn::Idle)
    }
}

/// Whether a classic script needs the disposable document-level lexical scope.
fn uses_document_script_scope(identifier: &str) -> bool {
    let internal = identifier.starts_with("blitsen:") && !identifier.starts_with(APP_ORIGIN);
    !internal
        && (identifier.starts_with(APP_ORIGIN)
            || Path::new(identifier).is_absolute()
            || parse_inline_script_identifier(identifier).is_some())
}

/// The file an application URL names, when the application is on disk.
///
/// `blitsen://app/assets/app.js` is where an application's own files live, and a
/// directory being run is the shape where each of them is also a real file. The
/// fragment an inline script carries is kept out of the answer by refusing it:
/// `index.html#script-2` is not a file, and treating it as one would evaluate
/// the document as a module.
fn application_path(identifier: &str) -> Option<PathBuf> {
    let relative = identifier.strip_prefix(blitsen_host::modules::APP_ORIGIN)?;
    if relative.contains('#') || relative.contains('?') {
        return None;
    }
    let root = blitsen_host::app::application_root()?;
    let path = root.join(relative);
    path.is_file().then_some(path)
}

/// The document an inline script's identifier names, on disk.
///
/// `blitsen://app/index.html#script-2` is not a file; `index.html` beside it is,
/// and it is what an `import` inside that script resolves against.
fn application_document_path(identifier: &str) -> Option<PathBuf> {
    let (document, _) = parse_inline_script_identifier(identifier)?;
    let relative = document.strip_prefix(blitsen_host::modules::APP_ORIGIN)?;
    let document = relative.split(['#', '?']).next().unwrap_or_default();
    if document.is_empty() {
        return None;
    }
    let path = blitsen_host::app::application_root()?.join(document);
    path.is_file().then_some(path)
}

/// The URL an inline script's identifier names, for this host.
///
/// Identifiers arrive as `<entrypoint>#script-<n>` on the application origin.
/// This host answers with the `file:` URL of the file behind it, because that
/// is the origin everything else it evaluates is on: Bun's loader is the
/// filesystem's, so an external module is named by its real path, and
/// `createRequire(import.meta.url)` — the documented way to reach a `.node`
/// addon — needs a file URL. One host, one kind of URL.
///
/// Phase 2 answers `blitsen://app/…` for the same document, and the two are
/// interchangeable where it matters: both are absolute, both resolve a relative
/// asset to a sibling, and `fetch` reads either out of the application (#126).
///
/// The fragment is kept. It is what makes one inline module distinct from the
/// next, which a module registry keyed by URL needs, and it costs nothing to
/// resolve against.
fn document_url(identifier: &str) -> Option<String> {
    if identifier.is_empty() {
        return None;
    }
    let (entrypoint, fragment) =
        parse_inline_script_identifier(identifier).unwrap_or((identifier, ""));
    let on_disk = application_document_path(identifier).or_else(|| {
        Path::new(entrypoint)
            .is_absolute()
            .then(|| PathBuf::from(entrypoint))
    });
    match on_disk {
        // A real file URL gives relative imports and asset references the
        // filesystem base the Node/Bun module loader expects.
        Some(path) => document_file_url(&path, entrypoint, fragment),
        // A bundle: no file behind it, so the application URL is the address.
        None => identifier.contains("://").then(|| identifier.to_owned()),
    }
}

/// Translates an on-disk document identity into the URL exposed to JavaScript.
///
/// Paths already cross this host's Node-API boundary as lossy UTF-8 strings;
/// retain that policy here, then leave file URL authority, drive and path
/// encoding rules to the standards implementation. The application query and
/// inline-script fragment are URL syntax rather than path bytes, so they are
/// restored only after the path has been converted.
fn document_file_url(path: &Path, entrypoint: &str, fragment: &str) -> Option<String> {
    let utf8_path = path.to_string_lossy();
    let url = Url::from_file_path(Path::new(utf8_path.as_ref())).ok()?;
    let query = entrypoint
        .strip_prefix(APP_ORIGIN)
        .and_then(|relative| relative.split_once('?').map(|(_, tail)| tail))
        .map_or(String::new(), |query| format!("?{query}"));
    Some(format!("{url}{query}{fragment}"))
}

pub(crate) fn external_from_raw(
    env: sys::napi_env,
    object: sys::napi_value,
) -> Result<ExternalId, JsError> {
    let mut data = ptr::null_mut();
    check(
        unsafe { sys::napi_unwrap(env, object, &mut data) },
        "read native instance data",
    )?;
    if data.is_null() {
        return Err(JsError::new("object has no native instance data"));
    }
    // SAFETY: successful napi_unwrap returns the InstanceData pointer supplied
    // by this module's instantiate method.
    Ok(unsafe { (*data.cast::<InstanceData>()).id })
}

pub(crate) fn typed_array_type(kind: TypedArrayKind) -> sys::napi_typedarray_type {
    match kind {
        TypedArrayKind::Int8 => sys::TypedarrayType::int8_array,
        TypedArrayKind::Uint8 => sys::TypedarrayType::uint8_array,
        TypedArrayKind::Uint8Clamped => sys::TypedarrayType::uint8_clamped_array,
        TypedArrayKind::Int16 => sys::TypedarrayType::int16_array,
        TypedArrayKind::Uint16 => sys::TypedarrayType::uint16_array,
        TypedArrayKind::Int32 => sys::TypedarrayType::int32_array,
        TypedArrayKind::Uint32 => sys::TypedarrayType::uint32_array,
        TypedArrayKind::Float32 => sys::TypedarrayType::float32_array,
        TypedArrayKind::Float64 => sys::TypedarrayType::float64_array,
        TypedArrayKind::BigInt64 => sys::TypedarrayType::bigint64_array,
        TypedArrayKind::BigUint64 => sys::TypedarrayType::biguint64_array,
    }
}

pub(crate) fn from_typed_array_type(
    kind: sys::napi_typedarray_type,
) -> Result<TypedArrayKind, JsError> {
    match kind {
        sys::TypedarrayType::int8_array => Ok(TypedArrayKind::Int8),
        sys::TypedarrayType::uint8_array => Ok(TypedArrayKind::Uint8),
        sys::TypedarrayType::uint8_clamped_array => Ok(TypedArrayKind::Uint8Clamped),
        sys::TypedarrayType::int16_array => Ok(TypedArrayKind::Int16),
        sys::TypedarrayType::uint16_array => Ok(TypedArrayKind::Uint16),
        sys::TypedarrayType::int32_array => Ok(TypedArrayKind::Int32),
        sys::TypedarrayType::uint32_array => Ok(TypedArrayKind::Uint32),
        sys::TypedarrayType::float32_array => Ok(TypedArrayKind::Float32),
        sys::TypedarrayType::float64_array => Ok(TypedArrayKind::Float64),
        sys::TypedarrayType::bigint64_array => Ok(TypedArrayKind::BigInt64),
        sys::TypedarrayType::biguint64_array => Ok(TypedArrayKind::BigUint64),
        other => Err(JsError::new(format!(
            "unknown Node-API typed-array kind {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use blitsen_core::inline_script_identifier;

    use super::{document_file_url, document_url, uses_document_script_scope};

    /// Issue #125: an inline module's `import.meta.url` is the document's URL,
    /// as it is in a browser, so `new URL('./x', import.meta.url)` names a file
    /// the application shipped rather than resolving against a `data:` URL.
    #[test]
    fn an_inline_scripts_identifier_names_the_document_it_came_from() {
        // Absolute means absolute *here*, and a URL path is not a Windows path:
        // `/tmp/app` is not an absolute path on Windows, and `C:\app` is not a
        // URL until the drive letter is rooted and the separators turned round.
        let (document, url) = if cfg!(windows) {
            (r"C:\app\index.html", "file:///C:/app/index.html")
        } else {
            ("/tmp/app/index.html", "file:///tmp/app/index.html")
        };
        // The fragment stays: it is what makes one inline module distinct from
        // the next, which a module registry keyed by URL needs, and resolving a
        // relative asset against it lands in the same place either way (#126).
        assert_eq!(
            document_url(&format!("{document}#script-1")),
            Some(format!("{url}#script-1"))
        );
        // An application URL with no file behind it — a bundle — is already a
        // URL and is answered as it stands.
        assert_eq!(
            document_url("blitsen://app/index.html#script-2").as_deref(),
            Some("blitsen://app/index.html#script-2")
        );
        // The same escaping the document's base URL uses, so a relative
        // resolution from a script and one from the document agree.
        let (spaced, spaced_url) = if cfg!(windows) {
            (r"C:\my app\index.html", "file:///C:/my%20app/index.html")
        } else {
            ("/tmp/my app/index.html", "file:///tmp/my%20app/index.html")
        };
        assert_eq!(
            document_url(&format!("{spaced}#script-1")),
            Some(format!("{spaced_url}#script-1"))
        );
        // Nothing to name, rather than a guess: the harness evaluates modules
        // under identifiers that address no document at all.
        assert_eq!(document_url("blitsen:inline"), None);
        assert_eq!(document_url(""), None);
    }

    #[test]
    fn only_a_trailing_inline_fragment_selects_document_scope() {
        assert!(uses_document_script_scope(&inline_script_identifier(
            "relative/index.html",
            1
        )));
        assert!(!uses_document_script_scope("relative/#script-1/library.js"));
        assert!(!uses_document_script_scope("relative/library#script-1.js"));
        assert!(!uses_document_script_scope("blitsen:runtime#script-1"));
        assert!(uses_document_script_scope("blitsen://app/assets/app.js"));
    }

    #[test]
    fn document_urls_encode_every_path_segment_before_the_inline_fragment() {
        let (document, expected) = if cfg!(windows) {
            (
                r"C:\50% off#archive?\café.html",
                "file:///C:/50%25%20off%23archive%3F/caf%C3%A9.html#script-3",
            )
        } else {
            (
                "/tmp/50% off#archive?/café.html",
                "file:///tmp/50%25%20off%23archive%3F/caf%C3%A9.html#script-3",
            )
        };
        assert_eq!(
            document_url(&inline_script_identifier(document, 3)).as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn an_application_query_stays_between_the_file_path_and_inline_fragment() {
        let (path, expected) = if cfg!(windows) {
            (
                Path::new(r"C:\app\index.html"),
                "file:///C:/app/index.html?theme=dark&build=42#script-7",
            )
        } else {
            (
                Path::new("/tmp/app/index.html"),
                "file:///tmp/app/index.html?theme=dark&build=42#script-7",
            )
        };
        assert_eq!(
            document_file_url(
                path,
                "blitsen://app/index.html?theme=dark&build=42",
                "#script-7",
            )
            .as_deref(),
            Some(expected)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_keep_the_hosts_lossy_javascript_identifier_policy() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::path::PathBuf;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/app-\x80/index.html".to_vec()));
        assert_eq!(
            document_file_url(&path, "", "#script-1").as_deref(),
            Some("file:///tmp/app-%EF%BF%BD/index.html#script-1")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_and_unc_paths_use_file_url_platform_rules() {
        assert_eq!(
            document_file_url(Path::new(r"C:\my app\index.html"), "", "#script-1").as_deref(),
            Some("file:///C:/my%20app/index.html#script-1")
        );
        assert_eq!(
            document_file_url(
                Path::new(r"\\server\share name\index.html"),
                "",
                "#script-2"
            )
            .as_deref(),
            Some("file://server/share%20name/index.html#script-2")
        );
    }
}
