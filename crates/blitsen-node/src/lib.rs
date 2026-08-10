//! Bun-loadable Node-API addon and JavaScript-engine implementation.

mod dom_bridge;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use base64::Engine as _;
use blitsen_blitz::BlitzDom;
use blitsen_core::{
    DocumentScript, ScriptDocument, WindowState, WrapperTable, execute_collected_document_scripts,
};
use blitsen_dom::{DomBackend, DomError};
use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    NativeMethod, TypedArray, TypedArrayKind,
};
use blitz::dom::{
    DocGuard, DocGuardMut, Document as BlitzDocument, DocumentConfig, NodeId, util::Color,
};
use blitz::paint::paint_scene;
use blitz::shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};
use blitz::traits::net::NetProvider;
use blitz::traits::shell::{ColorScheme, Viewport};
use napi::bindgen_prelude::{FromNapiValue, Unknown};
use napi::{Env, JsValue, Status, ValueType, sys};
use napi_derive::napi;
use peniko::{Fill, kurbo::Rect};
use serde::Serialize;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    ButtonSource, DeviceEvent, ElementState, MouseButton, MouseScrollDelta, PointerSource,
    StartCause, WindowEvent,
};
use winit::event_loop::pump_events::EventLoopExtPumpEvents;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{WindowAttributes, WindowId};

#[cfg(target_os = "macos")]
use winit::application::macos::ApplicationHandlerExtMacOS;

/// Stable addon name used by packaging and smoke tests.
pub const ADDON_NAME: &str = "blitsen-node";

fn callback_string(value: &Unknown<'static>) -> Result<String, JsError> {
    let value = value.value();
    // SAFETY: callback arguments are live handles in their originating env.
    unsafe { String::from_napi_value(value.env, value.value) }.map_err(js_error)
}

fn js_error(error: napi::Error) -> JsError {
    JsError::new(error.reason)
}

fn napi_error(error: JsError) -> napi::Error {
    napi::Error::new(Status::GenericFailure, error.to_string())
}

fn dom_error(error: DomError) -> napi::Error {
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
        let loader_base = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| JsError::new(error.to_string()))?
                .join("blitsen-inline-module.js")
        };
        let loader_base = serde_json::to_string(&loader_base.to_string_lossy())
            .map_err(|error| JsError::new(error.to_string()))?;
        if path.is_absolute() && path.is_file() {
            let specifier = serde_json::to_string(identifier)
                .map_err(|error| JsError::new(error.to_string()))?;
            return self.evaluate_script(
                &format!(
                    "process.getBuiltinModule('module').createRequire({loader_base})({specifier})"
                ),
                "blitsen:external-module-loader",
            );
        }
        let source = format!("{source}\n//# sourceURL={identifier}");
        let encoded = base64::engine::general_purpose::STANDARD.encode(source);
        let specifier = serde_json::to_string(&format!("data:text/javascript;base64,{encoded}"))
            .map_err(|error| JsError::new(error.to_string()))?;
        self.evaluate_script(
            &format!(
                "process.getBuiltinModule('module').createRequire({loader_base})({specifier})"
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
    session: RefCell<Option<WindowSession>>,
}

/// Options passed from directory-mode CLI to the native window.
#[napi(object)]
pub struct OpenDirectoryOptions {
    /// Canonical application root.
    pub root: String,
    /// Canonical `index.html` path.
    pub entrypoint: String,
    /// Initial logical width.
    pub width: u32,
    /// Initial logical height.
    pub height: u32,
    /// Native title-bar text.
    pub title: String,
    /// Original directory argument, retained for diagnostics.
    pub directory: String,
}

#[derive(Serialize)]
struct HarnessSnapshot {
    nodes: Vec<HarnessNode>,
    invalidation: HarnessInvalidation,
    paint_colors: Vec<HarnessPaintColor>,
}

#[derive(Serialize)]
struct HarnessInvalidation {
    restyled_nodes: usize,
    relaid_out_nodes: usize,
    full_document: bool,
}

#[derive(Serialize)]
struct HarnessPaintColor {
    rgba: String,
    pixels: usize,
}

#[derive(Serialize)]
struct HarnessNode {
    handle: u64,
    parent: Option<u64>,
    tag: String,
    text_content: String,
    attributes: BTreeMap<String, String>,
    inline_style: String,
    layout: HarnessLayout,
}

#[derive(Serialize)]
struct HarnessLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// Shared native DOM state addressed by serialized generational Blitz handles.
///
/// Every native bridge entry point parses the opaque handle and resolves it in
/// the authoritative document before performing work.
#[derive(Clone)]
pub struct DomRuntime {
    document: Rc<RefCell<BlitzDom>>,
}

struct WindowApplication<Rend: anyrender::WindowRenderer> {
    inner: BlitzApplication<Rend>,
    env: sys::napi_env,
    state: Rc<RefCell<WindowState>>,
    error: Rc<RefCell<Option<JsError>>>,
    started_at: Instant,
    document: Rc<RefCell<BlitzDom>>,
    pending_mouse_input: Vec<(WindowId, PendingMouseInput)>,
    pending_keyboard_input: Vec<(WindowId, PendingKeyboardInput)>,
    pointer_positions: HashMap<WindowId, (f64, f64)>,
    mouse_down_targets: HashMap<u16, NodeId>,
    mouse_buttons: u16,
    modifiers: ModifiersState,
    load_dispatched: bool,
}

#[derive(Clone, Copy)]
enum PendingMouseInput {
    Move {
        physical_x: f64,
        physical_y: f64,
    },
    Button {
        physical_x: f64,
        physical_y: f64,
        button: MouseButton,
        state: ElementState,
    },
    Wheel {
        delta_x: f64,
        delta_y: f64,
    },
}

#[derive(Clone)]
enum PendingKeyboardInput {
    Key {
        event_type: &'static str,
        key: String,
        code: String,
        repeat: bool,
    },
    WindowFocus(bool),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MouseEventInit {
    bubbles: bool,
    cancelable: bool,
    client_x: f64,
    client_y: f64,
    offset_x: f32,
    offset_y: f32,
    screen_x: f64,
    screen_y: f64,
    button: u16,
    buttons: u16,
    ctrl_key: bool,
    shift_key: bool,
    alt_key: bool,
    meta_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardEventInit {
    bubbles: bool,
    cancelable: bool,
    key: String,
    code: String,
    repeat: bool,
    ctrl_key: bool,
    shift_key: bool,
    alt_key: bool,
    meta_key: bool,
}

struct WindowSession {
    runtime: tokio::runtime::Runtime,
    event_loop: EventLoop,
    application: WindowApplication<anyrender_vello::VelloWindowRenderer>,
    error: Rc<RefCell<Option<JsError>>>,
}

fn dom_mouse_button(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        other => other as u16,
    }
}

fn dom_mouse_button_mask(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
        other => 1_u16
            .checked_shl(u32::from(dom_mouse_button(other)))
            .unwrap_or(0),
    }
}

fn css_pointer_coordinates(
    physical_x: f64,
    physical_y: f64,
    scale: f64,
    screen_origin_x: f64,
    screen_origin_y: f64,
) -> (f64, f64, f64, f64) {
    let client_x = physical_x / scale;
    let client_y = physical_y / scale;
    (
        client_x,
        client_y,
        screen_origin_x + client_x,
        screen_origin_y + client_y,
    )
}

fn dom_key_name(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".into(),
        Key::Unidentified(_) => "Unidentified".into(),
    }
}

