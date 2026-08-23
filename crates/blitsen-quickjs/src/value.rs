//! Persistent values, weak references, and native-class handles.

use std::rc::Rc;

use blitsen_js::{ExternalId, TypedArrayKind};
use rquickjs::class::{JsClass, Readable, Trace, Tracer};
use rquickjs::{Class, Constructor, Ctx, JsLifetime, Object, Persistent, Value};

use crate::context::Inner;

/// An owned value which keeps both its context and runtime alive.
pub struct QjsValue {
    // Drop the persistent value before its final `Inner` reference. rquickjs
    // requires every persistent to die before the runtime it belongs to.
    value: Persistent<Value<'static>>,
    pub(crate) inner: Rc<Inner>,
}

impl std::fmt::Debug for QjsValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("QjsValue(..)")
    }
}

impl QjsValue {
    pub(crate) fn new<'js>(inner: Rc<Inner>, ctx: &Ctx<'js>, value: Value<'js>) -> Self {
        Self {
            value: Persistent::save(ctx, value),
            inner,
        }
    }

    pub(crate) fn restore<'js>(&self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        self.value.clone().restore(ctx)
    }
}

impl Clone for QjsValue {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            inner: Rc::clone(&self.inner),
        }
    }
}

/// A JavaScript `WeakRef`, itself held persistently.
pub struct QjsWeakRef {
    pub(crate) reference: QjsValue,
}

/// A registered native class prototype.
#[derive(Clone)]
pub struct QjsClass {
    pub(crate) prototype: QjsValue,
}

/// Opaque data finalized by rquickjs's class machinery.
pub(crate) struct InstanceData {
    pub(crate) external: ExternalId,
    pub(crate) finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
}

impl Drop for InstanceData {
    fn drop(&mut self) {
        if let Some(finalizer) = self.finalizer.take() {
            // rquickjs class finalizers are C callbacks. Never let user code
            // unwind through that boundary.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finalizer(self.external);
            }));
        }
    }
}

// SAFETY: `InstanceData` contains no value or reference tied to a JS context;
// changing the phantom JS lifetime therefore cannot invalidate its contents.
unsafe impl<'js> JsLifetime<'js> for InstanceData {
    type Changed<'to> = InstanceData;
}

impl<'js> Trace<'js> for InstanceData {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

impl<'js> JsClass<'js> for InstanceData {
    const NAME: &'static str = "BlitsenNativeObject";
    type Mutable = Readable;

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

pub(crate) const fn typed_array_constructor(kind: TypedArrayKind) -> &'static str {
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

pub(crate) fn external_id<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<ExternalId> {
    let external = Class::<InstanceData>::from_value(value)
        .ok()
        .map(|class| class.borrow().external);
    if external.is_none() && ctx.has_exception() {
        // rquickjs's class probe uses QuickJS's checked opaque lookup, which
        // can raise while reporting "not this Rust class". The public engine
        // contract makes that a plain `None` for callback receivers and emits
        // its own diagnostic from `external_id`, so do not leave the probe's
        // internal exception pending.
        drop(ctx.catch());
    }
    external
}

pub(crate) fn instance<'js>(
    prototype: Object<'js>,
    external: ExternalId,
    finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
) -> rquickjs::Result<Value<'js>> {
    Class::instance_proto(
        InstanceData {
            external,
            finalizer,
        },
        prototype,
    )
    .map(Class::into_value)
}
