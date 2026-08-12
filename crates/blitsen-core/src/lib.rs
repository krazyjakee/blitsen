//! Runtime-neutral bridge between a DOM backend and JavaScript engine.

mod attributes;
mod bridge;
mod document;
pub mod events;
pub mod frame;
mod node;
pub mod replay;
mod scripts;
mod style;
mod window;
mod wrappers;

#[cfg(test)]
mod tests;

// The crate is split by concern; the public surface is unchanged.
pub use attributes::*;
pub use bridge::*;
pub use document::*;
pub use node::*;
pub use scripts::*;
pub use style::*;
pub use window::*;
pub use wrappers::*;
