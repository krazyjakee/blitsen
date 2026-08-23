//! The [`JsEngine`] implementation over rquickjs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blitsen_js::{
    ExternalId, JsEngine, JsError, JsType, LoopTurn, NativeCall, NativeCallback, NativeClass,
    TypedArray, TypedArrayKind,
};
use rquickjs::function::{Args, IntoJsFunc, ParamRequirement, Params, This};
use rquickjs::{
    Array, ArrayBuffer, Coerced, Constructor, Ctx, FromJs, Function, IntoJs, Module, Object, Type,
    TypedArray as RqTypedArray, U8Clamped, Value,
};

use crate::context::{Inner, QuickJs};
use crate::value::{
    QjsClass, QjsValue, QjsWeakRef, external_id as class_external_id, instance,
    typed_array_constructor,
};

struct CallbackParams;

struct CallbackAdapter {
    inner: std::rc::Weak<Inner>,
    callback: Rc<RefCell<NativeCallback<QjsValue>>>,
}

impl<'js> IntoJsFunc<'js, CallbackParams> for CallbackAdapter {
    fn param_requirements() -> ParamRequirement {
        ParamRequirement::any()
    }

    fn call<'a>(&self, params: Params<'a, 'js>) -> rquickjs::Result<Value<'js>> {
        let ctx = params.ctx().clone();
        let Some(inner) = self.inner.upgrade() else {
            return Err(QuickJs::throw(
                &ctx,
                "native callback is no longer available",
            ));
        };
        let engine = QuickJs { inner };
        let this = params.this();
        let external = class_external_id(&ctx, &this);
        let mut arguments = Vec::with_capacity(params.len());
        for index in 0..params.len() {
            arguments.push(engine.wrap(&ctx, params.arg(index).expect("argument exists")));
        }
        let call = NativeCall {
            this: engine.wrap(&ctx, this),
            arguments,
            external,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.callback.borrow_mut())(call)
        }));
        match outcome {
            Ok(Ok(value)) => value.restore(&ctx),
            Ok(Err(error)) => Err(QuickJs::throw(&ctx, error.message())),
            Err(_) => Err(QuickJs::throw(&ctx, "native callback panicked")),
        }
    }
}

impl JsEngine for QuickJs {
    type Value = QjsValue;
    type WeakRef = QjsWeakRef;
    type Class = QjsClass;

    fn from_value(value: &Self::Value) -> Self {
        Self {
            inner: Rc::clone(&value.inner),
        }
    }

    fn undefined(&mut self) -> Self::Value {
        self.with_ctx(|ctx| self.wrap(&ctx, Value::new_undefined(ctx.clone())))
    }

    fn null(&mut self) -> Self::Value {
        self.with_ctx(|ctx| self.wrap(&ctx, Value::new_null(ctx.clone())))
    }

    fn boolean(&mut self, value: bool) -> Self::Value {
        self.with_ctx(|ctx| self.wrap(&ctx, Value::new_bool(ctx.clone(), value)))
    }

    fn number(&mut self, value: f64) -> Self::Value {
        self.with_ctx(|ctx| self.wrap(&ctx, Value::new_float(ctx.clone(), value)))
    }

