//! Bun-loadable Node-API addon and JavaScript-engine implementation.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr;

use base64::Engine as _;
use blitsen_core::WindowState;
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    NativeMethod, TypedArray, TypedArrayKind,
};
use napi::bindgen_prelude::{FromNapiValue, Unknown};
use napi::{Env, JsValue, Status, ValueType, sys};
use napi_derive::napi;

/// Stable addon name used by packaging and smoke tests.
pub const ADDON_NAME: &str = "blitsen-node";

fn js_error(error: napi::Error) -> JsError {
    JsError::new(error.reason)
}

fn napi_error(error: JsError) -> napi::Error {
    napi::Error::new(Status::GenericFailure, error.to_string())
}

fn check(status: sys::napi_status, operation: &str) -> Result<(), JsError> {
    if status == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(JsError::new(format!(
            "{operation} failed with Node-API status {status}"
        )))
    }
}

fn check_call(
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

fn unknown(env: sys::napi_env, value: sys::napi_value) -> Unknown<'static> {
    // SAFETY: every value passed here was returned by Node-API for this env.
    unsafe { Unknown::from_raw_unchecked(env, value) }
}

fn raw(value: &Unknown<'static>) -> sys::napi_value {
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

struct InstanceData {
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
pub struct NodeApiEngine {
    env: Env,
}

impl NodeApiEngine {
    /// Creates an engine view for the current addon environment.
    pub fn new(env: Env) -> Self {
        Self { env }
    }

    fn raw_env(&self) -> sys::napi_env {
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
        self.env
            .run_script::<_, Unknown<'static>>(source)
            .map_err(js_error)
    }

    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        let path = Path::new(identifier);
        if path.is_absolute() && path.is_file() {
            let specifier = format!("file://{}", identifier.replace(' ', "%20"));
            return self.evaluate_script(
                &format!("import({specifier:?})"),
                "blitsen:external-module-loader",
            );
        }
        let source = format!("{source}\n//# sourceURL={identifier}");
        let encoded = base64::engine::general_purpose::STANDARD.encode(source);
        self.evaluate_script(
            &format!("import(\"data:text/javascript;base64,{encoded}\")"),
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

fn external_from_raw(env: sys::napi_env, object: sys::napi_value) -> Result<ExternalId, JsError> {
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

fn typed_array_type(kind: TypedArrayKind) -> sys::napi_typedarray_type {
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

fn from_typed_array_type(kind: sys::napi_typedarray_type) -> Result<TypedArrayKind, JsError> {
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

/// JavaScript-facing engine owner loaded by `new Engine()`.
#[napi]
pub struct Engine {
    runtime: RefCell<NodeApiEngine>,
}

#[napi]
impl Engine {
    /// Creates an engine in the current Bun/Node-API environment.
    #[napi(constructor)]
    pub fn new(env: Env) -> Self {
        Self {
            runtime: RefCell::new(NodeApiEngine::new(env)),
        }
    }

    /// Loads an HTML file from disk and returns its source for the document
    /// loader added by the following milestone issues.
    #[napi(js_name = "loadHTML")]
    pub fn load_html(&self, path: String) -> napi::Result<String> {
        let path = Path::new(&path);
        let source = std::fs::read_to_string(path).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not read {}: {error}", path.display()),
            )
        })?;
        // Exercise the owned runtime here so constructor state is not merely
        // decorative; document installation follows in issues #23 and #24.
        let _ = self.runtime.borrow_mut().undefined();
        Ok(source)
    }
}

/// Runs the load-bearing Node-API subset used by the trait implementation.
///
/// This is exported for the Bun compatibility test and is not public package API.
#[napi]
pub fn node_api_smoke(env: Env) -> napi::Result<bool> {
    let mut engine = NodeApiEngine::new(env);
    let string = engine.string("42").map_err(napi_error)?;
    if engine.to_number(&string).map_err(napi_error)? != 42.0 {
        return Ok(false);
    }
    let one = engine.number(1.0);
    let two = engine.number(2.0);
    let array = engine.array(&[one, two]).map_err(napi_error)?;
    if engine.to_array(&array).map_err(napi_error)?.len() != 2 {
        return Ok(false);
    }
    let typed = TypedArray::new(TypedArrayKind::Uint8, vec![1, 2, 3]).map_err(napi_error)?;
    let typed = engine.typed_array(&typed).map_err(napi_error)?;
    if engine.to_typed_array(&typed).map_err(napi_error)?.bytes != [1, 2, 3] {
        return Ok(false);
    }
    let result = engine
        .evaluate_script("21 * 2", "smoke.js")
        .and_then(|value| engine.to_number(&value))
        .map_err(napi_error)?;
    if result != 42.0 {
        return Ok(false);
    }

    let identity = engine
        .define_function("identity", Box::new(|call| Ok(call.arguments[0])))
        .map_err(napi_error)?;
    let argument = engine.string("callback").map_err(napi_error)?;
    let result = engine
        .call(&identity, None, &[argument])
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    if result != "callback" {
        return Ok(false);
    }

    let class = engine
        .register_class(NativeClass::new("SmokeNode").with_method(NativeMethod::new(
            "identity",
            Box::new(|call| Ok(call.this)),
        )))
        .map_err(napi_error)?;
    let instance = engine
        .instantiate(&class, ExternalId(42), None)
        .map_err(napi_error)?;
    if engine.external_id(&instance).map_err(napi_error)? != ExternalId(42) {
        return Ok(false);
    }
    let method = engine
        .get_property(&instance, "identity")
        .map_err(napi_error)?;
    engine
        .call(&method, Some(&instance), &[])
        .map_err(napi_error)?;
    let weak = engine.downgrade(&instance).map_err(napi_error)?;
    if engine.upgrade(&weak).map_err(napi_error)?.is_none() {
        return Ok(false);
    }

    let global_value = engine.string("visible").map_err(napi_error)?;
    engine
        .set_global("__blitsenSmoke", &global_value)
        .map_err(napi_error)?;
    let global_result = engine
        .evaluate_script("globalThis.__blitsenSmoke", "global-smoke.js")
        .and_then(|value| engine.to_string(&value))
        .map_err(napi_error)?;
    if global_result != "visible" {
        return Ok(false);
    }

    let document = engine.object().map_err(napi_error)?;
    let mut window_state = WindowState::new(800, 600, 2.0);
    let window = window_state
        .install(&mut engine, &document)
        .map_err(napi_error)?;
    let window_check = engine
        .evaluate_script(
            "window === globalThis && window.document !== undefined && innerWidth === 800 && innerHeight === 600 && devicePixelRatio === 2 && !('location' in window) && !('history' in window) && !('navigator' in window) && !('localStorage' in window)",
            "window-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    if !window_check {
        return Ok(false);
    }
    window_state.resize(1024, 768);
    window_state
        .sync(&mut engine, &window)
        .map_err(napi_error)?;
    let resized = engine
        .evaluate_script(
            "innerWidth === 1024 && innerHeight === 768",
            "resize-smoke.js",
        )
        .and_then(|value| engine.to_boolean(&value))
        .map_err(napi_error)?;
    if !resized {
        return Ok(false);
    }

    let throwing = engine
        .define_function(
            "throwing",
            Box::new(|_| Err(JsError::new("native callback failed"))),
        )
        .map_err(napi_error)?;
    let error = match engine.call(&throwing, None, &[]) {
        Ok(_) => return Ok(false),
        Err(error) => error,
    };
    if !error.message().contains("native callback failed") {
        return Ok(false);
    }

    let module = engine
        .evaluate_module("export const answer = 42", "smoke-module.js")
        .map_err(napi_error)?;
    if engine.value_type(&module).map_err(napi_error)? != JsType::Object {
        return Ok(false);
    }
    engine.drain_microtasks().map_err(napi_error)?;
    engine.pump_event_loop().map_err(napi_error)?;
    Ok(true)
}
