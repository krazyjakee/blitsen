//! S8 — `JsEngine` over QuickJS-ng, to price the engine swap.
//!
//! The question this exists to answer is not "does QuickJS work" but "what does
//! Blitsen's own `JsEngine` contract cost on an engine with no JIT, no ICU and
//! no LGPL obligation". So this implements the real trait rather than a
//! convenient subset, and the harness in `main.rs` measures the result against
//! the numbers `spikes/s0` recorded for JavaScriptCore.
//!
//! Three parts of the contract drive the design:
//!
//! * `Value: Clone` with no lifetime. QuickJS values are reference counted, so
//!   the handle owns a count and `Drop` gives it back. It also carries its
//!   `JSContext`, which is what makes the next point possible.
//! * `from_value` re-enters the engine from any value a callback was handed.
//!   The context pointer is in the handle and the engine state hangs off the
//!   context's opaque slot, so the engine is recoverable without capturing it.
//! * `instantiate` attaches an [`ExternalId`] plus a finalizer that must run
//!   exactly once. That is a QuickJS class with an opaque payload.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::rc::Rc;

use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    TypedArray, TypedArrayKind,
};
use rquickjs_sys as q;

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// An owned QuickJS value handle.
///
/// Carries the context because the trait's `from_value` has to rebuild the
/// engine from nothing else, and because freeing a value needs it anyway.
pub struct QjsValue {
    ctx: *mut q::JSContext,
    raw: q::JSValue,
}

impl QjsValue {
    /// Takes ownership of a value the engine just returned.
    ///
    /// # Safety
    /// `raw` must be an owned reference produced by `ctx`.
    unsafe fn own(ctx: *mut q::JSContext, raw: q::JSValue) -> Self {
        Self { ctx, raw }
    }
}

impl Clone for QjsValue {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx,
            raw: unsafe { q::JS_DupValue(self.ctx, self.raw) },
        }
    }
}

impl Drop for QjsValue {
    fn drop(&mut self) {
        unsafe { q::JS_FreeValue(self.ctx, self.raw) };
    }
}

/// A weak reference, held as the JavaScript `WeakRef` the engine understands.
///
/// Implemented in JavaScript rather than in the C API on purpose: QuickJS-ng
/// ships `WeakRef`, and borrowing the language's own semantics means the
/// collector decides liveness rather than this file guessing at it.
pub struct QjsWeakRef {
    reference: QjsValue,
}

/// A registered native class.
#[derive(Clone)]
pub struct QjsClass {
    id: q::JSClassID,
    constructor: QjsValue,
}

// ---------------------------------------------------------------------------
// Engine state
// ---------------------------------------------------------------------------

/// Boxed callback plus the engine it belongs to, stored as class opaque data.
struct CallbackData {
    callback: RefCell<NativeCallback<QjsValue>>,
}

/// Per-instance payload attached by [`JsEngine::instantiate`].
struct InstanceData {
    external: ExternalId,
    finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
}

struct Inner {
    runtime: *mut q::JSRuntime,
    context: *mut q::JSContext,
    /// Class for objects whose opaque is a [`CallbackData`].
    callback_class: q::JSClassID,
    /// Class for objects whose opaque is an [`InstanceData`].
    instance_class: q::JSClassID,
    /// True for the handle that created the runtime; borrowed handles from
    /// `from_value` must not tear it down.
    owner: bool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if !self.owner {
            return;
        }
        unsafe {
            q::JS_FreeContext(self.context);
            q::JS_FreeRuntime(self.runtime);
        }
    }
}

/// A QuickJS engine implementing Blitsen's host boundary.
pub struct QuickJs {
    inner: Rc<Inner>,
}

