//! Runtime-neutral bridge between a DOM backend and JavaScript engine.

use blitsen_dom::DomBackend;
use blitsen_js::JsEngine;

/// Owns the two replaceable sides of the Blitsen bridge.
pub struct Bridge<D, J> {
    dom: D,
    js: J,
}

impl<D: DomBackend, J: JsEngine> Bridge<D, J> {
    /// Creates a bridge without exposing either implementation to its peer.
    pub fn new(dom: D, js: J) -> Self {
        Self { dom, js }
    }

    /// Returns the backend implementations to their owner.
    pub fn into_parts(self) -> (D, J) {
        (self.dom, self.js)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDom;
    impl DomBackend for TestDom {}

    struct TestJs;
    impl JsEngine for TestJs {}

    #[test]
    fn bridge_accepts_only_boundary_implementations() {
        let bridge = Bridge::new(TestDom, TestJs);
        let (_dom, _js) = bridge.into_parts();
    }
}
