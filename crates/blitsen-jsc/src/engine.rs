//! [`JsEngine`](blitsen_js::JsEngine) implementation over the JSC C API.

use std::{
    cell::RefCell,
    ffi::{CString, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    rc::{Rc, Weak},
    sync::OnceLock,
};

use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    TypedArray, TypedArrayKind,
};
use libloading::Library;

use crate::{Error, ffi};

struct Runtime {
    // The library must outlive every context, class, function pointer, and value.
    _library: Library,
    functions: ffi::Functions,
    context: ffi::JsGlobalContextRef,
    callback_class: ffi::JsClassRef,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: this is the retained class reference created with this API.
        unsafe { (self.functions.class_release)(self.callback_class) };
        // The global context remains process-lived. See JavaScriptCore's docs.
    }
}

enum PrivateData {
    Callback {
        runtime: Weak<Runtime>,
        callback: RefCell<NativeCallback<JscValue>>,
    },
    Instance {
        id: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    },
}

unsafe extern "C" fn finalize_private(object: ffi::JsObjectRef) {
    // SAFETY: both native callable objects and native instances store exactly
    // one Box<PrivateData> through JSObjectSetPrivate/JSObjectMake.
    let Some(get_private) = OBJECT_GET_PRIVATE.get() else {
        return;
    };
    let private = unsafe { get_private(object) };
    if private.is_null() {
        return;
    }
    // SAFETY: JSC invokes a class finalizer at most once for this allocation.
    let mut private = unsafe { Box::from_raw(private.cast::<PrivateData>()) };
    if let PrivateData::Instance { id, finalizer } = &mut *private
        && let Some(finalizer) = finalizer.take()
    {
        finalizer(*id);
    }
}

type ObjectGetPrivate = unsafe extern "C" fn(ffi::JsObjectRef) -> *mut c_void;
static OBJECT_GET_PRIVATE: OnceLock<ObjectGetPrivate> = OnceLock::new();

unsafe extern "C" fn call_native(
    context: ffi::JsContextRef,
    function: ffi::JsObjectRef,
    this: ffi::JsObjectRef,
    argument_count: usize,
    arguments: *const ffi::JsValueRef,
    exception: *mut ffi::JsValueRef,
) -> ffi::JsValueRef {
    // SAFETY: callable objects created by define_function carry PrivateData.
    let Some(get_private) = OBJECT_GET_PRIVATE.get() else {
        return ptr::null();
    };
    let pointer = unsafe { get_private(function) };
    if pointer.is_null() {
        return ptr::null();
    }
    // SAFETY: the pointer is live for the duration of the object's callback.
    let private = unsafe { &*pointer.cast::<PrivateData>() };
    let PrivateData::Callback { runtime, callback } = private else {
        return ptr::null();
    };
    let Some(runtime) = runtime.upgrade() else {
        return ptr::null();
    };
    let functions = &runtime.functions;

    let invocation = catch_unwind(AssertUnwindSafe(|| {
        let arguments = if argument_count == 0 {
            &[]
        } else {
            // SAFETY: JSC supplies argument_count live entries for this call.
            unsafe { std::slice::from_raw_parts(arguments, argument_count) }
        };
        let this = JscValue::new(Rc::clone(&runtime), this.cast_const());
        let arguments = arguments
            .iter()
            .map(|value| JscValue::new(Rc::clone(&runtime), *value))
            .collect();
        let external = runtime.external_id_raw(this.raw).ok();
        callback.borrow_mut()(NativeCall {
            this,
            arguments,
            external,
        })
    }));

    match invocation {
        Ok(Ok(value)) => value.raw,
        Ok(Err(error)) => {
            runtime.set_exception(exception, &error.to_string());
            unsafe { (functions.value_undefined)(context) }
        }
        Err(_) => {
            runtime.set_exception(exception, "native JavaScript callback panicked");
            unsafe { (functions.value_undefined)(context) }
        }
    }
}

impl Runtime {
    fn new(library: Library) -> Result<Rc<Self>, Error> {
        // SAFETY: ffi::Functions declares the public C API signatures.
        let functions = unsafe { ffi::Functions::load(&library)? };
        // SAFETY: a null class requests JSC's ordinary global object.
        let context = unsafe { (functions.global_context_create)(ptr::null_mut()) };
        if context.is_null() {
            return Err(Error::ContextCreation);
        }

        let name = CString::new("BlitsenNativeFunction").expect("static string");
        let mut definition = ffi::ClassDefinition::named(name.as_ptr());
        definition.finalize = Some(finalize_private);
        definition.call_as_function = Some(call_native);
        // SAFETY: definition has the exact JavaScriptCore C layout, and JSC
        // copies it while creating the retained class.
        let callback_class = unsafe { (functions.class_create)(&definition) };
        if callback_class.is_null() {
            return Err(Error::ContextCreation);
        }

        let runtime = Rc::new(Self {
            _library: library,
            functions,
            context,
            callback_class,
        });
        let _ = OBJECT_GET_PRIVATE.set(runtime.functions.object_get_private);
        Ok(runtime)
    }

    fn js_string(&self, value: &str) -> Result<ffi::JsStringRef, JsError> {
        let value = CString::new(value)
            .map_err(|_| JsError::new("JavaScript string contains an interior NUL"))?;
        // SAFETY: CString is NUL terminated and the returned string is owned.
        let string = unsafe { (self.functions.string_create_utf8)(value.as_ptr()) };
        if string.is_null() {
            Err(JsError::new("JavaScriptCore could not allocate a string"))
        } else {
            Ok(string)
        }
    }

