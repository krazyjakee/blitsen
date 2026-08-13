//! Value, weak-reference and class handles, and the typed-array tables.
//!
//! Split from the engine itself because these are the types the trait names:
//! everything here is a handle the host may hold, with no engine behaviour of
//! its own beyond the reference counting `Clone` and `Drop` owe QuickJS.

use blitsen_js::TypedArrayKind;
use rquickjs_sys as q;

/// An owned QuickJS value handle.
///
/// Carries the context because the trait's `from_value` has to rebuild the
/// engine from nothing else, and because freeing a value needs it anyway.
pub struct QjsValue {
    pub(crate) ctx: *mut q::JSContext,
    pub(crate) raw: q::JSValue,
}

impl QjsValue {
    /// Takes ownership of a value the engine just returned.
    ///
    /// # Safety
    /// `raw` must be an owned reference produced by `ctx`.
    pub(crate) unsafe fn own(ctx: *mut q::JSContext, raw: q::JSValue) -> Self {
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
    pub(crate) reference: QjsValue,
}

/// A registered native class.
#[derive(Clone)]
pub struct QjsClass {
    pub(crate) id: q::JSClassID,
    pub(crate) constructor: QjsValue,
}

/// The JavaScript constructor that builds each kind.
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

pub(crate) const TYPED_ARRAY_KINDS: [(TypedArrayKind, q::JSTypedArrayEnum); 11] = [
    (
        TypedArrayKind::Int8,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT8,
    ),
    (
        TypedArrayKind::Uint8,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8,
    ),
    (
        TypedArrayKind::Uint8Clamped,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8C,
    ),
    (
        TypedArrayKind::Int16,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT16,
    ),
    (
        TypedArrayKind::Uint16,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT16,
    ),
    (
        TypedArrayKind::Int32,
        q::JSTypedArrayEnum_JS_TYPED_ARRAY_INT32,
    ),
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
