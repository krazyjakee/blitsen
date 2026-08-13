//! [`JsEngine`](blitsen_js::JsEngine) implementation over the JSC C API.

use std::{cell::RefCell, ffi::CString, ptr, rc::Rc};

use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCallback, NativeClass, TypedArray,
    TypedArrayKind,
};
use libloading::Library;

use crate::{Error, ffi};

use crate::runtime::{PrivateData, Runtime, finalize_private};

/// A protected JavaScriptCore value tied to its process-lived runtime.
pub struct JscValue {
    pub(crate) runtime: Rc<Runtime>,
    pub(crate) raw: ffi::JsValueRef,
}

impl JscValue {
    pub(crate) fn new(runtime: Rc<Runtime>, raw: ffi::JsValueRef) -> Self {
        // SAFETY: raw is a live value in runtime's context.
        unsafe { (runtime.functions.value_protect)(runtime.context, raw) };
        Self { runtime, raw }
    }
}

impl Clone for JscValue {
    fn clone(&self) -> Self {
        Self::new(Rc::clone(&self.runtime), self.raw)
    }
}

impl Drop for JscValue {
    fn drop(&mut self) {
        // SAFETY: every JscValue owns one matching protection.
        unsafe { (self.runtime.functions.value_unprotect)(self.runtime.context, self.raw) };
    }
}

/// A JavaScript `WeakRef` object which does not retain its target.
pub struct JscWeakRef(JscValue);

/// A registered native JSC class and its retained constructor.
pub struct JscClass {
    runtime: Rc<Runtime>,
    raw: ffi::JsClassRef,
    constructor: JscValue,
}

impl Drop for JscClass {
    fn drop(&mut self) {
        // SAFETY: raw is the retained class reference created at registration.
        unsafe { (self.runtime.functions.class_release)(self.raw) };
    }
}

/// Process-lived JavaScriptCore engine loaded from a replaceable shared library.
///
/// Cloning produces another view of the same context, not another context. The
/// host keeps such a view wherever it must re-enter JavaScript from a callback
/// it does not own, which is what the Phase 1 addon uses a raw `napi_env` for.
#[derive(Clone)]
pub struct JavaScriptCore {
    runtime: Rc<Runtime>,
}

impl JavaScriptCore {
    pub(crate) fn from_library(library: Library) -> Result<Self, Error> {
        Runtime::new(library).map(|runtime| Self { runtime })
    }

    /// Evaluates `source` and converts its completion value to a number.
    pub fn evaluate_number(&mut self, source: &str) -> Result<f64, Error> {
        self.evaluate_script(source, "blitsen:acquisition-smoke")
            .and_then(|value| self.to_number(&value))
            .map_err(|error| Error::Evaluation(error.to_string()))
    }

    /// Whether this library can link a module graph supplied by the host.
    ///
    /// False for a system JavaScriptCore, whose public C API has no module
    /// loader hook at all. Checked before an application is loaded so a build
    /// against the wrong library fails by name rather than at the first
    /// `import` inside the application.
    pub fn supports_modules(&self) -> bool {
        self.runtime
            .functions
            .load_and_evaluate_module_from_source
            .is_some()
            && self.runtime.functions.set_module_loader_functions.is_some()
    }

    /// Points the engine's module loader at the host's resolver and reader.
    ///
    /// `resolve(referrerUrl, specifier) -> url` and `fetch(url) -> source` are
    /// ordinary JavaScript functions, installed by
    /// [`blitsen_host::modules::ModuleRegistry`]. Giving the engine functions
    /// rather than C callbacks keeps this to one symbol and keeps the policy —
    /// what a specifier means, what the application may reach — in one place on
    /// the host side. See `docs/JSC.md`.
    pub fn set_module_loader(
        &mut self,
        resolve: &JscValue,
        fetch: &JscValue,
    ) -> Result<(), JsError> {
        let Some(set_loader) = self.runtime.functions.set_module_loader_functions else {
            return Err(JsError::new(
                "this JavaScriptCore library exposes no module loader hook \
                 (JSGlobalContextSetModuleLoaderFunctions); use Blitsen's pinned JSC build",
            ));
        };
        let resolve = self.object_ref(resolve)?;
        let fetch = self.object_ref(fetch)?;
        // SAFETY: both handles are live objects in this context, and the
        // context outlives the loader it is being given.
        unsafe { set_loader(self.runtime.context, resolve, fetch) };
        Ok(())
    }

