//! Statically linked QuickJS-ng behind the engine-neutral [`blitsen_js::JsEngine`] trait.
//!
//! The engine an export links, and the reason it is this one: QuickJS-ng is
//! MIT, so it goes inside the executable and nothing ships beside it
//! (`LICENSING.md`). The JavaScriptCore host it replaced was LGPL, which forced
//! a dynamically loaded, replaceable library alongside every export.
//! `spikes/s8` measured what the swap was worth and what it costs.
//!
//! The implementation uses rquickjs's safe runtime, context, value, loader,
//! class, typed-array, job, interrupt, GC, and memory APIs. Three parts of the
//! engine-neutral contract drive the remaining adapter design:
//!
//! * `Value: Clone` has no JavaScript lifetime. [`rquickjs::Persistent`] owns
//!   that count outside `Context::with`, and each handle retains the safe
//!   [`rquickjs::Context`] and [`rquickjs::Runtime`] which must outlive it.
//! * `from_value` re-enters the engine from any value a callback was handed.
//!   A callback already runs under rquickjs's non-reentrant runtime lock, so
//!   nested calls reconstruct a scoped [`rquickjs::Ctx`] from that one active
//!   context. This is the sole raw/lifetime escape hatch; its dynamic lock and
//!   higher-ranked lifetime invariants are documented at the unsafe block.
//! * `instantiate` attaches an [`blitsen_js::ExternalId`] plus a finalizer that must run
//!   exactly once. An rquickjs [`rquickjs::Class`] owns that payload, and its
//!   `Drop` catches finalizer panics before the class's C finalizer returns.
//!
//! There are no direct QuickJS C calls in this crate and no direct dependency
//! on `rquickjs-sys`. Android still enables rquickjs's `bindgen` feature because
//! its transitive sys crate does not ship Android bindings.

mod context;
mod engine;
mod modules;
mod value;

pub use context::QuickJs;
pub use value::{QjsClass, QjsValue, QjsWeakRef};
