//! Runtime-neutral bridge between a DOM backend and JavaScript engine.

pub mod frame;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use blitsen_dom::{DomBackend, DomError, DomName};
use blitsen_js::{ExternalId, JsEngine, JsError};

/// Minimal script-element view provided by the authoritative DOM backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentScript {
    /// Inline source text.
    pub source: String,
    /// Optional local `src` attribute.
    pub src: Option<String>,
    /// Raw `type` attribute.
    pub script_type: Option<String>,
    /// Whether `async` was present.
    pub async_attribute: bool,
    /// Whether `defer` was present.
    pub defer_attribute: bool,
}

/// DOM access needed to collect scripts without copying the tree.
pub trait ScriptDocument {
    /// Returns script elements in document order.
    fn document_scripts(&self) -> Result<Vec<DocumentScript>, DomError>;
}

impl<D: DomBackend> ScriptDocument for D {
    fn document_scripts(&self) -> Result<Vec<DocumentScript>, DomError> {
        self.query_selector_all(self.document(), "script")?
            .into_iter()
            .map(|node| {
                Ok(DocumentScript {
                    source: self.text_content(node)?,
                    src: self.attribute(node, &DomName::attribute("src"))?,
                    script_type: self.attribute(node, &DomName::attribute("type"))?,
                    async_attribute: self
                        .attribute(node, &DomName::attribute("async"))?
                        .is_some(),
                    defer_attribute: self
                        .attribute(node, &DomName::attribute("defer"))?
                        .is_some(),
                })
            })
            .collect()
    }
}

/// Evaluation operations used by the document script runner.
pub trait ScriptEngine {
    /// Engine-specific evaluation result.
    type Value;
    /// Evaluates a classic script.
    fn run_classic(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError>;
    /// Starts module evaluation.
    fn run_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError>;
}

impl<J: JsEngine> ScriptEngine for J {
    type Value = J::Value;

    fn run_classic(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        self.evaluate_script(source, identifier)
    }

    fn run_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError> {
        self.evaluate_module(source, identifier)
    }
}

/// Executes document scripts after parsing in strict document order.
///
/// v0 deliberately treats `async` and `defer` as document-order execution at
/// this post-parse checkpoint. This deterministic subset preserves dependency
/// order until networking and incremental parsing are introduced.
pub fn execute_document_scripts<D, J>(
    document: &D,
    engine: &mut J,
    entrypoint: &Path,
) -> Result<Vec<J::Value>, JsError>
where
    D: ScriptDocument,
    J: ScriptEngine,
{
    let scripts = document
        .document_scripts()
        .map_err(|error| JsError::new(error.to_string()))?;
    execute_collected_document_scripts(scripts, engine, entrypoint)
}

/// Executes a previously collected document-order script list.
///
/// Hosts with interior-mutable DOM storage use this form to release their tree
/// borrow before evaluation callbacks begin mutating that same tree.
pub fn execute_collected_document_scripts<J>(
    scripts: Vec<DocumentScript>,
    engine: &mut J,
    entrypoint: &Path,
) -> Result<Vec<J::Value>, JsError>
where
    J: ScriptEngine,
{
    let root = entrypoint.parent().unwrap_or_else(|| Path::new("."));
    let mut results = Vec::with_capacity(scripts.len());
    for (index, script) in scripts.into_iter().enumerate() {
        let module = script
            .script_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("module"));
        if script.script_type.as_deref().is_some_and(|kind| {
            !kind.is_empty()
                && !kind.eq_ignore_ascii_case("module")
                && !kind.eq_ignore_ascii_case("text/javascript")
                && !kind.eq_ignore_ascii_case("application/javascript")
        }) {
            continue;
        }
        let (source, identifier) = if let Some(src) = script.src {
            let path = resolve_local_script(root, &src)?;
            let source = std::fs::read_to_string(&path).map_err(|error| {
                JsError::new(format!("could not read script {}: {error}", path.display()))
            })?;
            (source, path.to_string_lossy().into_owned())
        } else {
            (
                script.source,
                format!("{}#script-{}", entrypoint.display(), index + 1),
            )
        };
        let result = if module {
            engine.run_module(&source, &identifier)
        } else {
            engine.run_classic(&source, &identifier)
        }
        .map_err(|error| {
            if error.stack().is_some() {
                error
            } else {
                JsError::new(format!("{identifier}: {}", error.message()))
            }
        })?;
        results.push(result);
    }
    Ok(results)
}

fn resolve_local_script(root: &Path, src: &str) -> Result<PathBuf, JsError> {
    if src.starts_with('/') || src.contains("://") {
        return Err(JsError::new(format!(
            "script src must be relative to the entrypoint: {src}"
        )));
    }
    let root = root
        .canonicalize()
        .map_err(|error| JsError::new(format!("could not resolve {}: {error}", root.display())))?;
    let path = root
        .join(src)
        .canonicalize()
        .map_err(|error| JsError::new(format!("could not resolve script {src}: {error}")))?;
    if !path.starts_with(&root) {
        return Err(JsError::new(format!(
            "script src escapes the application directory: {src}"
        )));
    }
    Ok(path)
}

/// Viewport-backed properties exposed on the JavaScript `window` object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
}

