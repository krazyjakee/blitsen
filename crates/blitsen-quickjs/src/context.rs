//! Runtime ownership and the scoped rquickjs entry point.

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use blitsen_js::JsError;
use rquickjs::{Coerced, Context, Ctx, Exception, FromJs, Runtime, Value};

use crate::value::QjsValue;

pub(crate) struct Inner {
    pub(crate) runtime: Runtime,
    pub(crate) context: Context,
    /// The context currently protected by rquickjs's runtime lock.
    active: Cell<Option<NonNull<rquickjs::qjs::JSContext>>>,
}

/// A QuickJS engine implementing Blitsen's host boundary.
#[derive(Clone)]
pub struct QuickJs {
    pub(crate) inner: Rc<Inner>,
}

impl QuickJs {
    /// Creates an rquickjs runtime and full ECMAScript context.
    pub fn new() -> Result<Self, JsError> {
        let runtime = Runtime::new().map_err(|error| JsError::new(error.to_string()))?;
        let context = Context::full(&runtime).map_err(|error| JsError::new(error.to_string()))?;
        Ok(Self {
            inner: Rc::new(Inner {
                runtime,
                context,
                active: Cell::new(None),
            }),
        })
    }

    /// Runs with the runtime lock, reusing it during a native callback.
    pub(crate) fn with_ctx<R>(&self, f: impl for<'js> FnOnce(Ctx<'js>) -> R) -> R {
        if let Some(raw) = self.inner.active.get() {
            // SAFETY: `active` is set only for the dynamic extent of
            // `Context::with` (or an rquickjs runtime operation which owns the
            // same lock). `QuickJs` is !Send, and the HRTB prevents the
            // manufactured lifetime from escaping this call.
            return f(unsafe { Ctx::from_raw(raw) });
        }

        self.inner.context.with(|ctx| {
            let previous = self.inner.active.replace(Some(ctx.as_raw()));
            let _reset = ActiveReset(&self.inner.active, previous);
            f(ctx)
        })
    }

    /// Marks jobs and GC as active runtime-lock scopes because both may invoke
    /// Rust code before their safe rquickjs methods return.
    pub(crate) fn with_runtime<R>(&self, f: impl FnOnce(&Runtime) -> R) -> R {
        let previous = self.inner.active.replace(Some(self.inner.context.as_raw()));
        let _reset = ActiveReset(&self.inner.active, previous);
        f(&self.inner.runtime)
    }

    pub(crate) fn wrap<'js>(&self, ctx: &Ctx<'js>, value: Value<'js>) -> QjsValue {
        QjsValue::new(Rc::clone(&self.inner), ctx, value)
    }

    pub(crate) fn with_result<R>(
        &self,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, JsError> {
        self.with_ctx(|ctx| f(&ctx).map_err(|error| self.map_error(&ctx, error)))
    }

    pub(crate) fn map_error<'js>(&self, ctx: &Ctx<'js>, error: rquickjs::Error) -> JsError {
        if !error.is_exception() {
            return JsError::new(error.to_string());
        }
        let thrown = ctx.catch();
        let message = Coerced::<String>::from_js(ctx, thrown.clone())
            .map(|text| text.0)
            .unwrap_or_else(|_| "unknown error".to_owned());
        let stack = thrown
            .as_object()
            .and_then(|object| {
                object
                    .get::<_, Option<Coerced<String>>>("stack")
                    .ok()
                    .flatten()
            })
            .map(|text| text.0)
            .filter(|text| !text.is_empty());
        match stack {
            Some(stack) => JsError::with_stack(message, stack),
            None => JsError::new(message),
        }
    }

    pub(crate) fn throw<'js>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Error {
        Exception::throw_message(ctx, message)
    }

    /// Bytes QuickJS has allocated and not yet returned to the allocator.
    pub fn heap_bytes(&self) -> usize {
        self.with_runtime(|runtime| runtime.memory_usage().malloc_size as usize)
    }
}

struct ActiveReset<'a>(
    &'a Cell<Option<NonNull<rquickjs::qjs::JSContext>>>,
    Option<NonNull<rquickjs::qjs::JSContext>>,
);

impl Drop for ActiveReset<'_> {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}