fn dom_key_code(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => String::new(),
    }
}

impl<Rend: anyrender::WindowRenderer> WindowApplication<Rend> {
    fn queue_mouse_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let input = match event {
            WindowEvent::PointerMoved {
                position,
                source: PointerSource::Mouse,
                ..
            } => {
                self.pointer_positions
                    .insert(window_id, (position.x, position.y));
                self.pending_mouse_input.retain(|(queued_window, input)| {
                    *queued_window != window_id || !matches!(input, PendingMouseInput::Move { .. })
                });
                PendingMouseInput::Move {
                    physical_x: position.x,
                    physical_y: position.y,
                }
            }
            WindowEvent::PointerButton {
                position,
                button: ButtonSource::Mouse(button),
                state,
                ..
            } => {
                self.pointer_positions
                    .insert(window_id, (position.x, position.y));
                PendingMouseInput::Button {
                    physical_x: position.x,
                    physical_y: position.y,
                    button: *button,
                    state: *state,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (f64::from(*x) * 40.0, f64::from(*y) * 40.0)
                    }
                    MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
                };
                PendingMouseInput::Wheel { delta_x, delta_y }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return false;
            }
            _ => return false,
        };
        self.pending_mouse_input.push((window_id, input));
        true
    }

    fn queue_keyboard_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let input = match event {
            WindowEvent::KeyboardInput { event, .. } => PendingKeyboardInput::Key {
                event_type: if event.state == ElementState::Pressed {
                    "keydown"
                } else {
                    "keyup"
                },
                key: dom_key_name(&event.logical_key),
                code: dom_key_code(event.physical_key),
                repeat: event.repeat,
            },
            WindowEvent::Focused(focused) => PendingKeyboardInput::WindowFocus(*focused),
            _ => return false,
        };
        self.pending_keyboard_input.push((window_id, input));
        true
    }

    fn dispatch_keyboard_event(
        &self,
        event_type: &str,
        init: &KeyboardEventInit,
    ) -> Result<bool, JsError> {
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let init = serde_json::to_string(init).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchKeyboardEvent({event_type}, {init})"),
            "blitsen:native-keyboard-event",
        )?;
        engine.to_boolean(&result)
    }

    fn drain_keyboard_input(&mut self, window_id: WindowId) {
        if self.error.borrow().is_some() {
            return;
        }
        let mut inputs = Vec::new();
        self.pending_keyboard_input
            .retain(|(queued_window, input)| {
                if *queued_window == window_id {
                    inputs.push(input.clone());
                    false
                } else {
                    true
                }
            });
        for input in inputs {
            let result = match input {
                PendingKeyboardInput::Key {
                    event_type,
                    key,
                    code,
                    repeat,
                } => self.dispatch_keyboard_event(
                    event_type,
                    &KeyboardEventInit {
                        bubbles: true,
                        cancelable: true,
                        key,
                        code,
                        repeat,
                        ctrl_key: self.modifiers.control_key(),
                        shift_key: self.modifiers.shift_key(),
                        alt_key: self.modifiers.alt_key(),
                        meta_key: self.modifiers.meta_key(),
                    },
                ),
                PendingKeyboardInput::WindowFocus(focused) => {
                    let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
                    engine
                        .evaluate_script(
                            &format!(
                                "globalThis.dispatchEvent(new Event({}))",
                                if focused { "\"focus\"" } else { "\"blur\"" }
                            ),
                            "blitsen:native-window-focus",
                        )
                        .and_then(|value| engine.to_boolean(&value))
                }
            };
            if let Err(error) = result {
                *self.error.borrow_mut() = Some(error);
                return;
            }
        }
    }

    fn dispatch_mouse_event(
        &self,
        event_type: &str,
        target: NodeId,
        init: &MouseEventInit,
    ) -> Result<bool, JsError> {
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let target = serde_json::to_string(&DomRuntime::serialize_handle(target))
            .map_err(|error| JsError::new(error.to_string()))?;
        let init = serde_json::to_string(init).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchMouseEvent({event_type}, {target}, {init})"),
            "blitsen:native-mouse-event",
        )?;
        engine.to_boolean(&result)
    }

    fn drain_mouse_input(&mut self, window_id: WindowId) {
        if self.error.borrow().is_some() {
            return;
        }
        let mut inputs = Vec::new();
        self.pending_mouse_input.retain(|(queued_window, input)| {
            if *queued_window == window_id {
                inputs.push(*input);
                false
            } else {
                true
            }
        });
        if inputs.is_empty() {
            return;
        }
        let Some((scale, screen_origin_x, screen_origin_y)) =
            self.inner.windows.get(&window_id).map(|view| {
                let scale = f64::from(view.doc.inner().viewport().hidpi_scale);
                let origin = view.window.outer_position().unwrap_or_default();
                (
                    scale,
                    f64::from(origin.x) / scale,
                    f64::from(origin.y) / scale,
                )
            })
        else {
            return;
        };

        for input in inputs {
            let (physical_x, physical_y, event_type, button, wheel_delta) = match input {
                PendingMouseInput::Move {
                    physical_x,
                    physical_y,
                } => (physical_x, physical_y, "mousemove", 0, None),
                PendingMouseInput::Button {
                    physical_x,
                    physical_y,
                    button,
                    state,
                } => {
                    let mask = dom_mouse_button_mask(button);
                    match state {
                        ElementState::Pressed => self.mouse_buttons |= mask,
                        ElementState::Released => self.mouse_buttons &= !mask,
                    }
                    (
                        physical_x,
                        physical_y,
                        if state == ElementState::Pressed {
                            "mousedown"
                        } else {
                            "mouseup"
                        },
                        dom_mouse_button(button),
                        None,
                    )
                }
                PendingMouseInput::Wheel { delta_x, delta_y } => {
                    let (physical_x, physical_y) = self
                        .pointer_positions
                        .get(&window_id)
                        .copied()
                        .unwrap_or_default();
                    (physical_x, physical_y, "wheel", 0, Some((delta_x, delta_y)))
                }
            };
            let (client_x, client_y, screen_x, screen_y) = css_pointer_coordinates(
                physical_x,
                physical_y,
                scale,
                screen_origin_x,
                screen_origin_y,
            );
            let hit = (|| {
                let snapshot = self.document.borrow_mut().flush_layout()?;
                self.document
                    .borrow()
                    .hit_test(client_x as f32, client_y as f32, snapshot)
            })();
            let hit = match hit {
                Ok(Some(hit)) => hit,
                Ok(None) => continue,
                Err(error) => {
                    *self.error.borrow_mut() = Some(JsError::new(error.to_string()));
                    return;
                }
            };
            let init = MouseEventInit {
                bubbles: true,
                cancelable: true,
                client_x,
                client_y,
                offset_x: hit.offset_x,
                offset_y: hit.offset_y,
                screen_x,
                screen_y,
                button,
                buttons: self.mouse_buttons,
                ctrl_key: self.modifiers.control_key(),
                shift_key: self.modifiers.shift_key(),
                alt_key: self.modifiers.alt_key(),
                meta_key: self.modifiers.meta_key(),
            };
            let default_allowed = match self.dispatch_mouse_event(event_type, hit.target, &init) {
                Ok(allowed) => allowed,
                Err(error) => {
                    *self.error.borrow_mut() = Some(error);
                    return;
                }
            };
            if default_allowed && let Some((delta_x, delta_y)) = wheel_delta {
                self.document.borrow_mut().document_mut().scroll_node_by(
                    hit.target,
                    -delta_x,
                    -delta_y,
                    |_| {},
                );
            }
            if let PendingMouseInput::Button { button, state, .. } = input {
                let button_id = dom_mouse_button(button);
                if state == ElementState::Pressed {
                    self.mouse_down_targets.insert(button_id, hit.target);
                } else {
                    let down_target = self.mouse_down_targets.remove(&button_id);
                    if button == MouseButton::Left
                        && down_target == Some(hit.target)
                        && let Err(error) = self.dispatch_mouse_event("click", hit.target, &init)
                    {
                        *self.error.borrow_mut() = Some(error);
                        return;
                    }
                }
            }
        }
    }

    fn animation_frames_pending(&self) -> bool {
        if self.error.borrow().is_some() {
            return false;
        }
        let result = (|| {
            let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
            let pending = engine.evaluate_script(
                "globalThis.__blitsenAnimationFramesPending()",
                "blitsen:animation-frame-pending",
            )?;
            engine.to_boolean(&pending)
        })();
        match result {
            Ok(pending) => pending,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                false
            }
        }
    }

    fn run_animation_frame(&self) -> bool {
        if self.error.borrow().is_some() {
            return false;
        }
        let timestamp = self.started_at.elapsed().as_secs_f64() * 1_000.0;
        let result = (|| {
            let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
            let pending = engine.evaluate_script(
                &format!("globalThis.__blitsenAnimationFrameTick({timestamp})"),
                "blitsen:animation-frame-tick",
            )?;
            engine.drain_microtasks()?;
            Ok(engine.to_number(&pending)? > 0.0)
        })();
        match result {
            Ok(pending) => pending,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                false
            }
        }
    }

    fn sync_window(&self, width: u32, height: u32, device_pixel_ratio: f64) {
        if self.error.borrow().is_some() {
            return;
        }
        *self.state.borrow_mut() = WindowState::new(width, height, device_pixel_ratio);
        let result = (|| {
            let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
            let window = engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            self.state.borrow().sync(&mut engine, &window)
        })();
        if let Err(error) = result {
            *self.error.borrow_mut() = Some(error);
        }
    }

    fn sync_native_window(&self, window_id: WindowId) {
        let Some((width, height, scale)) = self.inner.windows.get(&window_id).map(|view| {
            let document = view.doc.inner();
            let viewport = document.viewport();
            let logical =
                winit::dpi::PhysicalSize::new(viewport.window_size.0, viewport.window_size.1)
                    .to_logical::<u32>(f64::from(viewport.hidpi_scale));
            (logical.width, logical.height, viewport.hidpi_scale)
        }) else {
            return;
        };
        self.sync_window(width, height, f64::from(scale));
    }

    fn dispatch_window_event(&self, event_type: &str) -> Result<bool, JsError> {
        let event_type =
            serde_json::to_string(event_type).map_err(|error| JsError::new(error.to_string()))?;
        let mut engine = NodeApiEngine::new(Env::from_raw(self.env));
        let result = engine.evaluate_script(
            &format!("globalThis.__blitsenDispatchLifecycleEvent({event_type})"),
            "blitsen:native-window-event",
        )?;
        engine.to_boolean(&result)
    }

    fn maybe_dispatch_load(&mut self) {
        if self.load_dispatched || self.error.borrow().is_some() {
            return;
        }
        if self
            .document
            .borrow()
            .document_ref()
            .has_pending_critical_resources()
        {
            return;
        }
        match self.dispatch_window_event("load") {
            Ok(_) => {
                self.load_dispatched = true;
                for view in self.inner.windows.values() {
                    view.window.request_redraw();
                }
            }
            Err(error) => *self.error.borrow_mut() = Some(error),
        }
    }
}