    fn string(&mut self, value: &str) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| Ok(self.wrap(ctx, value.into_js(ctx)?)))
    }

    fn object(&mut self) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| Ok(self.wrap(ctx, Object::new(ctx.clone())?.into_value())))
    }

    fn array(&mut self, values: &[Self::Value]) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| {
            let array = Array::new(ctx.clone())?;
            for (index, value) in values.iter().enumerate() {
                array.set(index, value.restore(ctx)?)?;
            }
            Ok(self.wrap(ctx, array.into_value()))
        })
    }

    fn typed_array(&mut self, value: &TypedArray) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| {
            // Construct through the language's own constructor. The rquickjs
            // TypedArray::from_arraybuffer helper currently produces a
            // zero-length view under quickjs-ng 0.12 for this input.
            let buffer = ArrayBuffer::new_copy(ctx.clone(), &value.bytes)?;
            let constructor: Constructor =
                ctx.globals().get(typed_array_constructor(value.kind))?;
            let result: Value = constructor.construct((buffer,))?;
            Ok(self.wrap(ctx, result))
        })
    }

    fn value_type(&mut self, value: &Self::Value) -> Result<JsType, JsError> {
        self.with_result(|ctx| {
            let value = value.restore(ctx)?;
            Ok(match value.type_of() {
                Type::Undefined => JsType::Undefined,
                Type::Null => JsType::Null,
                Type::Bool => JsType::Boolean,
                Type::Int | Type::Float => JsType::Number,
                Type::String => JsType::String,
                Type::Function | Type::Constructor => JsType::Function,
                Type::Array => JsType::Array,
                _ if is_typed_array(&value) => JsType::TypedArray,
                _ => JsType::Object,
            })
        })
    }

    fn to_boolean(&mut self, value: &Self::Value) -> Result<bool, JsError> {
        self.with_result(|ctx| Ok(Coerced::<bool>::from_js(ctx, value.restore(ctx)?)?.0))
    }

    fn to_number(&mut self, value: &Self::Value) -> Result<f64, JsError> {
        self.with_result(|ctx| Ok(Coerced::<f64>::from_js(ctx, value.restore(ctx)?)?.0))
    }

    fn to_string(&mut self, value: &Self::Value) -> Result<String, JsError> {
        self.with_result(|ctx| Ok(Coerced::<String>::from_js(ctx, value.restore(ctx)?)?.0))
    }

    fn to_array(&mut self, value: &Self::Value) -> Result<Vec<Self::Value>, JsError> {
        self.with_result(|ctx| {
            let array = Array::from_value(value.restore(ctx)?)?;
            array
                .iter::<Value>()
                .map(|value| value.map(|value| self.wrap(ctx, value)))
                .collect()
        })
    }

    fn to_typed_array(&mut self, value: &Self::Value) -> Result<TypedArray, JsError> {
        self.with_result(|ctx| typed_array_contents(ctx, value.restore(ctx)?))
    }

    fn get_property(&mut self, object: &Self::Value, name: &str) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| {
            let object = Object::from_value(object.restore(ctx)?)?;
            Ok(self.wrap(ctx, object.get(name)?))
        })
    }

    fn set_property(
        &mut self,
        object: &Self::Value,
        name: &str,
        value: &Self::Value,
    ) -> Result<(), JsError> {
        self.with_result(|ctx| {
            Object::from_value(object.restore(ctx)?)?.set(name, value.restore(ctx)?)
        })
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), JsError> {
        self.with_result(|ctx| ctx.globals().set(name, value.restore(ctx)?))
    }

    fn define_function(
        &mut self,
        name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<Self::Value, JsError> {
        let inner = Rc::downgrade(&self.inner);
        let callback = Rc::new(RefCell::new(callback));
        self.with_result(|ctx| {
            let function = Function::new(ctx.clone(), CallbackAdapter { inner, callback })?;
            function.set_name(name)?;
            Ok(self.wrap(ctx, function.into_value()))
        })
    }

    fn call(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| {
            let function = Function::from_value(function.restore(ctx)?)?;
            let mut args = Args::new(ctx.clone(), arguments.len());
            if let Some(this) = this {
                args.this(this.restore(ctx)?)?;
            }
            for argument in arguments {
                args.push_arg(argument.restore(ctx)?)?;
            }
            Ok(self.wrap(ctx, function.call_arg(args)?))
        })
    }

    fn register_class(
        &mut self,
        definition: NativeClass<Self::Value>,
    ) -> Result<Self::Class, JsError> {
        let prototype = self.object()?;
        for method in definition.methods {
            let function = self.define_function(&method.name, method.callback)?;
            self.set_property(&prototype, &method.name, &function)?;
        }
        Ok(QjsClass { prototype })
    }

    fn instantiate(
        &mut self,
        class: &Self::Class,
        external: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    ) -> Result<Self::Value, JsError> {
        self.with_result(|ctx| {
            let prototype = Object::from_value(class.prototype.restore(ctx)?)?;
            Ok(self.wrap(ctx, instance(prototype, external, finalizer)?))
        })
    }

    fn external_id(&mut self, value: &Self::Value) -> Result<ExternalId, JsError> {
        self.with_result(|ctx| {
            class_external_id(ctx, &value.restore(ctx)?).ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "value",
                    "native object",
                    "value carries no native external data",
                )
            })
        })
        .map_err(|error| {
            if error
                .message()
                .contains("value carries no native external data")
            {
                JsError::new("value carries no native external data")
            } else {
                error
            }
        })
    }

    fn detach_array_buffer(&mut self, buffer: &Self::Value) -> Result<(), JsError> {
        self.with_result(|ctx| {
            let value = buffer.restore(ctx)?;
            let object = Object::from_value(value.clone()).map_err(|_| {
                rquickjs::Error::new_from_js_message(
                    value.type_name(),
                    "ArrayBuffer",
                    "only an ArrayBuffer can be transferred",
                )
            })?;
            let constructor: Value = ctx.globals().get("ArrayBuffer")?;
            if !object.is_instance_of(&constructor) {
                return Err(rquickjs::Error::new_from_js_message(
                    value.type_name(),
                    "ArrayBuffer",
                    "only an ArrayBuffer can be transferred",
                ));
            }
            // A detached buffer no longer exposes its backing store through
            // ArrayBuffer::from_value; detaching it again is a specified no-op.
            if let Some(mut buffer) = ArrayBuffer::from_value(value) {
                buffer.detach();
            }
            Ok(())
        })
        .map_err(|error| {
            if error.message().contains("only an ArrayBuffer") {
                JsError::new("only an ArrayBuffer can be transferred")
            } else {
                error
            }
        })
    }

    fn set_interrupt_flag(&mut self, stop: Arc<AtomicBool>) -> Result<(), JsError> {
        self.inner
            .runtime
            .set_interrupt_handler(Some(Box::new(move || stop.load(Ordering::Relaxed))));
        Ok(())
    }

    fn downgrade(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        self.with_result(|ctx| {
            let constructor: Constructor = ctx.globals().get("WeakRef")?;
            let reference: Value = constructor.construct((value.restore(ctx)?,))?;
            Ok(QjsWeakRef {
                reference: self.wrap(ctx, reference),
            })
        })
    }

    fn upgrade(&mut self, reference: &Self::WeakRef) -> Result<Option<Self::Value>, JsError> {
        self.with_result(|ctx| {
            let reference = Object::from_value(reference.reference.restore(ctx)?)?;
            let deref: Function = reference.get("deref")?;
            let target: Value = deref.call((This(reference),))?;
            Ok((!target.is_undefined()).then(|| self.wrap(ctx, target)))
        })
    }

    fn evaluate_script(&mut self, source: &str, filename: &str) -> Result<Self::Value, JsError> {
        if source.contains('\0') {
            return Err(JsError::new("source contains a NUL"));
        }
        if filename.contains('\0') {
            return Err(JsError::new("filename contains a NUL"));
        }
        self.with_result(|ctx| {
            let mut options = rquickjs::context::EvalOptions::default();
            options.strict = false;
            options.filename = Some(filename.to_owned());
            let value = ctx.eval_with_options(source, options)?;
            Ok(self.wrap(ctx, value))
        })
    }

    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        if source.contains('\0') {
            return Err(JsError::new("source contains a NUL"));
        }
        if identifier.contains('\0') {
            return Err(JsError::new("identifier contains a NUL"));
        }
        self.with_result(|ctx| {
            let module = Module::declare(ctx.clone(), identifier, source)?;
            let meta = module.meta()?;
            meta.set("url", identifier)?;
            meta.set("main", false)?;
            let (_, promise) = module.eval()?;
            Ok(self.wrap(ctx, promise.into_value()))
        })
    }

    fn drain_microtasks(&mut self) -> Result<usize, JsError> {
        let mut ran = 0;
        loop {
            match self.with_runtime(|runtime| runtime.execute_pending_job()) {
                Ok(false) => return Ok(ran),
                Ok(true) => ran += 1,
                Err(error) => {
                    return error
                        .0
                        .with(|ctx| Err(self.map_error(&ctx, rquickjs::Error::Exception)));
                }
            }
        }
    }

    fn pump_event_loop(&mut self) -> Result<LoopTurn, JsError> {
        Ok(if self.drain_microtasks()? > 0 {
            LoopTurn::Progress
        } else {
            LoopTurn::Idle
        })
    }

    fn collect_garbage(&mut self) -> Result<(), JsError> {
        self.with_runtime(RuntimeExt::run_gc);
        Ok(())
    }
}

