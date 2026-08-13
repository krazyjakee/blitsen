//! The [`JsEngine`] implementation over QuickJS-ng.
//!
//! Only the trait lives here: the handles it names are in [`crate::value`] and
//! the context state it reaches through is in [`crate::context`].

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_int, c_void};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCallback, NativeClass, TypedArray,
};
use rquickjs_sys as q;

use crate::context::{CallbackData, Inner, InstanceData, QuickJs, context_state, invoke_callback};
use crate::value::{QjsClass, QjsValue, QjsWeakRef, TYPED_ARRAY_KINDS, typed_array_constructor};

/// Stops the interpreter when the flag a worker's `terminate()` sets is raised.
///
/// Returning non-zero throws an uncatchable `InternalError` out of whatever was
/// running, which is what makes a worker stuck in a loop killable at all.
unsafe extern "C" fn interrupted(_rt: *mut q::JSRuntime, opaque: *mut c_void) -> c_int {
    let stop = unsafe { &*opaque.cast::<Arc<AtomicBool>>() };
    c_int::from(stop.load(Ordering::Relaxed))
}

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
            let set = unsafe { q::JS_SetPropertyUint32(self.ctx(), array.raw, index as u32, raw) };
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
        self.text(value.raw).ok_or_else(|| self.exception())
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
            let text = q::JS_NewStringLen(self.ctx(), named.as_ptr(), name.len() as q::size_t);
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
        let constructor = self.define_function(
            &definition.name,
            Box::new(move |_call| {
                Err(JsError::new(format!(
                    "{name} is not constructible from JavaScript"
                )))
            }),
        )?;
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

    fn detach_array_buffer(&mut self, buffer: &Self::Value) -> Result<(), JsError> {
        // Checked rather than assumed: `JS_DetachArrayBuffer` silently ignores a
        // value that is not an ArrayBuffer, and a transfer list that quietly did
        // nothing is exactly the failure this call exists to prevent.
        let mut length: q::size_t = 0;
        if unsafe { q::JS_GetArrayBuffer(self.ctx(), &mut length, buffer.raw) }.is_null() {
            // A buffer detached by an earlier transfer reports no data either,
            // and detaching it again is what the specification does anyway.
            let _ = unsafe { q::JS_GetException(self.ctx()) };
            if !unsafe { q::JS_IsObject(buffer.raw) } {
                return Err(JsError::new("only an ArrayBuffer can be transferred"));
            }
        }
        unsafe { q::JS_DetachArrayBuffer(self.ctx(), buffer.raw) };
        Ok(())
    }

    fn set_interrupt_flag(&mut self, stop: std::sync::Arc<AtomicBool>) -> Result<(), JsError> {
        // Leaked deliberately: QuickJS holds the pointer for the life of the
        // runtime, and the runtime is process-lived for the reasons
        // [`crate::context::Inner`]'s `Drop` sets out.
        let opaque = Box::into_raw(Box::new(stop)).cast::<c_void>();
        unsafe { q::JS_SetInterruptHandler(self.inner.runtime, Some(interrupted), opaque) };
        Ok(())
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
        // Compiled first, evaluated second, so `import.meta` can be filled in
        // between the two — which is the only point at which it can be, and
        // what `qjs.c` itself does. An entry module never passes through the
        // loader, so this is the one place its own address is known.
        let compiled = unsafe {
            q::JS_Eval(
                self.ctx(),
                code.as_ptr(),
                source.len() as q::size_t,
                name.as_ptr(),
                (q::JS_EVAL_TYPE_MODULE | q::JS_EVAL_FLAG_COMPILE_ONLY) as c_int,
            )
        };
        if unsafe { q::JS_IsException(compiled) } {
            return Err(self.exception());
        }
        unsafe {
            let module = q::JS_VALUE_GET_PTR(compiled).cast::<q::JSModuleDef>();
            crate::modules::set_import_meta(self.ctx(), module, name.as_ptr());
            // `JS_EvalFunction` takes the compiled module over.
            let raw = q::JS_EvalFunction(self.ctx(), compiled);
            self.checked(raw)
        }
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