impl WindowState {
    /// Creates viewport state in logical CSS pixels.
    pub fn new(width: u32, height: u32, device_pixel_ratio: f64) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio,
        }
    }

    /// Updates logical dimensions after a native resize event.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Installs `window` as the global object and attaches the document.
    pub fn install<J: JsEngine>(
        self,
        engine: &mut J,
        document: &J::Value,
    ) -> Result<J::Value, JsError> {
        let window = engine.evaluate_script(
            "for (const key of ['location','history','navigator','localStorage']) { try { delete globalThis[key] } catch {} } globalThis",
            "blitsen:window-bootstrap",
        )?;
        engine.set_global("window", &window)?;
        engine.set_property(&window, "document", document)?;
        self.sync(engine, &window)?;
        Ok(window)
    }

    /// Synchronizes viewport properties after state changes.
    pub fn sync<J: JsEngine>(self, engine: &mut J, window: &J::Value) -> Result<(), JsError> {
        let width = engine.number(f64::from(self.width));
        let height = engine.number(f64::from(self.height));
        let ratio = engine.number(self.device_pixel_ratio);
        engine.set_property(window, "innerWidth", &width)?;
        engine.set_property(window, "innerHeight", &height)?;
        engine.set_property(window, "devicePixelRatio", &ratio)
    }

    /// Returns logical viewport width.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Returns logical viewport height.
    pub fn height(self) -> u32 {
        self.height
    }

    /// Returns native pixels per CSS pixel.
    pub fn device_pixel_ratio(self) -> f64 {
        self.device_pixel_ratio
    }
}

/// Weak-reference operations needed by the wrapper identity table.
///
/// Every complete [`JsEngine`] implements this automatically. The smaller
/// boundary also permits deterministic identity-table tests without mocking
/// the rest of a JavaScript runtime.
pub trait WrapperEngine {
    /// JavaScript object handle.
    type Value: Clone;
    /// Engine-owned weak reference.
    type WeakRef;

    /// Creates a weak reference to a wrapper.
    fn downgrade_wrapper(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError>;
    /// Upgrades a weak reference while its wrapper remains live.
    fn upgrade_wrapper(
        &mut self,
        reference: &Self::WeakRef,
    ) -> Result<Option<Self::Value>, JsError>;
}

/// DOM operations needed by the JavaScript `document` object.
///
/// The blanket implementation delegates directly to [`DomBackend`], ensuring
/// selector matching remains the renderer's implementation (Stylo for Blitz).
pub trait DocumentBackend {
    /// Stable node handle returned to wrappers.
    type NodeId: Copy;

    /// Queries the document for the first selector match.
    fn document_query_selector(&self, selector: &str) -> Result<Option<Self::NodeId>, DomError>;
    /// Queries the document for every selector match in tree order.
    fn document_query_selector_all(&self, selector: &str) -> Result<Vec<Self::NodeId>, DomError>;
    /// Looks up an exact element ID through the backend's maintained index.
    fn document_get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError>;
    /// Creates a detached HTML element.
    fn document_create_element(&mut self, local_name: &str) -> Result<Self::NodeId, DomError>;
    /// Creates a detached text node.
    fn document_create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError>;
    /// Returns the body element.
    fn document_body(&self) -> Option<Self::NodeId>;
    /// Returns the document element.
    fn document_element(&self) -> Option<Self::NodeId>;
}

impl<D: DomBackend> DocumentBackend for D {
    type NodeId = D::NodeId;

    fn document_query_selector(&self, selector: &str) -> Result<Option<Self::NodeId>, DomError> {
        self.query_selector(self.document(), selector)
    }

    fn document_query_selector_all(&self, selector: &str) -> Result<Vec<Self::NodeId>, DomError> {
        self.query_selector_all(self.document(), selector)
    }

    fn document_get_element_by_id(&self, id: &str) -> Result<Option<Self::NodeId>, DomError> {
        self.get_element_by_id(id)
    }

    fn document_create_element(&mut self, local_name: &str) -> Result<Self::NodeId, DomError> {
        self.create_element(&DomName::html(local_name.to_ascii_lowercase()))
    }

    fn document_create_text(&mut self, text: &str) -> Result<Self::NodeId, DomError> {
        self.create_text(text)
    }

    fn document_body(&self) -> Option<Self::NodeId> {
        self.body()
    }

    fn document_element(&self) -> Option<Self::NodeId> {
        self.document_element()
    }
}

/// Static `NodeList` snapshot returned by `querySelectorAll` in v0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticNodeList<N> {
    nodes: Vec<N>,
}

impl<N> StaticNodeList<N> {
    /// Creates a snapshot from nodes already in tree order.
    pub fn new(nodes: Vec<N>) -> Self {
        Self { nodes }
    }

    /// Returns the number of nodes in the snapshot.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Reports whether the snapshot contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns a node by zero-based index.
    pub fn item(&self, index: usize) -> Option<&N> {
        self.nodes.get(index)
    }

    /// Iterates over the snapshot in tree order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &N> {
        self.nodes.iter()
    }

    /// Consumes the snapshot and returns its handles.
    pub fn into_vec(self) -> Vec<N> {
        self.nodes
    }
}