/// What lives in the context's opaque slot, so `from_value` can find it.
struct ContextState {
    callback_class: q::JSClassID,
    instance_class: q::JSClassID,
    runtime: *mut q::JSRuntime,
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
unsafe extern "C" fn invoke_callback(
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

unsafe fn context_state<'a>(ctx: *mut q::JSContext) -> &'a ContextState {
    let opaque = unsafe { q::JS_GetContextOpaque(ctx) };
    debug_assert!(!opaque.is_null(), "context has no Blitsen state");
    unsafe { &*opaque.cast::<ContextState>() }
}

unsafe fn throw(ctx: *mut q::JSContext, message: &str) -> q::JSValue {
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

    fn ctx(&self) -> *mut q::JSContext {
        self.inner.context
    }

    /// Turns an exception pending on the context into a [`JsError`].
    fn exception(&self) -> JsError {
        unsafe {
            let ctx = self.ctx();
            let value = q::JS_GetException(ctx);
            let message = self.text(value).unwrap_or_else(|| "unknown error".to_owned());
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
    fn text(&self, value: q::JSValue) -> Option<String> {
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
    fn checked(&self, raw: q::JSValue) -> Result<QjsValue, JsError> {
        if unsafe { q::JS_IsException(raw) } {
            Err(self.exception())
        } else {
            Ok(unsafe { QjsValue::own(self.ctx(), raw) })
        }
    }

    fn global(&self) -> QjsValue {
        unsafe { QjsValue::own(self.ctx(), q::JS_GetGlobalObject(self.ctx())) }
    }

    /// Compiles source to QuickJS bytecode, for the build-time-compile question.
    ///
    /// This is the `qjsc` path through the public C API: compile only, then
    /// serialize the resulting function object. `evaluate_bytecode` is its
    /// other half, and between them an export can ship compiled code and no
    /// parser input at all.
    pub fn compile(&mut self, source: &str, filename: &str, module: bool) -> Result<Vec<u8>, JsError> {
        unsafe {
            let code = CString::new(source).map_err(|_| JsError::new("source contains a NUL"))?;
            let name = CString::new(filename).map_err(|_| JsError::new("filename contains a NUL"))?;
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
            if tag == q::JS_TAG_MODULE {
                if q::JS_ResolveModule(self.ctx(), object) < 0 {
                    q::JS_FreeValue(self.ctx(), object);
                    return Err(self.exception());
                }
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

/// The JavaScript constructor that builds each kind.
const fn typed_array_constructor(kind: TypedArrayKind) -> &'static str {
    match kind {
        TypedArrayKind::Int8 => "Int8Array",
        TypedArrayKind::Uint8 => "Uint8Array",
        TypedArrayKind::Uint8Clamped => "Uint8ClampedArray",
        TypedArrayKind::Int16 => "Int16Array",
        TypedArrayKind::Uint16 => "Uint16Array",
        TypedArrayKind::Int32 => "Int32Array",
        TypedArrayKind::Uint32 => "Uint32Array",
        TypedArrayKind::Float32 => "Float32Array",
        TypedArrayKind::Float64 => "Float64Array",
        TypedArrayKind::BigInt64 => "BigInt64Array",
        TypedArrayKind::BigUint64 => "BigUint64Array",
    }
}

const TYPED_ARRAY_KINDS: [(TypedArrayKind, q::JSTypedArrayEnum); 11] = [
    (TypedArrayKind::Int8, q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT8),
    (TypedArrayKind::Uint8, q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8),
    (
        TypedArrayKind::Uint8Clamped,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8C,
    ),
    (TypedArrayKind::Int16, q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT16),
    (
        TypedArrayKind::Uint16,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT16,
    ),
    (TypedArrayKind::Int32, q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT32),
    (
        TypedArrayKind::Uint32,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT32,
    ),
    (
        TypedArrayKind::Float32,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT32,
    ),
    (
        TypedArrayKind::Float64,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT64,
    ),
    (
        TypedArrayKind::BigInt64,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_INT64,
    ),
    (
        TypedArrayKind::BigUint64,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_UINT64,
    ),
];

impl JsEngine for QuickJs {
    type Value = QjsValue;
    type WeakRef = QjsWeakRef;
    type Class = QjsClass;

    fn from_value(value: &Self::Value) -> Self {
        let ctx = value.ctx;
        let state = unsafe { context_state(ctx) };
        // A borrowed handle: it shares the live context but must not free it,
        // because the value that produced it is owned by the engine already.
        Self {
            inner: Rc::new(Inner {
                runtime: state.runtime,
                context: ctx,
                callback_class: state.callback_class,
                instance_class: state.instance_class,
                owner: false,
            }),
        }
    }

    fn undefined(&mut self) -> Self::Value {
        unsafe { QjsValue::own(self.ctx(), q::JS_UNDEFINED) }
    }

    fn null(&mut self) -> Self::Value {
        unsafe { QjsValue::own(self.ctx(), q::JS_NULL) }
    }

    fn boolean(&mut self, value: bool) -> Self::Value {
        unsafe { QjsValue::own(self.ctx(), if value { q::JS_TRUE } else { q::JS_FALSE }) }
    }

    fn number(&mut self, value: f64) -> Self::Value {
        unsafe { QjsValue::own(self.ctx(), q::JS_NewFloat64(value)) }
    }

    fn string(&mut self, value: &str) -> Result<Self::Value, JsError> {
        unsafe {
            let raw = q::JS_NewStringLen(
                self.ctx(),
                value.as_ptr().cast::<c_char>(),
                value.len() as q::size_t,
            );
            self.checked(raw)
        }
    }

    fn object(&mut self) -> Result<Self::Value, JsError> {
        let raw = unsafe { q::JS_NewObject(self.ctx()) };
        self.checked(raw)
    }

    fn array(&mut self, values: &[Self::Value]) -> Result<Self::Value, JsError> {
        let array = self.checked(unsafe { q::JS_NewArray(self.ctx()) })?;
        for (index, value) in values.iter().enumerate() {
            let raw = unsafe { q::JS_DupValue(self.ctx(), value.raw) };
            let set = unsafe {
                q::JS_SetPropertyUint32(self.ctx(), array.raw, index as u32, raw)
            };
            if set < 0 {
                return Err(self.exception());
            }
        }
        Ok(array)
    }

    fn typed_array(&mut self, value: &TypedArray) -> Result<Self::Value, JsError> {
        let kind = TYPED_ARRAY_KINDS
            .iter()
            .find(|(kind, _)| *kind == value.kind)
            .map(|(_, raw)| *raw)
            .ok_or_else(|| JsError::new("unsupported typed array kind"))?;
        let _ = kind;
        // Built through the JavaScript constructor rather than JS_NewTypedArray:
        // the C helper returned a zero-length view over a correct 8-byte buffer
        // on quickjs-ng 0.12, and the language's own constructor is both the
        // semantics this trait is specified against and the one the collector,
        // the prototype chain and `byteOffset` all already agree with.
        let buffer = unsafe {
            let raw = q::JS_NewArrayBufferCopy(
                self.ctx(),
                value.bytes.as_ptr(),
                value.bytes.len() as q::size_t,
            );
            self.checked(raw)?
        };
        let global = self.global();
        let constructor = self.get_property(&global, typed_array_constructor(value.kind))?;
        let raw = unsafe {
            let mut argv = [buffer.raw];
            q::JS_CallConstructor(self.ctx(), constructor.raw, 1, argv.as_mut_ptr())
        };
        self.checked(raw)
    }

    fn value_type(&mut self, value: &Self::Value) -> Result<JsType, JsError> {
        unsafe {
            let raw = value.raw;
            if q::JS_IsUndefined(raw) {
                return Ok(JsType::Undefined);
            }
            if q::JS_IsNull(raw) {
                return Ok(JsType::Null);
            }
            if q::JS_IsBool(raw) {
                return Ok(JsType::Boolean);
            }
            if q::JS_IsNumber(raw) {
                return Ok(JsType::Number);
            }
            if q::JS_IsString(raw) {
                return Ok(JsType::String);
            }
            if q::JS_IsFunction(self.ctx(), raw) {
                return Ok(JsType::Function);
            }
            if q::JS_IsArray(raw) {
                return Ok(JsType::Array);
            }
            if q::JS_GetTypedArrayType(raw) >= 0 {
                return Ok(JsType::TypedArray);
            }
            Ok(JsType::Object)
        }
    }

    fn to_boolean(&mut self, value: &Self::Value) -> Result<bool, JsError> {
        let result = unsafe { q::JS_ToBool(self.ctx(), value.raw) };
        if result < 0 {
            return Err(self.exception());
        }
        Ok(result != 0)
    }

    fn to_number(&mut self, value: &Self::Value) -> Result<f64, JsError> {
        let mut out = 0.0;
        if unsafe { q::JS_ToFloat64(self.ctx(), &mut out, value.raw) } < 0 {
            return Err(self.exception());
        }
        Ok(out)
    }

    fn to_string(&mut self, value: &Self::Value) -> Result<String, JsError> {
        self.text(value.raw)
            .ok_or_else(|| self.exception())
    }

    fn to_array(&mut self, value: &Self::Value) -> Result<Vec<Self::Value>, JsError> {
        let length = self.get_property(value, "length")?;
        let length = self.to_number(&length)? as usize;
        let mut out = Vec::with_capacity(length);
        for index in 0..length {
            let raw = unsafe { q::JS_GetPropertyUint32(self.ctx(), value.raw, index as u32) };
            out.push(self.checked(raw)?);
        }
        Ok(out)
    }

    fn to_typed_array(&mut self, value: &Self::Value) -> Result<TypedArray, JsError> {
        let raw_kind = unsafe { q::JS_GetTypedArrayType(value.raw) };
        if raw_kind < 0 {
            return Err(JsError::new("value is not a typed array"));
        }
        let kind = TYPED_ARRAY_KINDS
            .iter()
            .find(|(_, tag)| *tag == raw_kind as q::JSTypedArrayEnum)
            .map(|(kind, _)| *kind)
            .ok_or_else(|| JsError::new("unsupported typed array kind"))?;
        unsafe {
            let mut offset: q::size_t = 0;
            let mut length: q::size_t = 0;
            let mut element: q::size_t = 0;
            let buffer = q::JS_GetTypedArrayBuffer(
                self.ctx(),
                value.raw,
                &mut offset,
                &mut length,
                &mut element,
            );
            if q::JS_IsException(buffer) {
                return Err(self.exception());
            }
            let buffer = QjsValue::own(self.ctx(), buffer);
            let mut size: q::size_t = 0;
            let bytes = q::JS_GetArrayBuffer(self.ctx(), &mut size, buffer.raw);
            if bytes.is_null() {
                return Err(self.exception());
            }
            // The view is a window onto the buffer, so the copy starts where the
            // view does rather than at the buffer's own origin.
            let start = offset as usize;
            let end = start + length as usize;
            let slice = std::slice::from_raw_parts(bytes, size as usize);
            TypedArray::new(kind, slice[start..end].to_vec())
        }
    }

    fn get_property(&mut self, object: &Self::Value, name: &str) -> Result<Self::Value, JsError> {
        let key = CString::new(name).map_err(|_| JsError::new("property name contains a NUL"))?;
        let raw = unsafe { q::JS_GetPropertyStr(self.ctx(), object.raw, key.as_ptr()) };
        self.checked(raw)
    }

    fn set_property(
        &mut self,
        object: &Self::Value,
        name: &str,
        value: &Self::Value,
    ) -> Result<(), JsError> {
        let key = CString::new(name).map_err(|_| JsError::new("property name contains a NUL"))?;
        let raw = unsafe { q::JS_DupValue(self.ctx(), value.raw) };
        if unsafe { q::JS_SetPropertyStr(self.ctx(), object.raw, key.as_ptr(), raw) } < 0 {
            return Err(self.exception());
        }
        Ok(())
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), JsError> {
        let global = self.global();
        self.set_property(&global, name, value)
    }

    fn define_function(
        &mut self,
        name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<Self::Value, JsError> {
        unsafe {
            let holder = q::JS_NewObjectClass(self.ctx(), self.inner.callback_class);
            if q::JS_IsException(holder) {
                return Err(self.exception());
            }
            let data = Box::into_raw(Box::new(CallbackData {
                callback: RefCell::new(callback),
            }));
            q::JS_SetOpaque(holder, data.cast::<c_void>());

            let mut argv = [holder];
            let function = q::JS_NewCFunctionData(
                self.ctx(),
                Some(invoke_callback),
                0,
                0,
                1,
                argv.as_mut_ptr(),
            );
            q::JS_FreeValue(self.ctx(), holder);
            let function = self.checked(function)?;
            let named = CString::new(name).unwrap_or_else(|_| CString::new("native").unwrap());
            let text = q::JS_NewStringLen(
                self.ctx(),
                named.as_ptr(),
                name.len() as q::size_t,
            );
            q::JS_DefinePropertyValueStr(
                self.ctx(),
                function.raw,
                c"name".as_ptr(),
                text,
                q::JS_PROP_CONFIGURABLE as c_int,
            );
            Ok(function)
        }
    }

    fn call(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError> {
        let receiver = match this {
            Some(value) => value.raw,
            None => q::JS_UNDEFINED,
        };
        let mut argv: Vec<q::JSValue> = arguments.iter().map(|value| value.raw).collect();
        let raw = unsafe {
            q::JS_Call(
                self.ctx(),
                function.raw,
                receiver,
                argv.len() as c_int,
                argv.as_mut_ptr(),
            )
        };
        self.checked(raw)
    }

    fn register_class(
        &mut self,
        definition: NativeClass<Self::Value>,
    ) -> Result<Self::Class, JsError> {
        // One shared class id backs every native class: the trait's contract is
        // an opaque payload and a prototype of methods, and QuickJS gives the
        // prototype without needing a class id per definition.
        let prototype = self.object()?;
        for method in definition.methods {
            let function = self.define_function(&method.name, method.callback)?;
            self.set_property(&prototype, &method.name, &function)?;
        }
        let name = definition.name.clone();
        let constructor = self.define_function(&definition.name, Box::new(move |_call| {
            Err(JsError::new(format!("{name} is not constructible from JavaScript")))
        }))?;
        self.set_property(&constructor, "prototype", &prototype)?;
        Ok(QjsClass {
            id: self.inner.instance_class,
            constructor,
        })
    }

    fn instantiate(
        &mut self,
        class: &Self::Class,
        external: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    ) -> Result<Self::Value, JsError> {
        unsafe {
            let object = q::JS_NewObjectClass(self.ctx(), class.id);
            if q::JS_IsException(object) {
                return Err(self.exception());
            }
            let object = QjsValue::own(self.ctx(), object);
            let data = Box::into_raw(Box::new(InstanceData {
                external,
                finalizer,
            }));
            q::JS_SetOpaque(object.raw, data.cast::<c_void>());
            let prototype = self.get_property(&class.constructor, "prototype")?;
            let raw = q::JS_DupValue(self.ctx(), prototype.raw);
            if q::JS_SetPrototype(self.ctx(), object.raw, raw) < 0 {
                q::JS_FreeValue(self.ctx(), raw);
                return Err(self.exception());
            }
            q::JS_FreeValue(self.ctx(), raw);
            Ok(object)
        }
    }

    fn external_id(&mut self, value: &Self::Value) -> Result<ExternalId, JsError> {
        let opaque = unsafe { q::JS_GetOpaque(value.raw, self.inner.instance_class) };
        if opaque.is_null() {
            return Err(JsError::new("value carries no native external data"));
        }
        Ok(unsafe { &*opaque.cast::<InstanceData>() }.external)
    }

    fn downgrade(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        let global = self.global();
        let constructor = self.get_property(&global, "WeakRef")?;
        let raw = unsafe {
            let mut argv = [value.raw];
            q::JS_CallConstructor(self.ctx(), constructor.raw, 1, argv.as_mut_ptr())
        };
        Ok(QjsWeakRef {
            reference: self.checked(raw)?,
        })
    }

    fn upgrade(&mut self, reference: &Self::WeakRef) -> Result<Option<Self::Value>, JsError> {
        let deref = self.get_property(&reference.reference, "deref")?;
        let target = self.call(&deref, Some(&reference.reference), &[])?;
        match self.value_type(&target)? {
            JsType::Undefined => Ok(None),
            _ => Ok(Some(target)),
        }
    }

    fn evaluate_script(&mut self, source: &str, filename: &str) -> Result<Self::Value, JsError> {
        let code = CString::new(source).map_err(|_| JsError::new("source contains a NUL"))?;
        let name = CString::new(filename).map_err(|_| JsError::new("filename contains a NUL"))?;
        let raw = unsafe {
            q::JS_Eval(
                self.ctx(),
                code.as_ptr(),
                source.len() as q::size_t,
                name.as_ptr(),
                0,
            )
        };
        self.checked(raw)
    }

    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        let code = CString::new(source).map_err(|_| JsError::new("source contains a NUL"))?;
        let name =
            CString::new(identifier).map_err(|_| JsError::new("identifier contains a NUL"))?;
        let raw = unsafe {
            q::JS_Eval(
                self.ctx(),
                code.as_ptr(),
                source.len() as q::size_t,
                name.as_ptr(),
                q::JS_EVAL_TYPE_MODULE as c_int,
            )
        };
        self.checked(raw)
    }

    fn drain_microtasks(&mut self) -> Result<usize, JsError> {
        let mut ran = 0;
        loop {
            let mut context: *mut q::JSContext = std::ptr::null_mut();
            let status = unsafe { q::JS_ExecutePendingJob(self.inner.runtime, &mut context) };
            if status == 0 {
                return Ok(ran);
            }
            if status < 0 {
                return Err(self.exception());
            }
            ran += 1;
        }
    }

    fn pump_event_loop(&mut self) -> Result<LoopTurn, JsError> {
        // QuickJS has no host loop of its own: Blitsen owns the loop and this
        // is only the microtask checkpoint, which is exactly why an engine this
        // small is usable here at all.
        Ok(if self.drain_microtasks()? > 0 {
            LoopTurn::Progress
        } else {
            LoopTurn::Idle
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_array_round_trip_reports_what_it_saw() {
        let mut engine = QuickJs::new().unwrap();
        let source =
            TypedArray::new(TypedArrayKind::Float64, 8.0f64.to_ne_bytes().to_vec()).unwrap();
        let value = engine.typed_array(&source).unwrap();
        let kind_tag = unsafe { q::JS_GetTypedArrayType(value.raw) };
        let length = engine.get_property(&value, "length").unwrap();
        let byte_length = engine.get_property(&value, "byteLength").unwrap();
        eprintln!(
            "tag={kind_tag} length={:?} byteLength={:?} type={:?}",
            engine.to_number(&length),
            engine.to_number(&byte_length),
            engine.value_type(&value)
        );
        let back = engine.to_typed_array(&value).unwrap();
        eprintln!("source {:?} {:?}", source.kind, source.bytes);
        eprintln!("back   {:?} {:?}", back.kind, back.bytes);
        assert_eq!(back, source);
    }
}
