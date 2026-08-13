//! The JavaScriptCore context, and the private data attached to its objects.
//!
//! One process-lived runtime owns the context; everything that reaches into
//! JSC's C API through raw pointers is confined here.

use std::{
    cell::RefCell,
    ffi::{CString, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    rc::{Rc, Weak},
    sync::OnceLock,
};

use blitsen_js::{ExternalId, JsError, NativeCall, NativeCallback};
use libloading::Library;

use crate::engine::JscValue;
use crate::{Error, ffi};

pub(crate) struct Runtime {
    // The library must outlive every context, class, function pointer, and value.
    pub(crate) _library: Library,
    pub(crate) functions: ffi::Functions,
    pub(crate) context: ffi::JsGlobalContextRef,
    pub(crate) callback_class: ffi::JsClassRef,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: this is the retained class reference created with this API.
        unsafe { (self.functions.class_release)(self.callback_class) };
        // The global context remains process-lived. See JavaScriptCore's docs.
    }
}

pub(crate) enum PrivateData {
    Callback {
        runtime: Weak<Runtime>,
        callback: RefCell<NativeCallback<JscValue>>,
    },
    Instance {
        id: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    },
}

pub(crate) unsafe extern "C" fn finalize_private(object: ffi::JsObjectRef) {
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
    pub(crate) fn new(library: Library) -> Result<Rc<Self>, Error> {
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
        // The pinned Bun JSC context cannot currently be released without an
        // atom-table teardown assertion, and unloading the library under a live
        // context would be invalid for the same reason. One strong reference is
        // leaked here, once, so no arrangement of engine views can drop either.
        std::mem::forget(Rc::clone(&runtime));
        Ok(runtime)
    }

    pub(crate) fn js_string(&self, value: &str) -> Result<ffi::JsStringRef, JsError> {
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

    pub(crate) fn string_to_rust(&self, string: ffi::JsStringRef) -> String {
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

    pub(crate) fn value_to_string_raw(&self, value: ffi::JsValueRef) -> Result<String, JsError> {
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

    pub(crate) fn exception(&self, value: ffi::JsValueRef) -> JsError {
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

    pub(crate) fn set_exception(&self, output: *mut ffi::JsValueRef, message: &str) {
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

    pub(crate) fn object_from_raw(
        &self,
        value: ffi::JsValueRef,
    ) -> Result<ffi::JsObjectRef, JsError> {
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

    pub(crate) fn get_property_raw(
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

    pub(crate) fn set_property_raw(
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

    pub(crate) fn external_id_raw(&self, value: ffi::JsValueRef) -> Result<ExternalId, JsError> {
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