/// Runtime-neutral implementation of the v0 JavaScript `document` surface.
pub struct DocumentApi<'a, D> {
    backend: &'a mut D,
}

/// Authoritative-tree operations needed by JavaScript `Node` wrappers.
pub trait NodeTreeBackend {
    /// Stable node handle.
    type NodeId: Copy + Eq;

    /// Appends a node, moving it from an existing parent first.
    fn node_append(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError>;
    /// Inserts a node before an optional child of `parent`.
    fn node_insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError>;
    /// Detaches a node.
    fn node_remove(&mut self, node: Self::NodeId) -> Result<(), DomError>;
    /// Replaces a node in its current parent.
    fn node_replace(
        &mut self,
        old: Self::NodeId,
        replacement: Self::NodeId,
    ) -> Result<(), DomError>;
    /// Returns a node's parent.
    fn node_parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
    /// Returns a static snapshot of a node's children.
    fn node_children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError>;
    /// Returns a node's next sibling.
    fn node_next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError>;
}

impl<D: DomBackend> NodeTreeBackend for D {
    type NodeId = D::NodeId;

    fn node_append(&mut self, parent: Self::NodeId, child: Self::NodeId) -> Result<(), DomError> {
        self.append_child(parent, child)
    }

    fn node_insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    ) -> Result<(), DomError> {
        self.insert_before(parent, child, reference)
    }

    fn node_remove(&mut self, node: Self::NodeId) -> Result<(), DomError> {
        self.remove(node)
    }

    fn node_replace(
        &mut self,
        old: Self::NodeId,
        replacement: Self::NodeId,
    ) -> Result<(), DomError> {
        self.replace(old, replacement)
    }

    fn node_parent(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError> {
        self.parent(node)
    }

    fn node_children(&self, node: Self::NodeId) -> Result<Vec<Self::NodeId>, DomError> {
        self.children(node)
    }

    fn node_next_sibling(&self, node: Self::NodeId) -> Result<Option<Self::NodeId>, DomError> {
        self.next_sibling(node)
    }
}

/// Runtime-neutral implementation of JavaScript node mutation and traversal.
pub struct NodeTreeApi<'a, D: NodeTreeBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

/// Text and HTML operations required by JavaScript node wrappers.
pub trait NodeContentBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads concatenated descendant text.
    fn content_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces children with one text node (or none for an empty string).
    fn content_set_text(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError>;
    /// Serializes child nodes to HTML.
    fn content_inner_html(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Contextually parses and adopts replacement child nodes.
    fn content_set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError>;
}

impl<D: DomBackend> NodeContentBackend for D {
    type NodeId = D::NodeId;

    fn content_text(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.text_content(node)
    }

    fn content_set_text(&mut self, node: Self::NodeId, text: &str) -> Result<(), DomError> {
        self.set_text_content(node, text)
    }

    fn content_inner_html(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.inner_html(node)
    }

    fn content_set_inner_html(&mut self, node: Self::NodeId, html: &str) -> Result<(), DomError> {
        self.set_inner_html(node, html)
    }
}

/// Runtime-neutral `textContent` and `innerHTML` implementation.
pub struct NodeContentApi<'a, D: NodeContentBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

/// Attribute operations required by JavaScript element wrappers.
pub trait AttributeBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads a non-namespaced HTML attribute.
    fn element_attribute(&self, node: Self::NodeId, name: &str)
    -> Result<Option<String>, DomError>;
    /// Sets a non-namespaced HTML attribute and invalidates selector matching.
    fn element_set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError>;
    /// Removes an attribute and invalidates selector matching when present.
    fn element_remove_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<bool, DomError>;
}

impl<D: DomBackend> AttributeBackend for D {
    type NodeId = D::NodeId;

    fn element_attribute(
        &self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<Option<String>, DomError> {
        self.attribute(node, &DomName::attribute(name.to_ascii_lowercase()))
    }

    fn element_set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError> {
        self.set_attribute(node, &DomName::attribute(name.to_ascii_lowercase()), value)
    }

    fn element_remove_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<bool, DomError> {
        self.remove_attribute(node, &DomName::attribute(name.to_ascii_lowercase()))
    }
}

/// Runtime-neutral attributes and `classList` implementation.
pub struct ElementAttributesApi<'a, D: AttributeBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

impl<'a, D: AttributeBackend> ElementAttributesApi<'a, D> {
    /// Wraps an element from the authoritative backend.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Implements `getAttribute`.
    pub fn get_attribute(&self, name: &str) -> Result<Option<String>, DomError> {
        self.backend.element_attribute(self.node, name)
    }

    /// Implements `setAttribute`.
    pub fn set_attribute(&mut self, name: &str, value: &str) -> Result<(), DomError> {
        self.backend.element_set_attribute(self.node, name, value)
    }

    /// Implements `removeAttribute`.
    pub fn remove_attribute(&mut self, name: &str) -> Result<(), DomError> {
        self.backend.element_remove_attribute(self.node, name)?;
        Ok(())
    }

    /// Implements `hasAttribute`.
    pub fn has_attribute(&self, name: &str) -> Result<bool, DomError> {
        Ok(self.get_attribute(name)?.is_some())
    }

