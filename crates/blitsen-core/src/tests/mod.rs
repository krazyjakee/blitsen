//! Mock engines shared by the crate's tests.

mod scripts;
mod style;
mod window;
mod wrappers;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};

use blitsen_js::{ExternalId, JsError};

use super::*;

#[derive(Default)]
struct RecordingScriptEngine {
    evaluations: Vec<(String, String, String)>,
}

impl ScriptEngine for RecordingScriptEngine {
    type Value = usize;

    fn run_classic(&mut self, source: &str, identifier: &str) -> Result<usize, JsError> {
        self.evaluations
            .push(("classic".into(), source.into(), identifier.into()));
        Ok(self.evaluations.len())
    }

    fn run_module(&mut self, source: &str, identifier: &str) -> Result<usize, JsError> {
        self.evaluations
            .push(("module".into(), source.into(), identifier.into()));
        Ok(self.evaluations.len())
    }
}

type MockFinalizer = Box<dyn FnOnce(ExternalId) + 'static>;

struct MockObject {
    external: ExternalId,
    finalizer: RefCell<Option<MockFinalizer>>,
}

impl Drop for MockObject {
    fn drop(&mut self) {
        if let Some(finalizer) = self.finalizer.borrow_mut().take() {
            finalizer(self.external);
        }
    }
}

#[derive(Default)]
struct MockEngine;

impl WrapperEngine for MockEngine {
    type Value = Rc<MockObject>;
    type WeakRef = Weak<MockObject>;

    fn downgrade_wrapper(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        Ok(Rc::downgrade(value))
    }

    fn upgrade_wrapper(
        &mut self,
        reference: &Self::WeakRef,
    ) -> Result<Option<Self::Value>, JsError> {
        Ok(reference.upgrade())
    }
}

fn wrapper(external: ExternalId, finalizer: MockFinalizer) -> Rc<MockObject> {
    Rc::new(MockObject {
        external,
        finalizer: RefCell::new(Some(finalizer)),
    })
}
