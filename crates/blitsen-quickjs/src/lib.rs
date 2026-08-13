//! Statically linked QuickJS-ng behind the engine-neutral [`JsEngine`] trait.
//!
//! The counterpart to `blitsen-jsc`: same boundary, different engine, and the
//! reason for having it is what the two do *not* share. JavaScriptCore is LGPL,
//! so an export loads it dynamically and ships it alongside (`LICENSING.md`);
//! QuickJS-ng is MIT, so it links into the executable and nothing ships beside
//! it. `spikes/s8` measured what that is worth and what it costs.
//!
//! Three parts of the contract drive the design:
//!
//! * `Value: Clone` with no lifetime. QuickJS values are reference counted, so
//!   the handle owns a count and `Drop` gives it back. It also carries its
//!   `JSContext`, which is what makes the next point possible.
//! * `from_value` re-enters the engine from any value a callback was handed.
//!   The context pointer is in the handle and the engine state hangs off the
//!   context's opaque slot, so the engine is recoverable without capturing it.
//! * `instantiate` attaches an [`ExternalId`] plus a finalizer that must run
//!   exactly once. That is a QuickJS class with an opaque payload.

mod context;
mod engine;
mod modules;
mod value;

pub use context::QuickJs;
pub use value::{QjsClass, QjsValue, QjsWeakRef};