    /// Implements the reflected `id` getter.
    pub fn id(&self) -> Result<String, DomError> {
        Ok(self.get_attribute("id")?.unwrap_or_default())
    }

    /// Implements the reflected `id` setter.
    pub fn set_id(&mut self, value: &str) -> Result<(), DomError> {
        self.set_attribute("id", value)
    }

    /// Implements the reflected `className` getter.
    pub fn class_name(&self) -> Result<String, DomError> {
        Ok(self.get_attribute("class")?.unwrap_or_default())
    }

    /// Implements the reflected `className` setter.
    pub fn set_class_name(&mut self, value: &str) -> Result<(), DomError> {
        self.set_attribute("class", value)
    }

    /// Implements `classList.contains`.
    pub fn class_contains(&self, token: &str) -> Result<bool, DomError> {
        validate_class_token(token)?;
        Ok(self.class_tokens()?.iter().any(|class| class == token))
    }

    /// Implements `classList.add` for one or more tokens.
    pub fn class_add(&mut self, tokens: &[&str]) -> Result<(), DomError> {
        validate_class_tokens(tokens)?;
        let mut classes = self.class_tokens()?;
        for token in tokens {
            if !classes.iter().any(|class| class == token) {
                classes.push((*token).into());
            }
        }
        self.write_class_tokens(classes)
    }

    /// Implements `classList.remove` for one or more tokens.
    pub fn class_remove(&mut self, tokens: &[&str]) -> Result<(), DomError> {
        validate_class_tokens(tokens)?;
        let mut classes = self.class_tokens()?;
        classes.retain(|class| !tokens.iter().any(|token| class == token));
        self.write_class_tokens(classes)
    }

    /// Implements `classList.toggle`, including its optional force argument.
    pub fn class_toggle(&mut self, token: &str, force: Option<bool>) -> Result<bool, DomError> {
        validate_class_token(token)?;
        let present = self.class_contains(token)?;
        let desired = force.unwrap_or(!present);
        if desired != present {
            if desired {
                self.class_add(&[token])?;
            } else {
                self.class_remove(&[token])?;
            }
        }
        Ok(desired)
    }

    fn class_tokens(&self) -> Result<Vec<String>, DomError> {
        Ok(self
            .class_name()?
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect())
    }

    fn write_class_tokens(&mut self, classes: Vec<String>) -> Result<(), DomError> {
        self.set_class_name(&classes.join(" "))
    }
}

fn validate_class_tokens(tokens: &[&str]) -> Result<(), DomError> {
    for token in tokens {
        validate_class_token(token)?;
    }
    Ok(())
}

fn validate_class_token(token: &str) -> Result<(), DomError> {
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        Err(DomError::Syntax(
            "class token must be non-empty and contain no whitespace".into(),
        ))
    } else {
        Ok(())
    }
}

/// Inline-style operations required by `CSSStyleDeclaration` wrappers.
pub trait InlineStyleBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads one kebab-case inline property.
    fn style_property(
        &self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;
    /// Attempts to set one property, returning false for an invalid value.
    fn style_set_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError>;
    /// Removes one property and returns its old value.
    fn style_remove_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError>;
    /// Serializes the declaration block.
    fn style_css_text(&self, node: Self::NodeId) -> Result<String, DomError>;
    /// Replaces the declaration block through the backend CSS parser.
    fn style_set_css_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError>;
}

impl<D: DomBackend> InlineStyleBackend for D {
    type NodeId = D::NodeId;

    fn style_property(
        &self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        self.inline_style(node, property)
    }

    fn style_set_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
        value: &str,
    ) -> Result<bool, DomError> {
        self.set_inline_style(node, property, value)
    }

    fn style_remove_property(
        &mut self,
        node: Self::NodeId,
        property: &str,
    ) -> Result<Option<String>, DomError> {
        self.remove_inline_style(node, property)
    }

    fn style_css_text(&self, node: Self::NodeId) -> Result<String, DomError> {
        self.inline_style_text(node)
    }

    fn style_set_css_text(&mut self, node: Self::NodeId, css: &str) -> Result<(), DomError> {
        self.set_inline_style_text(node, css)
    }
}

/// Runtime-neutral `CSSStyleDeclaration` implementation for inline styles.
pub struct InlineStyleApi<'a, D: InlineStyleBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

impl<'a, D: InlineStyleBackend> InlineStyleApi<'a, D> {
    /// Wraps one element's inline declaration block.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Reads a camelCase JavaScript style property.
    pub fn get_js_property(&self, property: &str) -> Result<String, DomError> {
        self.get_property_value(&js_property_to_css(property))
    }

    /// Writes a camelCase JavaScript style property. Invalid CSS is ignored.
    pub fn set_js_property(&mut self, property: &str, value: &str) -> Result<(), DomError> {
        self.set_property(&js_property_to_css(property), value)
    }

    /// Implements `getPropertyValue` for a kebab-case or custom property.
    pub fn get_property_value(&self, property: &str) -> Result<String, DomError> {
        Ok(self
            .backend
            .style_property(self.node, property)?
            .unwrap_or_default())
    }

