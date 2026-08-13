//! The runtime, the context, and the state reachable from a bare `JSContext`.
//!
//! The engine is recoverable from any value a callback was handed, so the two
//! class ids and the runtime pointer live in the context's opaque slot and are
//! read back through [`context_state`]. Everything that owns or tears down
//! QuickJS state is here; [`crate::engine`] is the trait implementation above
//! it.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_int, c_void};
use std::rc::Rc;

use blitsen_js::{ExternalId, JsError, NativeCall, NativeCallback};
use rquickjs_sys as q;

use crate::value::QjsValue;

/// Boxed callback plus the engine it belongs to, stored as class opaque data.
pub(crate) struct CallbackData {
    pub(crate) callback: RefCell<NativeCallback<QjsValue>>,
}

/// Per-instance payload attached by [`JsEngine::instantiate`].
pub(crate) struct InstanceData {
    pub(crate) external: ExternalId,
    pub(crate) finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
}

pub(crate) struct Inner {
    pub(crate) runtime: *mut q::JSRuntime,
    pub(crate) context: *mut q::JSContext,
    /// Class for objects whose opaque is a [`CallbackData`].
    pub(crate) callback_class: q::JSClassID,
    /// Class for objects whose opaque is an [`InstanceData`].
    pub(crate) instance_class: q::JSClassID,
    /// True for the handle that created the runtime; borrowed handles from
    /// `from_value` must not tear it down.
    pub(crate) owner: bool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The context is process-lived, which is the same call `blitsen-jsc`
        // makes for JavaScriptCore and for the same reason (S0: releasing the
        // global context asserts during teardown when the host still holds
        // values). Here the symptom is QuickJS's own
        // `JS_FreeRuntime: Assertion 'list_empty(&rt->gc_obj_list)' failed`,
        // observed at exit with the DOM wrapper cache still reachable — the
        // objects are *supposed* to be alive, because the process is ending
        // and nothing is going to look at them again. Freeing here would trade
        // a clean exit for an abort in front of the user; the OS reclaims this
        // memory either way. In-process restart would need this resolved.
        let _ = (self.owner, self.runtime, self.context);
    }
}

/// A QuickJS engine implementing Blitsen's host boundary.
pub struct QuickJs {
    pub(crate) inner: Rc<Inner>,
}

/// What lives in the context's opaque slot, so `from_value` can find it.
pub(crate) struct ContextState {
    pub(crate) callback_class: q::JSClassID,
    pub(crate) instance_class: q::JSClassID,
    pub(crate) runtime: *mut q::JSRuntime,
}

unsafe extern "C" fn finalize_callback(_rt: *mut q::JSRuntime, value: q::JSValue) {
    // The class id is recoverable from the value itself, which is what lets a
    // finalizer that has no context still find its own payload.
    let class_id = unsafe { q::JS_GetClassID(value) };
    let opaque = unsafe { q::JS_GetOpaque(value, class_id) };
    if !opaque.is_null() {
        drop(unsafe { Box::from_raw(opaque.cast::<CallbackData>()) });
    }
}

unsafe extern "C" fn finalize_instance(_rt: *mut q::JSRuntime, value: q::JSValue) {
    let class_id = unsafe { q::JS_GetClassID(value) };
    let opaque = unsafe { q::JS_GetOpaque(value, class_id) };
    if opaque.is_null() {
        return;
    }
    let data = unsafe { Box::from_raw(opaque.cast::<InstanceData>()) };
    // Exactly once, as the trait promises: the box is consumed here and the
    // opaque slot dies with the object.
    if let Some(finalizer) = data.finalizer {
        finalizer(data.external);
    }
}