struct SharedBlitzDocument(Rc<RefCell<BlitzDom>>);

impl BlitzDocument for SharedBlitzDocument {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::RefCell(std::cell::Ref::map(self.0.borrow(), |document| {
            &**document.document_ref()
        }))
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::RefCell(std::cell::RefMut::map(self.0.borrow_mut(), |document| {
            &mut **document.document_mut()
        }))
    }

    fn poll(&mut self, _task_context: Option<std::task::Context<'_>>) -> bool {
        let mut document = self.0.borrow_mut();
        let pending_before = document.document_ref().has_pending_critical_resources();
        let changes_before = document.document_ref().has_changes();
        document.document_mut().handle_messages();
        pending_before != document.document_ref().has_pending_critical_resources()
            || changes_before != document.document_ref().has_changes()
    }
}

impl<Rend: anyrender::WindowRenderer> ApplicationHandler for WindowApplication<Rend> {
    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, cause: StartCause) {
        self.inner.new_events(event_loop, cause);
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.resumed(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.can_create_surfaces(event_loop);
        let windows: Vec<_> = self.inner.windows.keys().copied().collect();
        for id in windows {
            self.sync_native_window(id);
        }
        self.maybe_dispatch_load();
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.proxy_wake_up(event_loop);
        self.maybe_dispatch_load();
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let queued_mouse_input = self.queue_mouse_input(window_id, &event);
        let queued_keyboard_input = self.queue_keyboard_input(window_id, &event);
        let viewport_changed = matches!(
            &event,
            WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. }
        );
        let redraw = matches!(&event, WindowEvent::RedrawRequested);
        if redraw {
            self.drain_mouse_input(window_id);
            self.drain_keyboard_input(window_id);
        }
        let animation_pending = redraw && self.run_animation_frame();
        self.inner.window_event(event_loop, window_id, event);
        if viewport_changed {
            self.sync_native_window(window_id);
            if let Err(error) = self.dispatch_window_event("resize") {
                *self.error.borrow_mut() = Some(error);
            }
            if let Some(view) = self.inner.windows.get(&window_id) {
                view.window.request_redraw();
            }
        }
        if animation_pending && let Some(view) = self.inner.windows.get(&window_id) {
            view.window.request_redraw();
        }
        if (queued_mouse_input || queued_keyboard_input)
            && let Some(view) = self.inner.windows.get(&window_id)
        {
            view.window.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        device_id: Option<winit::event::DeviceId>,
        event: DeviceEvent,
    ) {
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.about_to_wait(event_loop);
        self.maybe_dispatch_load();
        if self.animation_frames_pending() {
            for view in self.inner.windows.values() {
                view.window.request_redraw();
            }
        }
    }

    fn suspended(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.suspended(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.destroy_surfaces(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
    }

    #[cfg(target_os = "macos")]
    fn macos_handler(&mut self) -> Option<&mut dyn ApplicationHandlerExtMacOS> {
        Some(self)
    }
}

#[cfg(target_os = "macos")]
impl<Rend: anyrender::WindowRenderer> ApplicationHandlerExtMacOS for WindowApplication<Rend> {
    fn standard_key_binding(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        action: &str,
    ) {
        self.inner
            .standard_key_binding(event_loop, window_id, action);
    }
}

impl DomRuntime {
    /// Owns a concrete Blitz backend behind single-threaded shared state.
    pub fn new(document: BlitzDom) -> Self {
        Self {
            document: Rc::new(RefCell::new(document)),
        }
    }

    /// Returns the shared backend for a synchronous bridge operation.
    pub fn document(&self) -> Rc<RefCell<BlitzDom>> {
        Rc::clone(&self.document)
    }

    /// Serializes a versioned Blitz handle without losing integer precision in JavaScript.
    pub fn serialize_handle(node: NodeId) -> String {
        node.as_u64().to_string()
    }

    /// Parses an opaque handle and rejects stale or fabricated generations.
    pub fn resolve_handle(&self, handle: &str) -> Result<NodeId, JsError> {
        let raw = handle
            .parse::<u64>()
            .map_err(|_| JsError::new("invalid DOM node handle"))?;
        let node = NodeId::from_u64(raw);
        self.document
            .borrow()
            .node_kind(node)
            .map_err(|error| JsError::new(error.to_string()))?;
        Ok(node)
    }

    /// Retains a detached node for one live JavaScript wrapper.
    pub fn retain_handle(&self, handle: &str) -> Result<(), JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .retain_for_js(node)
            .map_err(|error| JsError::new(error.to_string()))
    }

    /// Releases one wrapper and collects an otherwise-unowned detached subtree.
    pub fn release_handle(&self, handle: &str) -> Result<bool, JsError> {
        let node = self.resolve_handle(handle)?;
        self.document
            .borrow_mut()
            .release_from_js(node)
            .map_err(|error| JsError::new(error.to_string()))
    }
}

#[napi]
impl Engine {
    /// Creates an engine in the current Bun/Node-API environment.
    #[napi(constructor)]
    pub fn new(env: Env) -> Self {
        Self {
            runtime: RefCell::new(NodeApiEngine::new(env)),
            session: RefCell::new(None),
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

    /// Parses `index.html` and initializes a native Blitz window session.
    #[napi(js_name = "openDirectory")]
    pub fn open_directory(&self, options: OpenDirectoryOptions) -> napi::Result<()> {
        let started_at = Instant::now();
        let source = std::fs::read_to_string(&options.entrypoint).map_err(|error| {
            napi::Error::new(
                Status::GenericFailure,
                format!("could not read {}: {error}", options.entrypoint),
            )
        })?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
        let guard = runtime.enter();
        let event_loop = create_default_event_loop();
        let (proxy, receiver) = BlitzShellProxy::new(event_loop.create_proxy());
        let net_provider = Arc::new(blitz::net::Provider::new(Some(Arc::new(proxy.clone()))));
        let dom_runtime = DomRuntime::new(BlitzDom::from_html(
            &source,
            DocumentConfig {
                base_url: Some(format!("file://{}/", options.root.replace(' ', "%20"))),
                net_provider: Some(net_provider as Arc<dyn NetProvider>),
                viewport: Some(Viewport::new(
                    options.width,
                    options.height,
                    1.0,
                    ColorScheme::Light,
                )),
                ..Default::default()
            },
        ));
        let document = dom_runtime.document();
        validate_local_assets(
            &document.borrow(),
            Path::new(&options.root),
            Path::new(&options.entrypoint),
        )
        .map_err(napi_error)?;
        let scripts = {
            let document = document.borrow();
            document.document_scripts().map_err(dom_error)?
        };
        let mut engine = self.runtime.borrow_mut();
        let raw_env = engine.raw_env();
        let window_state = execute_window_scripts(
            &mut engine,
            dom_runtime,
            scripts,
            &options.entrypoint,
            options.width,
            options.height,
        )?;
        drop(engine);
        document.borrow_mut().flush_layout().map_err(dom_error)?;
        let renderer = anyrender_vello::VelloWindowRenderer::new();
        let attributes = WindowAttributes::default()
            .with_title(options.title)
            .with_surface_size(LogicalSize::new(options.width, options.height));
        let window = WindowConfig::with_attributes(
            Box::new(SharedBlitzDocument(Rc::clone(&document))),
            renderer,
            attributes,
        );
        let mut application = BlitzApplication::new(proxy, receiver);
        application.add_window(window);
        let window_error = Rc::new(RefCell::new(None));
        let application = WindowApplication {
            inner: application,
            env: raw_env,
            state: window_state,
            error: Rc::clone(&window_error),
            started_at,
            document,
            pending_mouse_input: Vec::new(),
            pending_keyboard_input: Vec::new(),
            pointer_positions: HashMap::new(),
            mouse_down_targets: HashMap::new(),
            mouse_buttons: 0,
            modifiers: ModifiersState::empty(),
            load_dispatched: false,
        };
        if self.session.borrow().is_some() {
            return Err(napi::Error::new(
                Status::GenericFailure,
                "a native window session is already open",
            ));
        }
        drop(guard);
        *self.session.borrow_mut() = Some(WindowSession {
            runtime,
            event_loop,
            application,
            error: window_error,
        });
        Ok(())
    }

    /// Advances winit once without blocking Bun's outer event loop.
    #[napi(js_name = "pumpWindow")]
    pub fn pump_window(&self) -> napi::Result<bool> {
        let alive = {
            let mut session = self.session.borrow_mut();
            let session = session.as_mut().ok_or_else(|| {
                napi::Error::new(Status::GenericFailure, "no native window session is open")
            })?;
            let _guard = session.runtime.enter();
            session
                .event_loop
                .pump_app_events(Some(Duration::ZERO), &mut session.application);
            if let Some(error) = session.error.borrow_mut().take() {
                return Err(napi_error(error));
            }
            !session.application.inner.windows.is_empty()
                || !session.application.inner.pending_windows.is_empty()
        };
        if !alive {
            self.session.borrow_mut().take();
        }
        Ok(alive)
    }
}

fn validate_local_assets(
    document: &BlitzDom,
    root: &Path,
    entrypoint: &Path,
) -> Result<(), JsError> {
    let root = root.canonicalize().map_err(|error| {
        JsError::new(format!("could not resolve application directory: {error}"))
    })?;
    let entrypoint_directory = entrypoint.parent().unwrap_or(&root);
    for (selector, attribute) in [
        ("script[src]", "src"),
        ("link[href]", "href"),
        ("img[src]", "src"),
        ("source[src]", "src"),
        ("audio[src]", "src"),
        ("video[src]", "src"),
        ("video[poster]", "poster"),
        ("track[src]", "src"),
        ("embed[src]", "src"),
        ("object[data]", "data"),
        ("input[src]", "src"),
    ] {
        for node in document
            .query_selector_all(document.document(), selector)
            .map_err(|error| JsError::new(error.to_string()))?
        {
            let Some(specifier) = document
                .attribute(node, &blitsen_dom::DomName::attribute(attribute))
                .map_err(|error| JsError::new(error.to_string()))?
            else {
                continue;
            };
            validate_local_asset(&root, entrypoint_directory, &specifier)?;
        }
    }
    Ok(())
}

fn validate_local_asset(root: &Path, from: &Path, specifier: &str) -> Result<(), JsError> {
    let has_scheme = specifier.split_once(':').is_some_and(|(scheme, _)| {
        let mut characters = scheme.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && characters
                .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
    });
    if specifier.starts_with('/') || has_scheme {
        return Err(JsError::new(format!(
            "asset URL must be relative to index.html: {specifier}"
        )));
    }
    let local = specifier.split(['?', '#']).next().unwrap_or_default();
    if local.is_empty() {
        return Ok(());
    }
    let asset = from
        .join(local)
        .canonicalize()
        .map_err(|_| JsError::new(format!("unreadable asset from index.html: {specifier}")))?;
    if !asset.starts_with(root) {
        return Err(JsError::new(format!(
            "asset escapes application directory: {specifier}"
        )));
    }
    if !asset.is_file() {
        return Err(JsError::new(format!(
            "unreadable asset from index.html: {specifier}"
        )));
    }
    Ok(())
}

fn execute_window_scripts(
    engine: &mut NodeApiEngine,
    runtime: DomRuntime,
    scripts: Vec<DocumentScript>,
    entrypoint: &str,
    width: u32,
    height: u32,
) -> napi::Result<Rc<RefCell<WindowState>>> {
    let window_state =
        dom_bridge::install(engine, runtime, width, height, 1.0).map_err(napi_error)?;
    execute_collected_document_scripts(scripts, engine, Path::new(entrypoint))
        .map_err(napi_error)?;
    engine
        .evaluate_script(
            "globalThis.__blitsenDispatchLifecycleEvent('DOMContentLoaded')",
            "blitsen:dom-content-loaded",
        )
        .map_err(napi_error)?;
    Ok(window_state)
}

fn execute_bridge_harness(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<(HarnessSnapshot, Vec<u8>)> {
    let width = width.unwrap_or(800);
    let height = height.unwrap_or(600);
    let runtime = DomRuntime::new(BlitzDom::from_html(
        &html,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    document.borrow_mut().flush_layout().map_err(dom_error)?;
    let mut engine = NodeApiEngine::new(env);
    let _window_state =
        dom_bridge::install(&mut engine, runtime, width, height, 1.0).map_err(napi_error)?;
    engine
        .evaluate_script(&script, "harness-script.js")
        .map_err(napi_error)?;
    snapshot_and_render(document, width, height)
}

fn execute_animation_harness(
    env: Env,
    html: String,
    script: String,
    frames: u32,
    width: u32,
    height: u32,
) -> napi::Result<Vec<HarnessSnapshot>> {
    let runtime = DomRuntime::new(BlitzDom::from_html(
        &html,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    document.borrow_mut().flush_layout().map_err(dom_error)?;
    let mut engine = NodeApiEngine::new(env);
    let _window_state =
        dom_bridge::install(&mut engine, runtime, width, height, 1.0).map_err(napi_error)?;
    engine
        .evaluate_script(&script, "animation-harness-script.js")
        .map_err(napi_error)?;

    let mut snapshots = Vec::with_capacity(frames as usize);
    for frame in 1..=frames {
        let timestamp = f64::from(frame) * (1_000.0 / 60.0);
        engine
            .evaluate_script(
                &format!("globalThis.__blitsenAnimationFrameTick({timestamp})"),
                "blitsen:animation-harness-tick",
            )
            .and_then(|_| engine.drain_microtasks().map(|_| ()))
            .map_err(napi_error)?;
        snapshots.push(snapshot_and_render(Rc::clone(&document), width, height)?.0);
    }
    Ok(snapshots)
}

fn snapshot_and_render(
    document: Rc<RefCell<BlitzDom>>,
    width: u32,
    height: u32,
) -> napi::Result<(HarnessSnapshot, Vec<u8>)> {
    let snapshot = document.borrow_mut().flush_layout().map_err(dom_error)?;
    let (invalidation_metrics, full_document) = document.borrow().last_frame_invalidation();
    let invalidation = HarnessInvalidation {
        restyled_nodes: invalidation_metrics.restyled_nodes,
        relaid_out_nodes: invalidation_metrics.relaid_out_nodes,
        full_document,
    };

    let mut document = document.borrow_mut();
    let ids = document
        .query_selector_all(document.document(), "*")
        .map_err(dom_error)?;
    let mut nodes = Vec::with_capacity(ids.len());
    for id in ids {
        let node = document.document_ref().get_node(id).ok_or_else(|| {
            napi::Error::new(Status::GenericFailure, "Blitz returned a stale node")
        })?;
        let Some(element) = node.element_data() else {
            continue;
        };
        let attributes = element
            .attrs()
            .iter()
            .map(|attribute| {
                (
                    attribute.name.local.to_string(),
                    attribute.value.to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let layout = document.bounding_rect(id, snapshot).map_err(dom_error)?;
        let inline_style = document.inline_style_text(id).map_err(dom_error)?;
        nodes.push(HarnessNode {
            handle: id.as_u64(),
            parent: node.parent.map(|parent| parent.as_u64()),
            tag: element.name.local.to_string(),
            text_content: document.text_content(id).map_err(dom_error)?,
            inline_style,
            attributes,
            layout: HarnessLayout {
                x: layout.x,
                y: layout.y,
                width: layout.width,
                height: layout.height,
            },
        });
    }
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            scene.fill(
                Fill::NonZero,
                Default::default(),
                Color::WHITE,
                Default::default(),
                &Rect::new(0.0, 0.0, width as f64, height as f64),
            );
            paint_scene(
                scene,
                document.document_mut().as_mut(),
                1.0,
                width,
                height,
                0,
                0,
            );
        },
        width,
        height,
    );
    let mut paint_colors = BTreeMap::<[u8; 4], usize>::new();
    for pixel in pixels.chunks_exact(4) {
        *paint_colors
            .entry([pixel[0], pixel[1], pixel[2], pixel[3]])
            .or_default() += 1;
    }
    let mut paint_colors: Vec<_> = paint_colors
        .into_iter()
        .map(|(rgba, pixels)| HarnessPaintColor {
            rgba: format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                rgba[0], rgba[1], rgba[2], rgba[3]
            ),
            pixels,
        })
        .collect();
    paint_colors.sort_unstable_by(|left, right| {
        right
            .pixels
            .cmp(&left.pixels)
            .then_with(|| left.rgba.cmp(&right.rgba))
    });
    paint_colors.truncate(16);
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
        writer
            .write_image_data(&pixels)
            .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))?;
    }
    Ok((
        HarnessSnapshot {
            nodes,
            invalidation,
            paint_colors,
        },
        png,
    ))
}

fn execute_document_harness(
    env: Env,
    entrypoint: &Path,
    width: u32,
    height: u32,
) -> napi::Result<HarnessSnapshot> {
    let source = std::fs::read_to_string(entrypoint).map_err(|error| {
        napi::Error::new(
            Status::GenericFailure,
            format!("could not read {}: {error}", entrypoint.display()),
        )
    })?;
    let root = entrypoint.parent().unwrap_or_else(|| Path::new("."));
    let runtime = DomRuntime::new(BlitzDom::from_html(
        &source,
        DocumentConfig {
            base_url: Some(format!("file://{}/", root.display())),
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    ));
    let document = runtime.document();
    let scripts = document.borrow().document_scripts().map_err(dom_error)?;
    let mut engine = NodeApiEngine::new(env);
    execute_window_scripts(
        &mut engine,
        runtime,
        scripts,
        &entrypoint.to_string_lossy(),
        width,
        height,
    )?;
    engine
        .evaluate_script(
            "globalThis.__blitsenDispatchLifecycleEvent('load')",
            "blitsen:load",
        )
        .map_err(napi_error)?;
    snapshot_and_render(document, width, height).map(|(snapshot, _)| snapshot)
}

/// Boots Blitz headlessly, runs JavaScript DOM mutations, and returns the Rust
/// tree state as JSON for cross-platform CI assertions.
#[napi]
pub fn run_bridge_harness(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (snapshot, _) = execute_bridge_harness(env, html, script, width, height)?;
    serde_json::to_string(&snapshot)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Advances a document through a deterministic sequence of animation frames.
#[napi]
pub fn run_animation_harness(
    env: Env,
    html: String,
    script: String,
    frames: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let frames = frames.unwrap_or(3);
    if frames > 10_000 {
        return Err(napi::Error::new(
            Status::InvalidArg,
            "animation harness is limited to 10000 frames",
        ));
    }
    let snapshots = execute_animation_harness(
        env,
        html,
        script,
        frames,
        width.unwrap_or(800),
        height.unwrap_or(600),
    )?;
    serde_json::to_string(&snapshots)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Loads a real HTML entrypoint and executes its collected script elements.
#[napi]
pub fn run_document_scripts_harness(
    env: Env,
    entrypoint: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let snapshot = execute_document_harness(
        env,
        Path::new(&entrypoint),
        width.unwrap_or(800),
        height.unwrap_or(600),
    )?;
    serde_json::to_string(&snapshot)
        .map_err(|error| napi::Error::new(Status::GenericFailure, error.to_string()))
}

/// Renders the post-JavaScript frame as a base64-encoded PNG.
#[napi]
pub fn render_bridge_harness_png(
    env: Env,
    html: String,
    script: String,
    width: Option<u32>,
    height: Option<u32>,
) -> napi::Result<String> {
    let (_, png) = execute_bridge_harness(env, html, script, width, height)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// Exercises the real Node-API weak-reference and finalizer identity path.
#[napi]
pub fn wrapper_identity_smoke(env: Env) -> napi::Result<bool> {
    let mut engine = NodeApiEngine::new(env);
    let class = engine
        .register_class(NativeClass::new("IdentityNode"))
        .map_err(napi_error)?;
    let table = WrapperTable::<NodeId, NodeWeakRef>::new();
    let raw_env = engine.raw_env();
    let weak_map_works = Env::from_raw(raw_env).run_in_scope(|| {
        let node = NodeId::from_u64(1);
        let first = table
            .get_or_create(&mut engine, node, |engine, finalizer| {
                engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
            })
            .map_err(napi_error)?;
        let second = table
            .get_or_create(&mut engine, node, |_, _| {
                Err(JsError::new("identity table created a duplicate wrapper"))
            })
            .map_err(napi_error)?;
        let mut strictly_equal = false;
        check(
            unsafe {
                sys::napi_strict_equals(raw_env, raw(&first), raw(&second), &mut strictly_equal)
            },
            "compare wrapper identity",
        )
        .map_err(napi_error)?;
        if !strictly_equal {
            return Ok(false);
        }
        engine
            .set_global("__blitsenIdentityFirst", &first)
            .and_then(|_| engine.set_global("__blitsenIdentitySecond", &second))
            .map_err(napi_error)?;
        engine
            .evaluate_script(
                "(() => { const identityMap = new WeakMap([[__blitsenIdentityFirst, 42]]); return identityMap.get(__blitsenIdentitySecond) === 42; })()",
                "blitsen:identity-weak-map",
            )
            .and_then(|value| engine.to_boolean(&value))
            .map_err(napi_error)
    })?;
    if !weak_map_works {
        return Ok(false);
    }

    for slot in 2..=100_001_u64 {
        Env::from_raw(raw_env).run_in_scope(|| {
            let node = NodeId::from_u64(slot);
            table
                .get_or_create(&mut engine, node, |engine, finalizer| {
                    engine.instantiate(&class, ExternalId(node.as_u64()), Some(finalizer))
                })
                .map(|_| ())
                .map_err(napi_error)
        })?;
    }
    if table.len() != 100_001 {
        return Ok(false);
    }
    engine
        .evaluate_script(
            "delete globalThis.__blitsenIdentityFirst; delete globalThis.__blitsenIdentitySecond; Bun.gc(true); Bun.gc(true)",
            "blitsen:identity-gc",
        )
        .map_err(napi_error)?;
    table.prune_collected(&mut engine).map_err(napi_error)?;
    Ok(table.is_empty())
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

#[cfg(test)]
mod tests {
    use blitsen_dom::DomName;

    use super::*;

    #[test]
    fn native_runtime_rejects_stale_generational_handles() {
        let mut dom = BlitzDom::from_html("<body><main id=host></main></body>", Default::default());
        let host = dom.get_element_by_id("host").unwrap().unwrap();
        let node = dom.create_element(&DomName::html("section")).unwrap();
        dom.append_child(host, node).unwrap();
        let runtime = DomRuntime::new(dom);
        let handle = DomRuntime::serialize_handle(node);

        runtime.retain_handle(&handle).unwrap();
        runtime.document().borrow_mut().remove(node).unwrap();
        assert_eq!(runtime.resolve_handle(&handle).unwrap(), node);
        assert!(runtime.release_handle(&handle).unwrap());
        assert!(runtime.resolve_handle(&handle).is_err());

        let replacement = runtime
            .document()
            .borrow_mut()
            .create_element(&DomName::html("aside"))
            .unwrap();
        assert_ne!(DomRuntime::serialize_handle(replacement), handle);
        assert!(runtime.resolve_handle("18446744073709551615").is_err());
    }

    #[test]
    fn entrypoint_assets_are_preflighted_inside_the_application_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/blitsen/test/fixtures/scripts")
            .canonicalize()
            .unwrap();
        let entrypoint = root.join("index.html");
        let document = |source| {
            BlitzDom::from_html(
                source,
                DocumentConfig {
                    base_url: Some(format!("file://{}/", root.display())),
                    ..Default::default()
                },
            )
        };
        let valid = document("<link href='#local'><img src='./dependency.js?cache=1'>");
        validate_local_assets(&valid, &root, &entrypoint).unwrap();

        for (source, expected) in [
            ("<img src='./missing.png'>", "unreadable asset"),
            ("<script src='/app.js'></script>", "must be relative"),
            ("<img src='https://example.com/a.png'>", "must be relative"),
            (
                "<img src='../../../../../Cargo.toml'>",
                "escapes application",
            ),
        ] {
            let invalid = document(source);
            let error = validate_local_assets(&invalid, &root, &entrypoint).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }
    }

    #[test]
    fn mouse_coordinates_and_button_masks_match_dom_conventions() {
        assert_eq!(dom_mouse_button(MouseButton::Left), 0);
        assert_eq!(dom_mouse_button(MouseButton::Middle), 1);
        assert_eq!(dom_mouse_button(MouseButton::Right), 2);
        assert_eq!(dom_mouse_button_mask(MouseButton::Left), 1);
        assert_eq!(dom_mouse_button_mask(MouseButton::Right), 2);
        assert_eq!(dom_mouse_button_mask(MouseButton::Middle), 4);
        assert_eq!(
            css_pointer_coordinates(300.0, 180.0, 2.0, 40.0, 30.0),
            (150.0, 90.0, 190.0, 120.0)
        );
        assert_eq!(dom_key_name(&Key::Character("a".into())), "a");
        assert_eq!(
            dom_key_name(&Key::Named(winit::keyboard::NamedKey::Tab)),
            "Tab"
        );
        assert_eq!(
            dom_key_code(PhysicalKey::Code(winit::keyboard::KeyCode::KeyA)),
            "KeyA"
        );
    }
}