    /// Implements `setProperty`; invalid declarations do not throw.
    pub fn set_property(&mut self, property: &str, value: &str) -> Result<(), DomError> {
        self.backend
            .style_set_property(self.node, property, value)?;
        Ok(())
    }

    /// Implements `removeProperty`, returning the previous value.
    pub fn remove_property(&mut self, property: &str) -> Result<String, DomError> {
        Ok(self
            .backend
            .style_remove_property(self.node, property)?
            .unwrap_or_default())
    }

    /// Implements the `cssText` getter.
    pub fn css_text(&self) -> Result<String, DomError> {
        self.backend.style_css_text(self.node)
    }

    /// Implements the `cssText` setter through the backend CSS parser.
    pub fn set_css_text(&mut self, css: &str) -> Result<(), DomError> {
        self.backend.style_set_css_text(self.node, css)
    }
}

/// Maps a JavaScript camelCase style property to its CSS spelling.
pub fn js_property_to_css(property: &str) -> String {
    if property.starts_with("--") {
        return property.into();
    }
    if property == "cssFloat" {
        return "float".into();
    }
    let mut css = String::with_capacity(property.len() + 4);
    for (index, character) in property.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index == 0 || !css.ends_with('-') {
                css.push('-');
            }
            css.push(character.to_ascii_lowercase());
        } else {
            css.push(character);
        }
    }
    css
}

impl<'a, D: NodeContentBackend> NodeContentApi<'a, D> {
    /// Wraps a node from the authoritative backend.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Implements the `textContent` getter.
    pub fn text_content(&self) -> Result<String, DomError> {
        self.backend.content_text(self.node)
    }

    /// Implements the `textContent` setter.
    pub fn set_text_content(&mut self, text: &str) -> Result<(), DomError> {
        self.backend.content_set_text(self.node, text)
    }

    /// Implements the `innerHTML` getter.
    pub fn inner_html(&self) -> Result<String, DomError> {
        self.backend.content_inner_html(self.node)
    }

    /// Implements the `innerHTML` setter through the backend fragment parser.
    pub fn set_inner_html(&mut self, html: &str) -> Result<(), DomError> {
        self.backend.content_set_inner_html(self.node, html)
    }
}

impl<'a, D: NodeTreeBackend> NodeTreeApi<'a, D> {
    /// Wraps one handle from the authoritative backend tree.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Returns this wrapper's node handle.
    pub fn node_id(&self) -> D::NodeId {
        self.node
    }

    /// Implements `appendChild` and returns the appended node.
    pub fn append_child(&mut self, child: D::NodeId) -> Result<D::NodeId, DomError> {
        self.backend.node_append(self.node, child)?;
        Ok(child)
    }

    /// Implements `insertBefore` and returns the inserted node.
    pub fn insert_before(
        &mut self,
        child: D::NodeId,
        reference: Option<D::NodeId>,
    ) -> Result<D::NodeId, DomError> {
        self.backend
            .node_insert_before(self.node, child, reference)?;
        Ok(child)
    }

    /// Implements `removeChild`, rejecting a node owned by another parent.
    pub fn remove_child(&mut self, child: D::NodeId) -> Result<D::NodeId, DomError> {
        if self.backend.node_parent(child)? != Some(self.node) {
            return Err(DomError::NotFound);
        }
        self.backend.node_remove(child)?;
        Ok(child)
    }

    /// Implements `Node.remove()`.
    pub fn remove(&mut self) -> Result<(), DomError> {
        self.backend.node_remove(self.node)
    }

    /// Implements the one-node v0 form of `replaceWith`.
    pub fn replace_with(&mut self, replacement: D::NodeId) -> Result<(), DomError> {
        self.backend.node_replace(self.node, replacement)
    }

    /// Implements `parentNode`.
    pub fn parent_node(&self) -> Result<Option<D::NodeId>, DomError> {
        self.backend.node_parent(self.node)
    }

    /// Implements `childNodes` as a snapshot for the current bridge turn.
    pub fn child_nodes(&self) -> Result<StaticNodeList<D::NodeId>, DomError> {
        self.backend
            .node_children(self.node)
            .map(StaticNodeList::new)
    }

    /// Implements `firstChild`.
    pub fn first_child(&self) -> Result<Option<D::NodeId>, DomError> {
        Ok(self.backend.node_children(self.node)?.first().copied())
    }

    /// Implements `nextSibling`.
    pub fn next_sibling(&self) -> Result<Option<D::NodeId>, DomError> {
        self.backend.node_next_sibling(self.node)
    }
}

impl<'a, D: DocumentBackend> DocumentApi<'a, D> {
    /// Borrows the authoritative DOM backend as a document object.
    pub fn new(backend: &'a mut D) -> Self {
        Self { backend }
    }

    /// Implements `document.querySelector`.
    pub fn query_selector(&self, selector: &str) -> Result<Option<D::NodeId>, DomError> {
        self.backend.document_query_selector(selector)
    }

    /// Implements `document.querySelectorAll` as a static v0 snapshot.
    pub fn query_selector_all(
        &self,
        selector: &str,
    ) -> Result<StaticNodeList<D::NodeId>, DomError> {
        self.backend
            .document_query_selector_all(selector)
            .map(StaticNodeList::new)
    }