/// The trampoline every native function goes through.
///
/// `data[0]` is an object whose opaque is the boxed Rust closure. Keeping the
/// closure on a JavaScript object rather than in a side table means its
/// lifetime is the function's lifetime, decided by the collector.
pub(crate) unsafe extern "C" fn invoke_callback(
    ctx: *mut q::JSContext,
    this_val: q::JSValue,
    argc: c_int,
    argv: *mut q::JSValue,
    _magic: c_int,
    data: *mut q::JSValue,
) -> q::JSValue {
    let state = unsafe { context_state(ctx) };
    let holder = unsafe { *data };
    let opaque = unsafe { q::JS_GetOpaque(holder, state.callback_class) };
    if opaque.is_null() {
        return unsafe { throw(ctx, "native callback is no longer available") };
    }
    let entry = unsafe { &*opaque.cast::<CallbackData>() };

    let this = unsafe { QjsValue::own(ctx, q::JS_DupValue(ctx, this_val)) };
    let external = unsafe { q::JS_GetOpaque(this_val, state.instance_class) };
    let external = if external.is_null() {
        None
    } else {
        Some(unsafe { &*external.cast::<InstanceData>() }.external)
    };
    let mut arguments = Vec::with_capacity(argc as usize);
    for index in 0..argc as isize {
        let raw = unsafe { *argv.offset(index) };
        arguments.push(unsafe { QjsValue::own(ctx, q::JS_DupValue(ctx, raw)) });
    }

    // A Rust panic must not unwind through QuickJS's C frames.
    let call = NativeCall {
        this,
        arguments,
        external,
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut callback = entry.callback.borrow_mut();
        (callback)(call)
    }));
    match outcome {
        Ok(Ok(value)) => {
            let raw = value.raw;
            std::mem::forget(value); // ownership moves to the caller
            raw
        }
        Ok(Err(error)) => unsafe { throw(ctx, error.message()) },
        Err(_) => unsafe { throw(ctx, "native callback panicked") },
    }
}

pub(crate) unsafe fn context_state<'a>(ctx: *mut q::JSContext) -> &'a ContextState {
    let opaque = unsafe { q::JS_GetContextOpaque(ctx) };
    debug_assert!(!opaque.is_null(), "context has no Blitsen state");
    unsafe { &*opaque.cast::<ContextState>() }
}

pub(crate) unsafe fn throw(ctx: *mut q::JSContext, message: &str) -> q::JSValue {
    let text = CString::new(message).unwrap_or_else(|_| CString::new("native error").unwrap());
    unsafe {
        let error = q::JS_NewError(ctx);
        let text = q::JS_NewStringLen(ctx, text.as_ptr(), message.len() as q::size_t);
        let key = c"message".as_ptr();
        q::JS_SetPropertyStr(ctx, error, key, text);
        q::JS_Throw(ctx, error)
    }
}

impl QuickJs {
    /// Creates a runtime, a context, and the two classes the trait needs.
    pub fn new() -> Result<Self, JsError> {
        unsafe {
            let runtime = q::JS_NewRuntime();
            if runtime.is_null() {
                return Err(JsError::new("QuickJS could not create a runtime"));
            }
            let context = q::JS_NewContext(runtime);
            if context.is_null() {
                q::JS_FreeRuntime(runtime);
                return Err(JsError::new("QuickJS could not create a context"));
            }

            let mut callback_class: q::JSClassID = 0;
            q::JS_NewClassID(runtime, &mut callback_class);
            let callback_def = q::JSClassDef {
                class_name: c"BlitsenNativeCallback".as_ptr(),
                finalizer: Some(finalize_callback),
                gc_mark: None,
                call: None,
                exotic: std::ptr::null_mut(),
            };
            q::JS_NewClass(runtime, callback_class, &callback_def);

            let mut instance_class: q::JSClassID = 0;
            q::JS_NewClassID(runtime, &mut instance_class);
            let instance_def = q::JSClassDef {
                class_name: c"BlitsenNativeObject".as_ptr(),
                finalizer: Some(finalize_instance),
                gc_mark: None,
                call: None,
                exotic: std::ptr::null_mut(),
            };
            q::JS_NewClass(runtime, instance_class, &instance_def);

            // Leaked on purpose: it must outlive every value that can reach it,
            // and the context is process-lived exactly as the JSC host's is.
            let state = Box::into_raw(Box::new(ContextState {
                callback_class,
                instance_class,
                runtime,
            }));
            q::JS_SetContextOpaque(context, state.cast::<c_void>());

            Ok(Self {
                inner: Rc::new(Inner {
                    runtime,
                    context,
                    callback_class,
                    instance_class,
                    owner: true,
                }),
            })
        }
    }

    pub(crate) fn ctx(&self) -> *mut q::JSContext {
        self.inner.context
    }