    fn string_to_rust(&self, string: ffi::JsStringRef) -> String {
        // SAFETY: string is a live JSC string.
        let capacity = unsafe { (self.functions.string_max_utf8)(string) };
        let mut bytes = vec![0_u8; capacity.max(1)];
        // SAFETY: bytes owns capacity writable bytes.
        let written = unsafe {
            (self.functions.string_get_utf8)(string, bytes.as_mut_ptr().cast(), bytes.len())
        };
        let content = written.saturating_sub(1).min(bytes.len());
        bytes.truncate(content);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn value_to_string_raw(&self, value: ffi::JsValueRef) -> Result<String, JsError> {
        let mut exception = ptr::null();
        // SAFETY: value belongs to this context and exception is writable.
        let string =
            unsafe { (self.functions.value_to_string)(self.context, value, &mut exception) };
        if !exception.is_null() {
            return Err(JsError::new("JavaScript string conversion threw"));
        }
        let result = self.string_to_rust(string);
        // SAFETY: value_to_string returned an owned JSStringRef.
        unsafe { (self.functions.string_release)(string) };
        Ok(result)
    }

    fn exception(&self, value: ffi::JsValueRef) -> JsError {
        let message = self
            .value_to_string_raw(value)
            .unwrap_or_else(|_| "JavaScript exception".to_owned());
        let stack = self
            .object_from_raw(value)
            .ok()
            .and_then(|object| self.get_property_raw(object, "stack").ok())
            .and_then(|stack| self.value_to_string_raw(stack).ok());
        if let Some(stack) = stack {
            JsError::with_stack(message, stack)
        } else {
            JsError::new(message)
        }
    }

    fn set_exception(&self, output: *mut ffi::JsValueRef, message: &str) {
        if output.is_null() {
            return;
        }
        let Ok(string) = self.js_string(message) else {
            return;
        };
        // SAFETY: output is supplied by JSC for this callback and string is live.
        unsafe {
            *output = (self.functions.value_string)(self.context, string);
            (self.functions.string_release)(string);
        }
    }

    fn object_from_raw(&self, value: ffi::JsValueRef) -> Result<ffi::JsObjectRef, JsError> {
        let mut exception = ptr::null();
        // SAFETY: value belongs to the context and exception is writable.
        let object =
            unsafe { (self.functions.value_to_object)(self.context, value, &mut exception) };
        if exception.is_null() && !object.is_null() {
            Ok(object)
        } else if !exception.is_null() {
            Err(self.exception(exception))
        } else {
            Err(JsError::new("value is not an object"))
        }
    }

    fn get_property_raw(
        &self,
        object: ffi::JsObjectRef,
        name: &str,
    ) -> Result<ffi::JsValueRef, JsError> {
        let name = self.js_string(name)?;
        let mut exception = ptr::null();
        // SAFETY: object/name belong to this API and exception is writable.
        let value = unsafe {
            (self.functions.object_get_property)(self.context, object, name, &mut exception)
        };
        // SAFETY: name is an owned temporary.
        unsafe { (self.functions.string_release)(name) };
        if exception.is_null() {
            Ok(value)
        } else {
            Err(self.exception(exception))
        }
    }

    fn set_property_raw(
        &self,
        object: ffi::JsObjectRef,
        name: &str,
        value: ffi::JsValueRef,
    ) -> Result<(), JsError> {
        let name = self.js_string(name)?;
        let mut exception = ptr::null();
        // SAFETY: all handles belong to this context; attributes 0 are ordinary.
        unsafe {
            (self.functions.object_set_property)(
                self.context,
                object,
                name,
                value,
                0,
                &mut exception,
            );
            (self.functions.string_release)(name);
        }
        if exception.is_null() {
            Ok(())
        } else {
            Err(self.exception(exception))
        }
    }

    fn external_id_raw(&self, value: ffi::JsValueRef) -> Result<ExternalId, JsError> {
        let object = self.object_from_raw(value)?;
        // SAFETY: object is live; only Blitsen class instances have our private data.
        let private = unsafe { (self.functions.object_get_private)(object) };
        if private.is_null() {
            return Err(JsError::new("object has no native instance data"));
        }
        // SAFETY: native Blitsen objects store PrivateData allocations.
        match unsafe { &*private.cast::<PrivateData>() } {
            PrivateData::Instance { id, .. } => Ok(*id),
            PrivateData::Callback { .. } => Err(JsError::new("object is a native function")),
        }
    }
}

/// A protected JavaScriptCore value tied to its process-lived runtime.
pub struct JscValue {
    runtime: Rc<Runtime>,
    raw: ffi::JsValueRef,
}

impl JscValue {
    fn new(runtime: Rc<Runtime>, raw: ffi::JsValueRef) -> Self {
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
pub struct JavaScriptCore {
    runtime: Rc<Runtime>,
}

impl Drop for JavaScriptCore {
    fn drop(&mut self) {
        // The pinned Bun JSC context cannot currently be released without an
        // atom-table teardown assertion. Keep one strong runtime/library owner
        // beside that deliberately process-lived context.
        std::mem::forget(Rc::clone(&self.runtime));
    }
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