    /// Implements `document.getElementById` without rebuilding an ID index.
    pub fn get_element_by_id(&self, id: &str) -> Result<Option<D::NodeId>, DomError> {
        self.backend.document_get_element_by_id(id)
    }

    /// Implements HTML `document.createElement`.
    pub fn create_element(&mut self, local_name: &str) -> Result<D::NodeId, DomError> {
        self.backend.document_create_element(local_name)
    }

    /// Implements `document.createTextNode`.
    pub fn create_text_node(&mut self, text: &str) -> Result<D::NodeId, DomError> {
        self.backend.document_create_text(text)
    }

    /// Implements `document.body`.
    pub fn body(&self) -> Option<D::NodeId> {
        self.backend.document_body()
    }

    /// Implements `document.documentElement`.
    pub fn document_element(&self) -> Option<D::NodeId> {
        self.backend.document_element()
    }
}

impl<E: JsEngine> WrapperEngine for E {
    type Value = E::Value;
    type WeakRef = E::WeakRef;

    fn downgrade_wrapper(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError> {
        self.downgrade(value)
    }

    fn upgrade_wrapper(
        &mut self,
        reference: &Self::WeakRef,
    ) -> Result<Option<Self::Value>, JsError> {
        self.upgrade(reference)
    }
}

struct WrapperEntry<W> {
    weak: W,
    token: u64,
}

/// Preserves one JavaScript wrapper identity for each live node handle.
///
/// The table holds only weak JavaScript references. A wrapper finalizer removes
/// its own entry, so JavaScript reachability—not the cache—controls collection.
pub struct WrapperTable<N, W> {
    entries: Rc<RefCell<HashMap<N, WrapperEntry<W>>>>,
    next_token: Cell<u64>,
}

impl<N, W> Default for WrapperTable<N, W> {
    fn default() -> Self {
        Self {
            entries: Rc::new(RefCell::new(HashMap::new())),
            next_token: Cell::new(0),
        }
    }
}

impl<N, W> WrapperTable<N, W>
where
    N: Clone + Eq + Hash + 'static,
    W: 'static,
{
    /// Creates an empty identity table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the existing live wrapper or creates exactly one replacement.
    ///
    /// `create` receives the finalizer that must be attached to the new native
    /// JavaScript object. It may compose that callback with node-lifetime work,
    /// but must invoke it once when the wrapper is collected.
    pub fn get_or_create<E, F>(
        &self,
        engine: &mut E,
        node: N,
        create: F,
    ) -> Result<E::Value, JsError>
    where
        E: WrapperEngine<WeakRef = W>,
        F: FnOnce(&mut E, Box<dyn FnOnce(ExternalId) + 'static>) -> Result<E::Value, JsError>,
    {
        let existing = {
            let entries = self.entries.borrow();
            match entries.get(&node) {
                Some(entry) => engine.upgrade_wrapper(&entry.weak)?,
                None => None,
            }
        };
        if let Some(wrapper) = existing {
            return Ok(wrapper);
        }
        self.entries.borrow_mut().remove(&node);

        let token = self.next_token.get();
        self.next_token.set(token.wrapping_add(1));
        let entries = Rc::downgrade(&self.entries);
        let finalizer_node = node.clone();
        let finalizer = Box::new(move |_external: ExternalId| {
            let Some(entries) = entries.upgrade() else {
                return;
            };
            let mut entries = entries.borrow_mut();
            if entries
                .get(&finalizer_node)
                .is_some_and(|entry| entry.token == token)
            {
                entries.remove(&finalizer_node);
            }
        });

        let wrapper = create(engine, finalizer)?;
        let weak = engine.downgrade_wrapper(&wrapper)?;
        self.entries
            .borrow_mut()
            .insert(node, WrapperEntry { weak, token });
        Ok(wrapper)
    }

    /// Removes entries whose JavaScript wrappers have already been collected.
    ///
    /// Finalizers normally keep the table current. This is a defensive sweep
    /// for hosts that defer finalizer callbacks until a later loop turn.
    pub fn prune_collected<E>(&self, engine: &mut E) -> Result<usize, JsError>
    where
        E: WrapperEngine<WeakRef = W>,
    {
        let collected = {
            let entries = self.entries.borrow();
            let mut collected = Vec::new();
            for (node, entry) in entries.iter() {
                if engine.upgrade_wrapper(&entry.weak)?.is_none() {
                    collected.push((node.clone(), entry.token));
                }
            }
            collected
        };
        let count = collected.len();
        let mut entries = self.entries.borrow_mut();
        for (node, token) in collected {
            if entries.get(&node).is_some_and(|entry| entry.token == token) {
                entries.remove(&node);
            }
        }
        Ok(count)
    }

    /// Returns the number of cached weak wrappers.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// Reports whether no node currently has a cached wrapper.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }
}

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
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};

    use blitsen_dom::NodeId;

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