// Function item indirection keeps the closure passed to `with_runtime` simple
// enough for all supported compilers to infer.
struct RuntimeExt;
impl RuntimeExt {
    fn run_gc(runtime: &rquickjs::Runtime) {
        runtime.run_gc();
    }
}

fn is_typed_array(value: &Value<'_>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.is_typed_array::<i8>()
        || object.is_typed_array::<u8>()
        || object.is_typed_array::<U8Clamped>()
        || object.is_typed_array::<i16>()
        || object.is_typed_array::<u16>()
        || object.is_typed_array::<i32>()
        || object.is_typed_array::<u32>()
        || object.is_typed_array::<f32>()
        || object.is_typed_array::<f64>()
        || object.is_typed_array::<i64>()
        || object.is_typed_array::<u64>()
}

fn typed_array_contents<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<TypedArray> {
    macro_rules! extract {
        ($type:ty, $kind:expr) => {
            if value
                .as_object()
                .is_some_and(|object| object.is_typed_array::<$type>())
            {
                let array = RqTypedArray::<$type>::from_value(value)?;
                let bytes = array.as_bytes().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        "detached typed array",
                        "TypedArray",
                        "typed array buffer is detached",
                    )
                })?;
                return TypedArray::new($kind, bytes.to_vec()).map_err(|error| {
                    rquickjs::Error::new_from_js_message(
                        "TypedArray",
                        "TypedArray",
                        error.message(),
                    )
                });
            }
        };
    }
    extract!(i8, TypedArrayKind::Int8);
    extract!(u8, TypedArrayKind::Uint8);
    extract!(U8Clamped, TypedArrayKind::Uint8Clamped);
    extract!(i16, TypedArrayKind::Int16);
    extract!(u16, TypedArrayKind::Uint16);
    extract!(i32, TypedArrayKind::Int32);
    extract!(u32, TypedArrayKind::Uint32);
    extract!(f32, TypedArrayKind::Float32);
    extract!(f64, TypedArrayKind::Float64);
    extract!(i64, TypedArrayKind::BigInt64);
    extract!(u64, TypedArrayKind::BigUint64);
    let _ = ctx;
    Err(rquickjs::Error::new_from_js(
        value.type_name(),
        "TypedArray",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitsen_js::{NativeClass, NativeMethod};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn embedded_nul_is_preserved_across_owned_string_boundaries() {
        const TEXT: &str = "a\0b";
        let mut engine = QuickJs::new().expect("a runtime");
        let text = engine.string(TEXT).expect("a JavaScript string");
        assert_eq!(engine.to_string(&text).unwrap(), TEXT);
        let function = engine
            .define_function(TEXT, Box::new(|_| Err(JsError::new(TEXT))))
            .unwrap();
        let name = engine.get_property(&function, "name").unwrap();
        assert_eq!(engine.to_string(&name).unwrap(), TEXT);
        assert_eq!(
            engine.call(&function, None, &[]).unwrap_err().message(),
            "Error: a\0b"
        );
    }

    #[test]
    fn native_callbacks_reenter_and_panics_become_exceptions() {
        let mut engine = QuickJs::new().unwrap();
        engine
            .define_global_function(
                "reenter",
                Box::new(|call| {
                    let mut engine = QuickJs::from_value(&call.this);
                    engine.string("reentered")
                }),
            )
            .unwrap();
        let result = engine.evaluate_script("reenter()", "callback.js").unwrap();
        assert_eq!(engine.to_string(&result).unwrap(), "reentered");
        engine
            .evaluate_script(
                "queueMicrotask(() => globalThis.jobResult = reenter())",
                "job.js",
            )
            .unwrap();
        assert_eq!(engine.drain_microtasks().unwrap(), 1);
        let result = engine
            .evaluate_script("globalThis.jobResult", "job-result.js")
            .unwrap();
        assert_eq!(engine.to_string(&result).unwrap(), "reentered");

        let panic = engine
            .define_function("panic", Box::new(|_| panic!("boom")))
            .unwrap();
        assert_eq!(
            engine.call(&panic, None, &[]).unwrap_err().message(),
            "Error: native callback panicked"
        );
    }

    #[test]
    fn native_instances_finalize_once_and_expose_external_id() {
        let mut engine = QuickJs::new().unwrap();
        let class = engine
            .register_class(NativeClass::new("Probe").with_method(NativeMethod::new(
                "id",
                Box::new(|call| {
                    let mut engine = QuickJs::from_value(&call.this);
                    Ok(engine.number(call.external.unwrap().0 as f64))
                }),
            )))
            .unwrap();
        let finalized = Arc::new(AtomicUsize::new(0));
        let marker = Arc::clone(&finalized);
        let instance = engine
            .instantiate(
                &class,
                ExternalId(41),
                Some(Box::new(move |_| {
                    marker.fetch_add(1, Ordering::SeqCst);
                })),
            )
            .unwrap();
        assert_eq!(engine.external_id(&instance).unwrap(), ExternalId(41));
        engine.set_global("probe", &instance).unwrap();
        let id = engine.evaluate_script("probe.id()", "class.js").unwrap();
        assert_eq!(engine.to_number(&id).unwrap(), 41.0);
        let null = engine.null();
        engine.set_global("probe", &null).unwrap();
        drop(instance);
        engine.collect_garbage().unwrap();
        assert_eq!(finalized.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn modules_typed_arrays_detachment_jobs_and_weak_refs_work() {
        let mut engine = QuickJs::new().unwrap();
        engine
            .evaluate_module(
                "globalThis.moduleUrl = import.meta.url; queueMicrotask(() => globalThis.job = 1)",
                "blitsen:test-module",
            )
            .unwrap();
        assert!(engine.drain_microtasks().unwrap() > 0);
        let result = engine
            .evaluate_script("`${moduleUrl}:${job}`", "result.js")
            .unwrap();
        assert_eq!(engine.to_string(&result).unwrap(), "blitsen:test-module:1");

        let view = engine
            .evaluate_script(
                "new Uint16Array(new Uint16Array([99, 1, 513]).buffer, 2, 2)",
                "ta.js",
            )
            .unwrap();
        let copied = engine.to_typed_array(&view).unwrap();
        assert_eq!(copied.kind, TypedArrayKind::Uint16);
        assert_eq!(
            copied.bytes,
            [1_u16, 513]
                .into_iter()
                .flat_map(u16::to_ne_bytes)
                .collect::<Vec<_>>()
        );
        let buffer = engine.get_property(&view, "buffer").unwrap();
        engine.detach_array_buffer(&buffer).unwrap();
        engine.detach_array_buffer(&buffer).unwrap();
        let byte_length = engine.get_property(&buffer, "byteLength").unwrap();
        assert_eq!(engine.to_number(&byte_length).unwrap(), 0.0);

        let object = engine.object().unwrap();
        let weak = engine.downgrade(&object).unwrap();
        assert!(engine.upgrade(&weak).unwrap().is_some());
        drop(object);
        engine.collect_garbage().unwrap();
        assert!(engine.upgrade(&weak).unwrap().is_none());
    }

    #[test]
    fn persistent_values_keep_the_runtime_alive_without_leaking_it() {
        let value = {
            let mut engine = QuickJs::new().unwrap();
            engine.string("still alive").unwrap()
        };
        let mut recovered = QuickJs::from_value(&value);
        assert_eq!(recovered.to_string(&value).unwrap(), "still alive");
        drop(recovered);
        drop(value);
    }

    #[test]
    fn collecting_the_heap_early_returns_unreachable_cycles() {
        let mut engine = QuickJs::new().unwrap();
        engine.inner.runtime.set_gc_threshold(usize::MAX);
        let settled = engine.heap_bytes();
        engine.evaluate_script(
            "globalThis.junk=[]; for(let i=0;i<20000;i++){const n={s:'x'.repeat(64)};n.self=n;junk.push(n)} junk=null",
            "gc.js",
        ).unwrap();
        let littered = engine.heap_bytes();
        engine.collect_garbage().unwrap();
        assert!(engine.heap_bytes() < littered);
        assert!(littered > settled);
    }
}
