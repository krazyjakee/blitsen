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

    // JsEngine deliberately has no default operations: every host must make an
    // explicit choice for the complete boundary. DomBackend is still a marker
    // until issue #15 defines its surface.
    fn assert_dom_boundary<T: DomBackend>() {}

    #[test]
    fn bridge_dom_type_is_runtime_neutral() {
        assert_dom_boundary::<TestDom>();
    }
}