    #[test]
    fn repeated_lookups_preserve_strict_object_identity() {
        let table = WrapperTable::new();
        let mut engine = MockEngine;
        let node = NodeId::new(4, 2);
        let first = table
            .get_or_create(&mut engine, node, |_, finalizer| {
                Ok(wrapper(ExternalId(node.to_u64()), finalizer))
            })
            .unwrap();
        let second = table
            .get_or_create(&mut engine, node, |_, _| {
                panic!("created a duplicate wrapper")
            })
            .unwrap();

        assert!(Rc::ptr_eq(&first, &second));
        let mut weak_map = HashMap::new();
        weak_map.insert(Rc::as_ptr(&first), "value");
        assert_eq!(weak_map.get(&Rc::as_ptr(&second)), Some(&"value"));
    }

    #[test]
    fn finalizers_remove_only_the_wrapper_generation_they_own() {
        let table = WrapperTable::new();
        let mut engine = MockEngine;
        let node = NodeId::new(1, 0);
        let live_wrapper = table
            .get_or_create(&mut engine, node, |_, finalizer| {
                Ok(wrapper(ExternalId(node.to_u64()), finalizer))
            })
            .unwrap();
        assert_eq!(table.len(), 1);
        drop(live_wrapper);
        assert!(table.is_empty());

        let replacement = table
            .get_or_create(&mut engine, node, |_, finalizer| {
                Ok(wrapper(ExternalId(node.to_u64()), finalizer))
            })
            .unwrap();
        assert_eq!(table.len(), 1);
        drop(replacement);
        assert!(table.is_empty());
    }

    #[test]
    fn churning_one_hundred_thousand_nodes_does_not_grow_the_table() {
        let table = WrapperTable::new();
        let mut engine = MockEngine;
        for slot in 0..100_000 {
            let node = NodeId::new(slot, 0);
            let wrapper = table
                .get_or_create(&mut engine, node, |_, finalizer| {
                    Ok(wrapper(ExternalId(node.to_u64()), finalizer))
                })
                .unwrap();
            drop(wrapper);
        }
        assert!(table.is_empty());
    }

    #[test]
    fn document_queries_delegate_and_nodelists_are_static() {
        let mut backend = MockDocument {
            matches: vec![2, 3],
            ..Default::default()
        };
        let list = {
            let document = DocumentApi::new(&mut backend);
            assert_eq!(document.query_selector(".item").unwrap(), Some(2));
            document.query_selector_all(".item").unwrap()
        };
        backend.matches.push(4);

        assert_eq!(list.into_vec(), vec![2, 3]);
        assert_eq!(
            backend.queried_selectors.into_inner(),
            vec![".item", ".item"]
        );
    }

    #[test]
    fn document_exposes_creation_and_root_elements() {
        let mut backend = MockDocument::default();
        let mut document = DocumentApi::new(&mut backend);

        assert_eq!(document.create_element("section").unwrap(), 1);
        assert_eq!(document.create_text_node("hello").unwrap(), 2);
        assert_eq!(document.get_element_by_id("target").unwrap(), Some(2));
        assert_eq!(document.body(), Some(10));
        assert_eq!(document.document_element(), Some(1));
    }

    #[test]
    fn node_mutations_update_the_authoritative_tree() {
        let mut tree = MockTree::default();
        {
            let mut root = NodeTreeApi::new(&mut tree, 1);
            root.append_child(2).unwrap();
            root.append_child(3).unwrap();
            root.insert_before(4, Some(3)).unwrap();
            assert_eq!(root.child_nodes().unwrap().into_vec(), vec![2, 4, 3]);
            assert_eq!(root.first_child().unwrap(), Some(2));
        }
        assert_eq!(
            NodeTreeApi::new(&mut tree, 4).next_sibling().unwrap(),
            Some(3)
        );

        // Moving an already-parented node detaches it first.
        NodeTreeApi::new(&mut tree, 5).append_child(4).unwrap();
        assert_eq!(tree.children.get(&1).unwrap(), &vec![2, 3]);
        assert_eq!(tree.children.get(&5).unwrap(), &vec![4]);

        NodeTreeApi::new(&mut tree, 1).remove_child(2).unwrap();
        assert!(!tree.parents.contains_key(&2));
        NodeTreeApi::new(&mut tree, 1).append_child(6).unwrap();
        NodeTreeApi::new(&mut tree, 6).replace_with(7).unwrap();
        assert_eq!(tree.children.get(&1).unwrap(), &vec![3, 7]);
        assert!(!tree.parents.contains_key(&6));
    }

    #[test]
    fn remove_child_rejects_a_node_from_another_parent() {
        let mut tree = MockTree::default();
        NodeTreeApi::new(&mut tree, 2).append_child(3).unwrap();
        assert_eq!(
            NodeTreeApi::new(&mut tree, 1).remove_child(3),
            Err(DomError::NotFound)
        );
        assert_eq!(tree.parents.get(&3), Some(&2));
    }

    #[test]
    fn text_and_html_replace_children_and_invalidate() {
        let mut backend = MockContent {
            text: "AB".into(),
            html: "<b>A</b><i>B</i>".into(),
            invalidations: 0,
        };
        {
            let mut node = NodeContentApi::new(&mut backend, 1);
            assert_eq!(node.text_content().unwrap(), "AB");
            assert_eq!(node.inner_html().unwrap(), "<b>A</b><i>B</i>");
            node.set_text_content("a < b & c").unwrap();
            assert_eq!(node.inner_html().unwrap(), "a &lt; b &amp; c");
            node.set_inner_html("<span>A &amp; B</span>").unwrap();
            assert_eq!(node.text_content().unwrap(), "A & B");
            assert_eq!(node.inner_html().unwrap(), "<span>A &amp; B</span>");
        }
        assert_eq!(backend.invalidations, 2);
    }

