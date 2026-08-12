//! Mock backend and engine shared by the crate's tests.

mod dom;
mod scripts;
mod window;
mod wrappers;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};

use blitsen_dom::{DomError, NodeId};
use blitsen_js::{ExternalId, JsError};

use super::*;

#[derive(Default)]
struct MockDocument {
    next_node: u32,
    matches: Vec<u32>,
    queried_selectors: RefCell<Vec<String>>,
}

impl DocumentBackend for MockDocument {
    type NodeId = u32;

    fn document_query_selector(&self, selector: &str) -> Result<Option<u32>, DomError> {
        self.queried_selectors.borrow_mut().push(selector.into());
        Ok(self.matches.first().copied())
    }

    fn document_query_selector_all(&self, selector: &str) -> Result<Vec<u32>, DomError> {
        self.queried_selectors.borrow_mut().push(selector.into());
        Ok(self.matches.clone())
    }

    fn document_get_element_by_id(&self, id: &str) -> Result<Option<u32>, DomError> {
        Ok((id == "target").then_some(2))
    }

    fn document_create_element(&mut self, local_name: &str) -> Result<u32, DomError> {
        assert_eq!(local_name, "section");
        self.next_node += 1;
        Ok(self.next_node)
    }

    fn document_create_text(&mut self, text: &str) -> Result<u32, DomError> {
        assert_eq!(text, "hello");
        self.next_node += 1;
        Ok(self.next_node)
    }

    fn document_body(&self) -> Option<u32> {
        Some(10)
    }

    fn document_element(&self) -> Option<u32> {
        Some(1)
    }
}

#[derive(Default)]
struct MockTree {
    parents: HashMap<u32, u32>,
    children: HashMap<u32, Vec<u32>>,
}

impl MockTree {
    fn detach(&mut self, node: u32) {
        if let Some(parent) = self.parents.remove(&node) {
            self.children
                .get_mut(&parent)
                .unwrap()
                .retain(|id| *id != node);
        }
    }
}

impl NodeTreeBackend for MockTree {
    type NodeId = u32;

    fn node_append(&mut self, parent: u32, child: u32) -> Result<(), DomError> {
        self.detach(child);
        self.parents.insert(child, parent);
        self.children.entry(parent).or_default().push(child);
        Ok(())
    }

    fn node_insert_before(
        &mut self,
        parent: u32,
        child: u32,
        reference: Option<u32>,
    ) -> Result<(), DomError> {
        if let Some(reference) = reference
            && self.parents.get(&reference) != Some(&parent)
        {
            return Err(DomError::NotFound);
        }
        self.detach(child);
        let children = self.children.entry(parent).or_default();
        let index = reference
            .map(|reference| children.iter().position(|id| *id == reference).unwrap())
            .unwrap_or(children.len());
        children.insert(index, child);
        self.parents.insert(child, parent);
        Ok(())
    }

    fn node_remove(&mut self, node: u32) -> Result<(), DomError> {
        self.detach(node);
        Ok(())
    }

    fn node_replace(&mut self, old: u32, replacement: u32) -> Result<(), DomError> {
        let parent = self.parents.get(&old).copied().ok_or(DomError::NotFound)?;
        self.detach(replacement);
        let index = self.children[&parent]
            .iter()
            .position(|id| *id == old)
            .unwrap();
        self.detach(old);
        self.children
            .get_mut(&parent)
            .unwrap()
            .insert(index, replacement);
        self.parents.insert(replacement, parent);
        Ok(())
    }

    fn node_parent(&self, node: u32) -> Result<Option<u32>, DomError> {
        Ok(self.parents.get(&node).copied())
    }

    fn node_children(&self, node: u32) -> Result<Vec<u32>, DomError> {
        Ok(self.children.get(&node).cloned().unwrap_or_default())
    }

    fn node_next_sibling(&self, node: u32) -> Result<Option<u32>, DomError> {
        let Some(parent) = self.parents.get(&node) else {
            return Ok(None);
        };
        let children = &self.children[parent];
        let index = children.iter().position(|id| *id == node).unwrap();
        Ok(children.get(index + 1).copied())
    }
}

struct MockContent {
    text: String,
    html: String,
    invalidations: usize,
}

struct MockScripts(Vec<DocumentScript>);

impl ScriptDocument for MockScripts {
    fn document_scripts(&self) -> Result<Vec<DocumentScript>, DomError> {
        Ok(self.0.clone())
    }
}

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

#[derive(Default)]
struct MockAttributes {
    values: HashMap<String, String>,
    restyles: usize,
}

#[derive(Default)]
struct MockStyle {
    properties: HashMap<String, String>,
}

impl InlineStyleBackend for MockStyle {
    type NodeId = u32;

    fn style_property(&self, _node: u32, property: &str) -> Result<Option<String>, DomError> {
        Ok(self.properties.get(property).cloned())
    }

    fn style_set_property(
        &mut self,
        _node: u32,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError> {
        if value == "invalid" {
            return Ok(false);
        }
        self.properties.insert(property.into(), value.into());
        Ok(true)
    }

    fn style_remove_property(
        &mut self,
        _node: u32,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        Ok(self.properties.remove(property))
    }

    fn style_css_text(&self, _node: u32) -> Result<String, DomError> {
        let mut declarations: Vec<_> = self.properties.iter().collect();
        declarations.sort_by_key(|(name, _)| *name);
        Ok(declarations
            .into_iter()
            .map(|(name, value)| format!("{name}: {value};"))
            .collect::<Vec<_>>()
            .join(" "))
    }

    fn style_set_css_text(&mut self, _node: u32, css: &str) -> Result<(), DomError> {
        self.properties.clear();
        for declaration in css.split(';') {
            if let Some((name, value)) = declaration.split_once(':') {
                self.style_set_property(0, name.trim(), value.trim())?;
            }
        }
        Ok(())
    }
}

impl AttributeBackend for MockAttributes {
    type NodeId = u32;

    fn element_attribute(&self, _node: u32, name: &str) -> Result<Option<String>, DomError> {
        Ok(self.values.get(name).cloned())
    }

    fn element_set_attribute(
        &mut self,
        _node: u32,
        name: &str,
        value: &str,
    ) -> Result<(), DomError> {
        self.values.insert(name.into(), value.into());
        self.restyles += 1;
        Ok(())
    }

    fn element_remove_attribute(&mut self, _node: u32, name: &str) -> Result<bool, DomError> {
        let removed = self.values.remove(name).is_some();
        self.restyles += usize::from(removed);
        Ok(removed)
    }
}

impl NodeContentBackend for MockContent {
    type NodeId = u32;

    fn content_text(&self, _node: u32) -> Result<String, DomError> {
        Ok(self.text.clone())
    }

    fn content_set_text(&mut self, _node: u32, text: &str) -> Result<(), DomError> {
        self.text = text.into();
        self.html = if text.is_empty() {
            String::new()
        } else {
            text.replace('&', "&amp;").replace('<', "&lt;")
        };
        self.invalidations += 1;
        Ok(())
    }

    fn content_inner_html(&self, _node: u32) -> Result<String, DomError> {
        Ok(self.html.clone())
    }

    fn content_set_inner_html(&mut self, _node: u32, html: &str) -> Result<(), DomError> {
        self.html = html.into();
        self.text = html
            .replace("<span>", "")
            .replace("</span>", "")
            .replace("&amp;", "&");
        self.invalidations += 1;
        Ok(())
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
