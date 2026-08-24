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
struct RecordingEvaluations {
    evaluations: Vec<(String, String, String)>,
}

impl RecordingEvaluations {
    fn evaluate(&mut self, module: bool, source: &str, identifier: &str) -> Result<usize, JsError> {
        self.evaluations.push((
            (if module { "module" } else { "classic" }).into(),
            source.into(),
            identifier.into(),
        ));
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

impl MockEngine {
    fn downgrade(&mut self, value: &Rc<MockObject>) -> Result<Weak<MockObject>, JsError> {
        Ok(Rc::downgrade(value))
    }

    fn upgrade(&mut self, reference: &Weak<MockObject>) -> Result<Option<Rc<MockObject>>, JsError> {
        Ok(reference.upgrade())
    }
}

fn wrapper(external: ExternalId, finalizer: MockFinalizer) -> Rc<MockObject> {
    Rc::new(MockObject {
        external,
        finalizer: RefCell::new(Some(finalizer)),
    })
}