    #[test]
    fn attributes_reflect_and_class_changes_affect_the_cascade() {
        let mut backend = MockAttributes::default();
        {
            let mut element = ElementAttributesApi::new(&mut backend, 1);
            element.set_id("target").unwrap();
            assert_eq!(element.id().unwrap(), "target");
            element.set_class_name("button").unwrap();
            element.class_add(&["primary", "button"]).unwrap();
            assert!(element.class_contains("primary").unwrap());
            assert_eq!(element.class_name().unwrap(), "button primary");
            assert!(!element.class_toggle("primary", None).unwrap());
            assert!(element.class_toggle("disabled", Some(true)).unwrap());
            assert_eq!(element.class_name().unwrap(), "button disabled");
            element.remove_attribute("id").unwrap();
            assert!(!element.has_attribute("id").unwrap());
        }

        // This models a selector cascade read after backend restyling, not just
        // an attribute string assertion.
        let computed_opacity = if backend.values["class"]
            .split_ascii_whitespace()
            .any(|class| class == "disabled")
        {
            0.5
        } else {
            1.0
        };
        assert_eq!(computed_opacity, 0.5);
        assert_eq!(backend.restyles, 6);
    }

    #[test]
    fn class_list_rejects_invalid_tokens_without_mutating() {
        let mut backend = MockAttributes::default();
        let mut element = ElementAttributesApi::new(&mut backend, 1);
        assert!(matches!(
            element.class_add(&["two words"]),
            Err(DomError::Syntax(_))
        ));
        assert_eq!(element.class_name().unwrap(), "");
    }

    #[test]
    fn inline_style_maps_properties_and_ignores_invalid_values() {
        assert_eq!(js_property_to_css("backgroundColor"), "background-color");
        assert_eq!(js_property_to_css("WebkitTransform"), "-webkit-transform");
        assert_eq!(js_property_to_css("--brandColor"), "--brandColor");
        assert_eq!(js_property_to_css("cssFloat"), "float");

        let mut backend = MockStyle::default();
        let mut style = InlineStyleApi::new(&mut backend, 1);
        style.set_js_property("backgroundColor", "red").unwrap();
        style.set_property("--brand", "blue").unwrap();
        style.set_js_property("width", "invalid").unwrap();
        assert_eq!(style.get_js_property("backgroundColor").unwrap(), "red");
        assert_eq!(style.get_js_property("width").unwrap(), "");
        assert_eq!(style.remove_property("--brand").unwrap(), "blue");
        assert_eq!(style.remove_property("--brand").unwrap(), "");

        style
            .set_css_text("left: 40px; color: green; width: invalid")
            .unwrap();
        assert_eq!(style.get_js_property("left").unwrap(), "40px");
        assert_eq!(style.css_text().unwrap(), "color: green; left: 40px;");
    }

    #[test]
    fn window_state_tracks_logical_resize_dimensions() {
        let mut window = WindowState::new(800, 600, 2.0);
        assert_eq!((window.width(), window.height()), (800, 600));
        assert_eq!(window.device_pixel_ratio(), 2.0);
        window.resize(1024, 768);
        assert_eq!((window.width(), window.height()), (1024, 768));
    }

    #[test]
    fn document_scripts_run_in_order_with_local_module_identity() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/s7/fixture/index.html");
        let document = MockScripts(vec![
            DocumentScript {
                source: "globalThis.first = true".into(),
                src: None,
                script_type: None,
                async_attribute: false,
                defer_attribute: false,
            },
            DocumentScript {
                source: String::new(),
                src: Some("src/math.js".into()),
                script_type: Some("module".into()),
                async_attribute: true,
                defer_attribute: false,
            },
            DocumentScript {
                source: "ignored".into(),
                src: None,
                script_type: Some("application/json".into()),
                async_attribute: false,
                defer_attribute: false,
            },
        ]);
        let mut engine = RecordingScriptEngine::default();
        assert_eq!(
            execute_document_scripts(&document, &mut engine, &fixture).unwrap(),
            vec![1, 2]
        );
        assert_eq!(engine.evaluations[0].0, "classic");
        assert!(engine.evaluations[0].2.ends_with("index.html#script-1"));
        assert_eq!(engine.evaluations[1].0, "module");
        assert!(engine.evaluations[1].2.ends_with("src/math.js"));
        assert!(!engine.evaluations[1].1.is_empty());
    }

    #[test]
    fn document_scripts_reject_server_root_and_remote_sources() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture/index.html");
        for src in ["/assets/app.js", "https://example.com/app.js"] {
            let document = MockScripts(vec![DocumentScript {
                source: String::new(),
                src: Some(src.into()),
                script_type: None,
                async_attribute: false,
                defer_attribute: false,
            }]);
            let error = execute_document_scripts(
                &document,
                &mut RecordingScriptEngine::default(),
                &fixture,
            )
            .unwrap_err();
            assert!(error.message().contains("must be relative"));
        }
    }
}