    /// Requests a full collection. Intended for conformance tests and diagnostics.
    pub fn collect_garbage(&mut self) {
        // SAFETY: the context is live and main-thread-owned.
        unsafe { (self.runtime.functions.garbage_collect)(self.runtime.context) };
    }

    fn wrap(&self, raw: ffi::JsValueRef) -> JscValue {
        JscValue::new(Rc::clone(&self.runtime), raw)
    }

    fn object_ref(&self, value: &JscValue) -> Result<ffi::JsObjectRef, JsError> {
        if !Rc::ptr_eq(&self.runtime, &value.runtime) {
            return Err(JsError::new("JavaScript value belongs to another context"));
        }
        self.runtime.object_from_raw(value.raw)
    }
}

impl JsEngine for JavaScriptCore {
    type Value = JscValue;
    type WeakRef = JscWeakRef;
    type Class = JscClass;

    fn from_value(value: &Self::Value) -> Self {
        Self {
            runtime: Rc::clone(&value.runtime),
        }
    }

    fn undefined(&mut self) -> Self::Value {
        // SAFETY: context is live.
        self.wrap(unsafe { (self.runtime.functions.value_undefined)(self.runtime.context) })
    }

    fn null(&mut self) -> Self::Value {
        // SAFETY: context is live.
        self.wrap(unsafe { (self.runtime.functions.value_null)(self.runtime.context) })
    }

    fn boolean(&mut self, value: bool) -> Self::Value {
        // SAFETY: context is live.
        self.wrap(unsafe { (self.runtime.functions.value_boolean)(self.runtime.context, value) })
    }

    fn number(&mut self, value: f64) -> Self::Value {
        // SAFETY: context is live.
        self.wrap(unsafe { (self.runtime.functions.value_number)(self.runtime.context, value) })
    }

    fn string(&mut self, value: &str) -> Result<Self::Value, JsError> {
        let string = self.runtime.js_string(value)?;
        // SAFETY: context/string are live.
        let value = unsafe { (self.runtime.functions.value_string)(self.runtime.context, string) };
        // SAFETY: JSValue retains the string contents.
        unsafe { (self.runtime.functions.string_release)(string) };
        Ok(self.wrap(value))
    }