    /// Turns an exception pending on the context into a [`JsError`].
    pub(crate) fn exception(&self) -> JsError {
        unsafe {
            let ctx = self.ctx();
            let value = q::JS_GetException(ctx);
            let message = self
                .text(value)
                .unwrap_or_else(|| "unknown error".to_owned());
            let stack = q::JS_GetPropertyStr(ctx, value, c"stack".as_ptr());
            let stack_text = self.text(stack).filter(|text| !text.is_empty());
            q::JS_FreeValue(ctx, stack);
            q::JS_FreeValue(ctx, value);
            match stack_text {
                Some(stack) => JsError::with_stack(message, stack),
                None => JsError::new(message),
            }
        }
    }

    /// Reads a value as a Rust string without taking ownership of it.
    pub(crate) fn text(&self, value: q::JSValue) -> Option<String> {
        unsafe {
            let mut len: q::size_t = 0;
            let raw = q::JS_ToCStringLen2(self.ctx(), &mut len, value, false);
            if raw.is_null() {
                return None;
            }
            let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
            q::JS_FreeCString(self.ctx(), raw);
            Some(text)
        }
    }

    /// Wraps a raw result, converting a thrown exception into `Err`.
    pub(crate) fn checked(&self, raw: q::JSValue) -> Result<QjsValue, JsError> {
        if unsafe { q::JS_IsException(raw) } {
            Err(self.exception())
        } else {
            Ok(unsafe { QjsValue::own(self.ctx(), raw) })
        }
    }

    pub(crate) fn global(&self) -> QjsValue {
        unsafe { QjsValue::own(self.ctx(), q::JS_GetGlobalObject(self.ctx())) }
    }

    /// Compiles source to QuickJS bytecode, for the build-time-compile question.
    ///
    /// This is the `qjsc` path through the public C API: compile only, then
    /// serialize the resulting function object. `evaluate_bytecode` is its
    /// other half, and between them an export can ship compiled code and no
    /// parser input at all.
    pub fn compile(
        &mut self,
        source: &str,
        filename: &str,
        module: bool,
    ) -> Result<Vec<u8>, JsError> {
        unsafe {
            let code = CString::new(source).map_err(|_| JsError::new("source contains a NUL"))?;
            let name =
                CString::new(filename).map_err(|_| JsError::new("filename contains a NUL"))?;
            let mut flags = q::JS_EVAL_FLAG_COMPILE_ONLY;
            if module {
                flags |= q::JS_EVAL_TYPE_MODULE;
            }
            let compiled = q::JS_Eval(
                self.ctx(),
                code.as_ptr(),
                source.len() as q::size_t,
                name.as_ptr(),
                flags as c_int,
            );
            if q::JS_IsException(compiled) {
                return Err(self.exception());
            }
            let mut len: q::size_t = 0;
            let bytes = q::JS_WriteObject(
                self.ctx(),
                &mut len,
                compiled,
                q::JS_WRITE_OBJ_BYTECODE as c_int,
            );
            q::JS_FreeValue(self.ctx(), compiled);
            if bytes.is_null() {
                return Err(self.exception());
            }
            let out = std::slice::from_raw_parts(bytes, len as usize).to_vec();
            q::js_free(self.ctx(), bytes.cast::<c_void>());
            Ok(out)
        }
    }

    /// Runs code produced by [`QuickJs::compile`].
    pub fn evaluate_bytecode(&mut self, bytes: &[u8]) -> Result<QjsValue, JsError> {
        unsafe {
            let object = q::JS_ReadObject(
                self.ctx(),
                bytes.as_ptr(),
                bytes.len() as q::size_t,
                q::JS_READ_OBJ_BYTECODE as c_int,
            );
            if q::JS_IsException(object) {
                return Err(self.exception());
            }
            let tag = q::JS_VALUE_GET_NORM_TAG(object);
            if tag == q::JS_TAG_MODULE && q::JS_ResolveModule(self.ctx(), object) < 0 {
                q::JS_FreeValue(self.ctx(), object);
                return Err(self.exception());
            }
            let result = q::JS_EvalFunction(self.ctx(), object);
            self.checked(result)
        }
    }
}

impl Clone for QuickJs {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

/// An engine handle that shares the context without owning it.
pub(crate) unsafe fn borrowed(ctx: *mut q::JSContext) -> QuickJs {
    let state = unsafe { context_state(ctx) };
    QuickJs {
        inner: Rc::new(Inner {
            runtime: state.runtime,
            context: ctx,
            callback_class: state.callback_class,
            instance_class: state.instance_class,
            owner: false,
        }),
    }
}