    fn object(&mut self) -> Result<Self::Value, JsError> {
        // SAFETY: null class/private data create an ordinary object.
        let object = unsafe {
            (self.runtime.functions.object_make)(
                self.runtime.context,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        Ok(self.wrap(object.cast_const()))
    }

    fn array(&mut self, values: &[Self::Value]) -> Result<Self::Value, JsError> {
        let values: Vec<_> = values.iter().map(|value| value.raw).collect();
        let mut exception = ptr::null();
        // SAFETY: values are live in this context.
        let array = unsafe {
            (self.runtime.functions.object_make_array)(
                self.runtime.context,
                values.len(),
                values.as_ptr(),
                &mut exception,
            )
        };
        if exception.is_null() {
            Ok(self.wrap(array.cast_const()))
        } else {
            Err(self.runtime.exception(exception))
        }
    }

    fn typed_array(&mut self, value: &TypedArray) -> Result<Self::Value, JsError> {
        let mut exception = ptr::null();
        // SAFETY: kind and length are validated by TypedArray.
        let array = unsafe {
            (self.runtime.functions.object_make_typed_array)(
                self.runtime.context,
                typed_array_kind(value.kind),
                value.len(),
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        // SAFETY: array is the live typed array just created.
        let bytes = unsafe {
            (self.runtime.functions.object_typed_array_bytes)(
                self.runtime.context,
                array,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        if !value.bytes.is_empty() {
            // SAFETY: the JSC allocation is exactly value.bytes.len() bytes.
            unsafe {
                ptr::copy_nonoverlapping(value.bytes.as_ptr(), bytes.cast(), value.bytes.len())
            };
        }
        Ok(self.wrap(array.cast_const()))
    }

    fn value_type(&mut self, value: &Self::Value) -> Result<JsType, JsError> {
        // SAFETY: value belongs to the live context.
        let kind = unsafe { (self.runtime.functions.value_type)(self.runtime.context, value.raw) };
        match kind {
            0 => Ok(JsType::Undefined),
            1 => Ok(JsType::Null),
            2 => Ok(JsType::Boolean),
            3 => Ok(JsType::Number),
            4 => Ok(JsType::String),
            5 => {
                let object = self.object_ref(value)?;
                // SAFETY: object is live.
                if unsafe {
                    (self.runtime.functions.object_is_function)(self.runtime.context, object)
                } {
                    return Ok(JsType::Function);
                }
                // SAFETY: value is live.
                if unsafe {
                    (self.runtime.functions.value_is_array)(self.runtime.context, value.raw)
                } {
                    return Ok(JsType::Array);
                }
                let mut exception = ptr::null();
                // SAFETY: value is live.
                let typed = unsafe {
                    (self.runtime.functions.value_typed_array_type)(
                        self.runtime.context,
                        value.raw,
                        &mut exception,
                    )
                };
                if !exception.is_null() {
                    return Err(self.runtime.exception(exception));
                }
                Ok(if typed == 10 {
                    JsType::Object
                } else {
                    JsType::TypedArray
                })
            }
            other => Err(JsError::new(format!(
                "unsupported JavaScript value type {other}"
            ))),
        }
    }

    fn to_boolean(&mut self, value: &Self::Value) -> Result<bool, JsError> {
        // SAFETY: value belongs to this context.
        Ok(unsafe { (self.runtime.functions.value_to_boolean)(self.runtime.context, value.raw) })
    }

    fn to_number(&mut self, value: &Self::Value) -> Result<f64, JsError> {
        let mut exception = ptr::null();
        // SAFETY: value is live and exception is writable.
        let number = unsafe {
            (self.runtime.functions.value_to_number)(
                self.runtime.context,
                value.raw,
                &mut exception,
            )
        };
        if exception.is_null() {
            Ok(number)
        } else {
            Err(self.runtime.exception(exception))
        }
    }

    fn to_string(&mut self, value: &Self::Value) -> Result<String, JsError> {
        self.runtime.value_to_string_raw(value.raw)
    }

    fn to_array(&mut self, value: &Self::Value) -> Result<Vec<Self::Value>, JsError> {
        if self.value_type(value)? != JsType::Array {
            return Err(JsError::new("value is not an array"));
        }
        let object = self.object_ref(value)?;
        let length = self.runtime.get_property_raw(object, "length")?;
        let length = self.to_number(&self.wrap(length))? as usize;
        (0..length)
            .map(|index| {
                let mut exception = ptr::null();
                // SAFETY: object is an array and index is in range.
                let value = unsafe {
                    (self.runtime.functions.object_get_index)(
                        self.runtime.context,
                        object,
                        index as u32,
                        &mut exception,
                    )
                };
                if exception.is_null() {
                    Ok(self.wrap(value))
                } else {
                    Err(self.runtime.exception(exception))
                }
            })
            .collect()
    }

    fn to_typed_array(&mut self, value: &Self::Value) -> Result<TypedArray, JsError> {
        let mut exception = ptr::null();
        // SAFETY: value is live.
        let kind = unsafe {
            (self.runtime.functions.value_typed_array_type)(
                self.runtime.context,
                value.raw,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        let kind = from_typed_array_kind(kind)?;
        let object = self.object_ref(value)?;
        // SAFETY: object is a typed array.
        let length = unsafe {
            (self.runtime.functions.object_typed_array_byte_length)(
                self.runtime.context,
                object,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        // SAFETY: object is a typed array.
        let bytes = unsafe {
            (self.runtime.functions.object_typed_array_bytes)(
                self.runtime.context,
                object,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        let bytes = if length == 0 {
            Vec::new()
        } else {
            // SAFETY: JSC reports a live view of exactly length bytes.
            unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length) }.to_vec()
        };
        TypedArray::new(kind, bytes)
    }

    fn get_property(&mut self, object: &Self::Value, name: &str) -> Result<Self::Value, JsError> {
        let object = self.object_ref(object)?;
        self.runtime
            .get_property_raw(object, name)
            .map(|value| self.wrap(value))
    }

    fn set_property(
        &mut self,
        object: &Self::Value,
        name: &str,
        value: &Self::Value,
    ) -> Result<(), JsError> {
        self.runtime
            .set_property_raw(self.object_ref(object)?, name, value.raw)
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), JsError> {
        // SAFETY: context is live.
        let global = unsafe { (self.runtime.functions.context_global)(self.runtime.context) };
        self.runtime.set_property_raw(global, name, value.raw)
    }

    fn define_function(
        &mut self,
        _name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<Self::Value, JsError> {
        let private = Box::into_raw(Box::new(PrivateData::Callback {
            runtime: Rc::downgrade(&self.runtime),
            callback: RefCell::new(callback),
        }));
        // SAFETY: callback_class accepts and finalizes this PrivateData pointer.
        let object = unsafe {
            (self.runtime.functions.object_make)(
                self.runtime.context,
                self.runtime.callback_class,
                private.cast(),
            )
        };
        if object.is_null() {
            // SAFETY: JSC did not take ownership when object creation failed.
            drop(unsafe { Box::from_raw(private) });
            Err(JsError::new(
                "JavaScriptCore could not create a native function",
            ))
        } else {
            Ok(self.wrap(object.cast_const()))
        }
    }

    fn call(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError> {
        let function = self.object_ref(function)?;
        let this = this
            .map(|value| self.object_ref(value))
            .transpose()?
            .unwrap_or(ptr::null_mut());
        let arguments: Vec<_> = arguments.iter().map(|value| value.raw).collect();
        let mut exception = ptr::null();
        // SAFETY: handles belong to the context and argument storage is live.
        let value = unsafe {
            (self.runtime.functions.object_call)(
                self.runtime.context,
                function,
                this,
                arguments.len(),
                arguments.as_ptr(),
                &mut exception,
            )
        };
        if exception.is_null() {
            Ok(self.wrap(value))
        } else {
            Err(self.runtime.exception(exception))
        }
    }

    fn register_class(
        &mut self,
        definition: NativeClass<Self::Value>,
    ) -> Result<Self::Class, JsError> {
        let name = CString::new(definition.name)
            .map_err(|_| JsError::new("native class name contains an interior NUL"))?;
        let mut class_definition = ffi::ClassDefinition::named(name.as_ptr());
        class_definition.finalize = Some(finalize_private);
        // SAFETY: JSC copies the definition into a retained class.
        let class = unsafe { (self.runtime.functions.class_create)(&class_definition) };
        if class.is_null() {
            return Err(JsError::new(
                "JavaScriptCore could not register a native class",
            ));
        }
        // SAFETY: null constructor callback requests JSC's default constructor.
        let constructor = unsafe {
            (self.runtime.functions.object_make_constructor)(
                self.runtime.context,
                class,
                ptr::null(),
            )
        };
        let constructor = self.wrap(constructor.cast_const());
        let prototype = self.get_property(&constructor, "prototype")?;
        for method in definition.methods {
            let function = self.define_function(&method.name, method.callback)?;
            self.set_property(&prototype, &method.name, &function)?;
        }
        Ok(JscClass {
            runtime: Rc::clone(&self.runtime),
            raw: class,
            constructor,
        })
    }

    fn instantiate(
        &mut self,
        class: &Self::Class,
        external: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    ) -> Result<Self::Value, JsError> {
        let constructor = self.object_ref(&class.constructor)?;
        let mut exception = ptr::null();
        // SAFETY: constructor is the live default constructor for class.
        let object = unsafe {
            (self.runtime.functions.object_construct)(
                self.runtime.context,
                constructor,
                0,
                ptr::null(),
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.runtime.exception(exception));
        }
        let private = Box::into_raw(Box::new(PrivateData::Instance {
            id: external,
            finalizer,
        }));
        // SAFETY: this class supports private storage and owns it on success.
        if unsafe { (self.runtime.functions.object_set_private)(object, private.cast()) } {
            Ok(self.wrap(object.cast_const()))
        } else {
            // SAFETY: JSC rejected ownership.
            drop(unsafe { Box::from_raw(private) });
            Err(JsError::new("JavaScriptCore rejected native instance data"))
        }
    }

    fn external_id(&mut self, value: &Self::Value) -> Result<ExternalId, JsError> {
        self.runtime.external_id_raw(value.raw)
    }

    fn downgrade(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        let constructor =
            self.evaluate_script("value => new WeakRef(value)", "blitsen:weak-ref")?;
        self.call(&constructor, None, std::slice::from_ref(value))
            .map(JscWeakRef)
    }

    fn upgrade(&mut self, reference: &Self::WeakRef) -> Result<Option<Self::Value>, JsError> {
        let deref = self.get_property(&reference.0, "deref")?;
        let value = self.call(&deref, Some(&reference.0), &[])?;
        Ok((self.value_type(&value)? != JsType::Undefined).then_some(value))
    }

    fn evaluate_script(&mut self, source: &str, filename: &str) -> Result<Self::Value, JsError> {
        let source = self.runtime.js_string(source)?;
        let filename = self.runtime.js_string(filename)?;
        let mut exception = ptr::null();
        // SAFETY: strings/context are live and exception is writable.
        let value = unsafe {
            (self.runtime.functions.evaluate_script)(
                self.runtime.context,
                source,
                ptr::null_mut(),
                filename,
                1,
                &mut exception,
            )
        };
        // SAFETY: both are owned temporary strings.
        unsafe {
            (self.runtime.functions.string_release)(source);
            (self.runtime.functions.string_release)(filename);
        }
        if exception.is_null() {
            Ok(self.wrap(value))
        } else {
            Err(self.runtime.exception(exception))
        }
    }

    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        let Some(evaluate) = self.runtime.functions.load_and_evaluate_module_from_source else {
            return Err(JsError::new(
                "JavaScriptCore library lacks JSLoadAndEvaluateModuleFromSource; use Blitsen's pinned JSC build",
            ));
        };
        let source = self.runtime.js_string(source)?;
        let identifier = self.runtime.js_string(identifier)?;
        let mut exception = ptr::null();
        // SAFETY: this optional symbol is loaded with its Bun JSC declaration.
        unsafe {
            evaluate(self.runtime.context, source, identifier, 1, &mut exception);
            (self.runtime.functions.string_release)(source);
            (self.runtime.functions.string_release)(identifier);
        }
        if exception.is_null() {
            Ok(self.undefined())
        } else {
            Err(self.runtime.exception(exception))
        }
    }

    fn drain_microtasks(&mut self) -> Result<usize, JsError> {
        // JSC performs its microtask checkpoint when the outermost C API call
        // returns. A no-op evaluation supplies an explicit checkpoint boundary.
        self.evaluate_script("void 0", "blitsen:microtask-checkpoint")?;
        Ok(0)
    }

    fn pump_event_loop(&mut self) -> Result<LoopTurn, JsError> {
        // Phase 2 runtime services own timers and I/O tasks; bare JSC has no
        // independent outer event loop to pump.
        Ok(LoopTurn::Idle)
    }
}

fn typed_array_kind(kind: TypedArrayKind) -> u32 {
    match kind {
        TypedArrayKind::Int8 => 0,
        TypedArrayKind::Int16 => 1,
        TypedArrayKind::Int32 => 2,
        TypedArrayKind::Uint8 => 3,
        TypedArrayKind::Uint8Clamped => 4,
        TypedArrayKind::Uint16 => 5,
        TypedArrayKind::Uint32 => 6,
        TypedArrayKind::Float32 => 7,
        TypedArrayKind::Float64 => 8,
        TypedArrayKind::BigInt64 => 11,
        TypedArrayKind::BigUint64 => 12,
    }
}

fn from_typed_array_kind(kind: u32) -> Result<TypedArrayKind, JsError> {
    match kind {
        0 => Ok(TypedArrayKind::Int8),
        1 => Ok(TypedArrayKind::Int16),
        2 => Ok(TypedArrayKind::Int32),
        3 => Ok(TypedArrayKind::Uint8),
        4 => Ok(TypedArrayKind::Uint8Clamped),
        5 => Ok(TypedArrayKind::Uint16),
        6 => Ok(TypedArrayKind::Uint32),
        7 => Ok(TypedArrayKind::Float32),
        8 => Ok(TypedArrayKind::Float64),
        11 => Ok(TypedArrayKind::BigInt64),
        12 => Ok(TypedArrayKind::BigUint64),
        other => Err(JsError::new(format!(
            "value is not a supported typed array ({other})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use blitsen_js::{JsEngine, JsType, NativeClass, NativeMethod, TypedArray, TypedArrayKind};

    use super::JavaScriptCore;

    fn native_weak_reference(
        engine: &mut JavaScriptCore,
        class: &super::JscClass,
        finalized: &Rc<Cell<Option<blitsen_js::ExternalId>>>,
    ) -> super::JscWeakRef {
        let finalizer_state = Rc::clone(finalized);
        let instance = engine
            .instantiate(
                class,
                blitsen_js::ExternalId(73),
                Some(Box::new(move |id| finalizer_state.set(Some(id)))),
            )
            .unwrap();
        assert_eq!(
            engine.external_id(&instance).unwrap(),
            blitsen_js::ExternalId(73)
        );
        let method = engine.get_property(&instance, "identity").unwrap();
        let argument = engine.string("Blitsen").unwrap();
        let result = engine
            .call(&method, Some(&instance), std::slice::from_ref(&argument))
            .unwrap();
        assert_eq!(engine.to_string(&result).unwrap(), "Blitsen");
        engine.downgrade(&instance).unwrap()
    }

    #[test]
    fn public_c_api_implements_the_engine_boundary() {
        let mut engine = match JavaScriptCore::load() {
            Ok(engine) => engine,
            Err(error) if std::env::var_os("BLITSEN_REQUIRE_JSC").is_none() => {
                // Cross-target builds compile the loader without requiring a
                // host JSC installation. Native release jobs supply it.
                eprintln!("skipping JSC conformance test: {error}");
                return;
            }
            Err(error) => panic!("required JavaScriptCore library is unavailable: {error}"),
        };

        let undefined = engine.undefined();
        let null = engine.null();
        let boolean = engine.boolean(true);
        let number = engine.number(42.5);
        let string = engine.string("Blitsen").unwrap();
        assert_eq!(engine.value_type(&undefined).unwrap(), JsType::Undefined);
        assert_eq!(engine.value_type(&null).unwrap(), JsType::Null);
        assert!(engine.to_boolean(&boolean).unwrap());
        assert_eq!(engine.to_number(&number).unwrap(), 42.5);
        assert_eq!(engine.to_string(&string).unwrap(), "Blitsen");

        let object = engine.object().unwrap();
        engine.set_property(&object, "answer", &number).unwrap();
        let answer = engine.get_property(&object, "answer").unwrap();
        assert_eq!(engine.to_number(&answer).unwrap(), 42.5);
        engine.set_global("nativeObject", &object).unwrap();
        let answer = engine
            .evaluate_script("nativeObject.answer", "engine-test.js")
            .unwrap();
        assert_eq!(engine.to_number(&answer).unwrap(), 42.5);

        let array = engine.array(&[boolean.clone(), number.clone()]).unwrap();
        assert_eq!(engine.value_type(&array).unwrap(), JsType::Array);
        let values = engine.to_array(&array).unwrap();
        assert!(engine.to_boolean(&values[0]).unwrap());
        assert_eq!(engine.to_number(&values[1]).unwrap(), 42.5);

        let typed = TypedArray::new(TypedArrayKind::Uint32, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let value = engine.typed_array(&typed).unwrap();
        assert_eq!(engine.value_type(&value).unwrap(), JsType::TypedArray);
        assert_eq!(engine.to_typed_array(&value).unwrap(), typed);

        let identity = engine
            .define_function(
                "identity",
                Box::new(|call| {
                    call.arguments
                        .first()
                        .cloned()
                        .ok_or_else(|| blitsen_js::JsError::new("missing argument"))
                }),
            )
            .unwrap();
        assert_eq!(engine.value_type(&identity).unwrap(), JsType::Function);
        let result = engine
            .call(&identity, None, std::slice::from_ref(&number))
            .unwrap();
        assert_eq!(engine.to_number(&result).unwrap(), 42.5);

        let class = engine
            .register_class(
                NativeClass::new("NativeThing").with_method(NativeMethod::new(
                    "identity",
                    Box::new(|call| Ok(call.arguments[0].clone())),
                )),
            )
            .unwrap();
        let finalized = Rc::new(Cell::new(None));
        let weak = native_weak_reference(&mut engine, &class, &finalized);
        assert!(engine.upgrade(&weak).unwrap().is_some());
        for _ in 0..3 {
            engine.collect_garbage();
        }
        // Collection is turn-dependent: ECMAScript keeps WeakRef targets alive
        // through the current job, and #87 supplies the Phase 2 outer event
        // loop boundary that clears JSC's kept-object set. If the conservative
        // native stack no longer retains this instance, its finalizer must run.
        if engine.upgrade(&weak).unwrap().is_none() {
            assert_eq!(finalized.get(), Some(blitsen_js::ExternalId(73)));
        }

        engine
            .evaluate_script(
                "globalThis.microtaskResult = 0; Promise.resolve().then(() => microtaskResult = 42)",
                "microtask-test.js",
            )
            .unwrap();
        engine.drain_microtasks().unwrap();
        let result = engine
            .evaluate_script("microtaskResult", "microtask-result.js")
            .unwrap();
        assert_eq!(engine.to_number(&result).unwrap(), 42.0);

        let error = engine
            .evaluate_script("throw new Error('expected failure')", "failure.js")
            .err()
            .expect("script should throw");
        assert!(error.to_string().contains("expected failure"));
    }
}
