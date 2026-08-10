//! Native DOM object installation for the Bun host.

use std::cell::RefCell;
use std::rc::Rc;

use blitsen_core::{WindowState, WrapperTable, js_property_to_css};
use blitsen_dom::{DomBackend, DomError, DomName, NodeKind};
use blitsen_js::{ExternalId, JsEngine, JsError, NativeClass};
use blitz::dom::NodeId;
use napi::{Env, Unknown, sys};
use serde_json::{Value, json};

use super::{DomRuntime, NodeApiEngine, NodeWeakRef, callback_string, check, unknown};

const BOOTSTRAP: &str = r#"
(() => {
  const call = (operation, ...args) =>
    JSON.parse(__blitsenDomCall(operation, ...args.map(value => String(value))));
  const handle = Symbol("Blitsen node handle");
  let nextAnimationFrameId = 1;
  let animationFrames = new Map();
  let runningAnimationFrames = null;

  const requestAnimationFrame = callback => {
    if (typeof callback !== "function") throw new TypeError("requestAnimationFrame callback must be a function");
    const id = nextAnimationFrameId++;
    animationFrames.set(id, callback);
    return id;
  };
  const cancelAnimationFrame = id => {
    animationFrames.delete(Number(id));
    runningAnimationFrames?.delete(Number(id));
  };
  const animationFrameTick = timestamp => {
    const callbacks = animationFrames;
    animationFrames = new Map();
    runningAnimationFrames = callbacks;
    for (const [id, callback] of callbacks) {
      if (!callbacks.has(id)) continue;
      try { callback(Number(timestamp)); }
      catch (error) { console.error("Uncaught exception in requestAnimationFrame callback", error); }
    }
    runningAnimationFrames = null;
    return animationFrames.size;
  };

  const eventStates = new WeakMap();
  const stateFor = event => {
    const state = eventStates.get(event);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  };

  class Event {
    constructor(type, options = {}) {
      eventStates.set(this, {
        type: String(type), target: null, currentTarget: null, eventPhase: 0,
        bubbles: Boolean(options.bubbles), cancelable: Boolean(options.cancelable),
        defaultPrevented: false, propagationStopped: false,
        immediatePropagationStopped: false, passive: false,
        timeStamp: performance.now(),
      });
    }
    get type() { return stateFor(this).type; }
    get target() { return stateFor(this).target; }
    get currentTarget() { return stateFor(this).currentTarget; }
    get eventPhase() { return stateFor(this).eventPhase; }
    get bubbles() { return stateFor(this).bubbles; }
    get cancelable() { return stateFor(this).cancelable; }
    get defaultPrevented() { return stateFor(this).defaultPrevented; }
    get timeStamp() { return stateFor(this).timeStamp; }
    preventDefault() {
      const state = stateFor(this);
      if (state.cancelable && !state.passive) state.defaultPrevented = true;
    }
    stopPropagation() { stateFor(this).propagationStopped = true; }
    stopImmediatePropagation() {
      const state = stateFor(this);
      state.propagationStopped = true;
      state.immediatePropagationStopped = true;
    }
  }

  class MouseEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      const numbers = ["clientX", "clientY", "offsetX", "offsetY", "screenX", "screenY", "button", "buttons"];
      for (const property of numbers) Object.defineProperty(this, property, {
        value: Number(options[property] ?? 0), enumerable: true,
      });
      for (const property of ["ctrlKey", "shiftKey", "altKey", "metaKey"]) Object.defineProperty(this, property, {
        value: Boolean(options[property]), enumerable: true,
      });
    }
  }

  class KeyboardEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperties(this, {
        key: { value: String(options.key ?? ""), enumerable: true },
        code: { value: String(options.code ?? ""), enumerable: true },
        repeat: { value: Boolean(options.repeat), enumerable: true },
        ctrlKey: { value: Boolean(options.ctrlKey), enumerable: true },
        shiftKey: { value: Boolean(options.shiftKey), enumerable: true },
        altKey: { value: Boolean(options.altKey), enumerable: true },
        metaKey: { value: Boolean(options.metaKey), enumerable: true },
      });
    }
  }

  class CustomEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, "detail", { value: options.detail ?? null, enumerable: true });
    }
  }

  const eventInternals = Object.freeze({
    state: stateFor,
    begin(event, target, currentTarget, eventPhase, passive = false) {
      const state = stateFor(event);
      state.target ??= target;
      state.currentTarget = currentTarget;
      state.eventPhase = eventPhase;
      state.passive = passive;
      return state;
    },
    finish(event) {
      const state = stateFor(event);
      state.currentTarget = null;
      state.eventPhase = 0;
      state.passive = false;
    },
  });

  class Node {
    constructor() { throw new TypeError("Illegal constructor"); }
    appendChild(child) {
      call("appendChild", this[handle], requireNode(child));
      return child;
    }
    insertBefore(child, reference) {
      call("insertBefore", this[handle], requireNode(child), reference == null ? "" : requireNode(reference));
      return child;
    }
    removeChild(child) {
      call("removeChild", this[handle], requireNode(child));
      return child;
    }
    remove() { call("remove", this[handle]); }
    replaceWith(replacement) { call("replaceWith", this[handle], requireNode(replacement)); }
    get parentNode() { return wrap(call("parentNode", this[handle])); }
    get childNodes() { return new NodeList(call("childNodes", this[handle]).map(wrap)); }
    get firstChild() { return wrap(call("firstChild", this[handle])); }
    get nextSibling() { return wrap(call("nextSibling", this[handle])); }
    get isConnected() { return call("isConnected", this[handle]); }
    get textContent() { return call("textContent", this[handle]); }
    set textContent(value) { call("setTextContent", this[handle], String(value)); }
  }

  const styleCache = new WeakMap();
  const classListCache = new WeakMap();

  class Element extends Node {
    getAttribute(name) { return call("getAttribute", this[handle], String(name)); }
    setAttribute(name, value) { call("setAttribute", this[handle], String(name), String(value)); }
    removeAttribute(name) { call("removeAttribute", this[handle], String(name)); }
    hasAttribute(name) { return call("hasAttribute", this[handle], String(name)); }
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get classList() {
      let list = classListCache.get(this);
      if (!list) {
        list = new DOMTokenList(this);
        classListCache.set(this, list);
      }
      return list;
    }
    get style() {
      let style = styleCache.get(this);
      if (!style) {
        const declaration = new CSSStyleDeclaration(this);
        style = new Proxy(declaration, {
          get(target, property, receiver) {
            if (typeof property !== "string" || property in target) return Reflect.get(target, property, receiver);
            return target._getJsProperty(property);
          },
          set(target, property, value, receiver) {
            if (typeof property !== "string" || property in target) return Reflect.set(target, property, value, receiver);
            target._setJsProperty(property, String(value));
            return true;
          }
        });
        styleCache.set(this, style);
      }
      return style;
    }
    get innerHTML() { return call("innerHTML", this[handle]); }
    set innerHTML(value) { call("setInnerHTML", this[handle], String(value)); }
  }

  class NodeList {
    constructor(items) {
      Object.defineProperty(this, "length", { value: items.length, enumerable: false });
      items.forEach((item, index) => Object.defineProperty(this, index, { value: item, enumerable: true }));
      Object.freeze(this);
    }
    item(index) { return this[index] ?? null; }
    *[Symbol.iterator]() { for (let index = 0; index < this.length; index++) yield this[index]; }
  }

  class DOMTokenList {
    constructor(element) { this._element = element; }
    _tokens() { return this._element.className.trim() ? this._element.className.trim().split(/\s+/) : []; }
    _validate(tokens) {
      for (const token of tokens) {
        if (!token || /\s/.test(token)) throw new DOMException("The token must not be empty or contain whitespace", "SyntaxError");
      }
    }
    contains(token) { this._validate([token]); return this._tokens().includes(token); }
    add(...tokens) {
      this._validate(tokens);
      const values = this._tokens();
      for (const token of tokens) if (!values.includes(token)) values.push(token);
      this._element.className = values.join(" ");
    }
    remove(...tokens) {
      this._validate(tokens);
      this._element.className = this._tokens().filter(token => !tokens.includes(token)).join(" ");
    }
    toggle(token, force) {
      this._validate([token]);
      const present = this.contains(token);
      const desired = force === undefined ? !present : Boolean(force);
      if (desired !== present) (desired ? this.add(token) : this.remove(token));
      return desired;
    }
    toString() { return this._element.className; }
  }

  class CSSStyleDeclaration {
    constructor(element) { this._element = element; }
    _name(property) { const name = String(property); return name.startsWith("--") ? name : name.toLowerCase(); }
    getPropertyValue(property) { return call("styleGet", this._element[handle], this._name(property)); }
    setProperty(property, value) { call("styleSet", this._element[handle], this._name(property), String(value)); }
    removeProperty(property) { return call("styleRemove", this._element[handle], this._name(property)); }
    get cssText() { return call("styleText", this._element[handle]); }
    set cssText(value) { call("setStyleText", this._element[handle], String(value)); }
    _getJsProperty(property) { return call("styleGetJs", this._element[handle], property); }
    _setJsProperty(property, value) { call("styleSetJs", this._element[handle], property, value); }
  }

  const requireNode = value => {
    if (!(value instanceof Node) || !(handle in value)) throw new TypeError("argument is not a Node");
    return value[handle];
  };
  const wrap = rawHandle => {
    if (rawHandle == null) return null;
    const wrapper = __blitsenWrap(String(rawHandle));
    if (!(handle in wrapper)) {
      Object.defineProperty(wrapper, handle, { value: String(rawHandle) });
      Object.setPrototypeOf(wrapper, call("kind", rawHandle) === "element" ? Element.prototype : Node.prototype);
    }
    return wrapper;
  };

  class Document {
    querySelector(selector) { return wrap(call("querySelector", String(selector))); }
    querySelectorAll(selector) { return new NodeList(call("querySelectorAll", String(selector)).map(wrap)); }
    getElementById(id) { return wrap(call("getElementById", String(id))); }
    createElement(name) { return wrap(call("createElement", String(name))); }
    createTextNode(text) { return wrap(call("createTextNode", String(text))); }
    get body() { return wrap(call("body")); }
    get documentElement() { return wrap(call("documentElement")); }
  }

  const document = new Document();
  Object.assign(globalThis, {
    Node, Element, NodeList, Document, DOMTokenList, CSSStyleDeclaration, document,
    Event, MouseEvent, KeyboardEvent, CustomEvent,
    requestAnimationFrame, cancelAnimationFrame,
    __blitsenAnimationFrameTick: animationFrameTick,
    __blitsenAnimationFramesPending: () => animationFrames.size > 0,
    __blitsenEventInternals: eventInternals,
  });
  globalThis.window = globalThis;
  for (const key of ["location", "history", "navigator", "localStorage"]) {
    try { delete globalThis[key]; } catch {}
  }
})();
"#;

/// Installs the real DOM object graph into a Node-API JavaScript environment.
pub(super) fn install(
    engine: &mut NodeApiEngine,
    runtime: DomRuntime,
    width: u32,
    height: u32,
    device_pixel_ratio: f64,
) -> Result<Rc<RefCell<WindowState>>, JsError> {
    let class = Rc::new(engine.register_class(NativeClass::new("BlitsenNode"))?);
    let table = Rc::new(WrapperTable::<NodeId, NodeWeakRef>::new());
    let raw_env = engine.raw_env();

    let wrapper_runtime = runtime.clone();
    let wrapper_table = Rc::clone(&table);
    let wrapper_class = Rc::clone(&class);
    let wrap_function = engine.define_function(
        "__blitsenWrap",
        Box::new(move |call| {
            let handle = argument(&call.arguments, 0, "node handle")?;
            let node = wrapper_runtime.resolve_handle(&handle)?;
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            wrapper_table.get_or_create(&mut callback_engine, node, |engine, table_finalizer| {
                wrapper_runtime.retain_handle(&handle)?;
                let finalizer_runtime = wrapper_runtime.clone();
                let finalizer_handle = handle.clone();
                let finalizer = Box::new(move |external| {
                    table_finalizer(external);
                    let _ = finalizer_runtime.release_handle(&finalizer_handle);
                });
                match engine.instantiate(&wrapper_class, ExternalId(node.as_u64()), Some(finalizer))
                {
                    Ok(wrapper) => Ok(wrapper),
                    Err(error) => {
                        let _ = wrapper_runtime.release_handle(&handle);
                        Err(error)
                    }
                }
            })
        }),
    )?;
    engine.set_global("__blitsenWrap", &wrap_function)?;

    let dispatch_runtime = runtime;
    let call_function = engine.define_function(
        "__blitsenDomCall",
        Box::new(move |call| {
            let operation = argument(&call.arguments, 0, "operation")?;
            let arguments = call
                .arguments
                .iter()
                .skip(1)
                .map(callback_string)
                .collect::<Result<Vec<_>, _>>()?;
            let result = dispatch(&dispatch_runtime, &operation, &arguments)?;
            json_string(raw_env, &result)
        }),
    )?;
    engine.set_global("__blitsenDomCall", &call_function)?;
    engine.evaluate_script(BOOTSTRAP, "blitsen:dom-bootstrap")?;

    let document = engine.evaluate_script("globalThis.document", "blitsen:document-value")?;
    let window_state = Rc::new(RefCell::new(WindowState::new(
        width,
        height,
        device_pixel_ratio,
    )));
    window_state.borrow().install(engine, &document)?;
    let resize_state = Rc::clone(&window_state);
    let resize_function = engine.define_function(
        "__blitsenWindowResize",
        Box::new(move |call| {
            let width = argument(&call.arguments, 0, "viewport width")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport width"))?;
            let height = argument(&call.arguments, 1, "viewport height")?
                .parse::<u32>()
                .map_err(|_| JsError::new("invalid viewport height"))?;
            resize_state.borrow_mut().resize(width, height);
            let mut callback_engine = NodeApiEngine::new(Env::from_raw(raw_env));
            let window =
                callback_engine.evaluate_script("globalThis", "blitsen:window-resize-target")?;
            resize_state.borrow().sync(&mut callback_engine, &window)?;
            Ok(call.this)
        }),
    )?;
    engine.set_global("__blitsenWindowResize", &resize_function)?;
    Ok(window_state)
}

fn argument(arguments: &[Unknown<'static>], index: usize, name: &str) -> Result<String, JsError> {
    arguments
        .get(index)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
        .and_then(callback_string)
}

fn bridge_arg<'a>(arguments: &'a [String], index: usize, name: &str) -> Result<&'a str, JsError> {
    arguments
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| JsError::new(format!("missing {name}")))
}

fn handle(_runtime: &DomRuntime, arguments: &[String], index: usize) -> Result<NodeId, JsError> {
    bridge_arg(arguments, index, "node handle")?
        .parse::<u64>()
        .map(NodeId::from_u64)
        .map_err(|_| JsError::new("invalid DOM node handle"))
}

fn serialized(node: Option<NodeId>) -> Value {
    node.map(DomRuntime::serialize_handle)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn dom_error(error: DomError) -> JsError {
    JsError::new(error.to_string())
}

fn dispatch(runtime: &DomRuntime, operation: &str, arguments: &[String]) -> Result<Value, JsError> {
    let shared = runtime.document();
    let mut dom = shared.borrow_mut();
    match operation {
        "kind" => Ok(Value::String(
            match dom
                .node_kind(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
            {
                NodeKind::Element => "element",
                NodeKind::Document => "document",
                NodeKind::Text => "text",
                NodeKind::Comment => "comment",
                NodeKind::Fragment => "fragment",
            }
            .into(),
        )),
        "querySelector" => Ok(serialized(
            dom.query_selector(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?,
        )),
        "querySelectorAll" => Ok(json!(
            dom.query_selector_all(dom.document(), bridge_arg(arguments, 0, "selector")?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "getElementById" => Ok(serialized(
            dom.get_element_by_id(bridge_arg(arguments, 0, "id")?)
                .map_err(dom_error)?,
        )),
        "createElement" => {
            let name = bridge_arg(arguments, 0, "element name")?;
            if name.is_empty()
                || name.chars().any(|character| {
                    character.is_whitespace() || matches!(character, '<' | '>' | '/' | '\0')
                })
            {
                return Err(JsError::new("invalid HTML element name"));
            }
            Ok(serialized(Some(
                dom.create_element(&DomName::html(name.to_ascii_lowercase()))
                    .map_err(dom_error)?,
            )))
        }
        "createTextNode" => Ok(serialized(Some(
            dom.create_text(bridge_arg(arguments, 0, "text")?)
                .map_err(dom_error)?,
        ))),
        "body" => Ok(serialized(dom.body())),
        "documentElement" => Ok(serialized(dom.document_element())),
        "appendChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            dom.append_child(parent, child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "insertBefore" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            let reference = if bridge_arg(arguments, 2, "reference")?.is_empty() {
                None
            } else {
                Some(handle(runtime, arguments, 2)?)
            };
            dom.insert_before(parent, child, reference)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeChild" => {
            let parent = handle(runtime, arguments, 0)?;
            let child = handle(runtime, arguments, 1)?;
            if dom.parent(child).map_err(dom_error)? != Some(parent) {
                return Err(dom_error(DomError::NotFound));
            }
            dom.remove(child).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "remove" => {
            let node = handle(runtime, arguments, 0)?;
            if dom.parent(node).map_err(dom_error)?.is_some() {
                dom.remove(node).map_err(dom_error)?;
            }
            Ok(Value::Null)
        }
        "replaceWith" => {
            let node = handle(runtime, arguments, 0)?;
            let replacement = handle(runtime, arguments, 1)?;
            dom.replace(node, replacement).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "parentNode" => Ok(serialized(
            dom.parent(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "childNodes" => Ok(json!(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .into_iter()
                .map(DomRuntime::serialize_handle)
                .collect::<Vec<_>>()
        )),
        "firstChild" => Ok(serialized(
            dom.children(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?
                .first()
                .copied(),
        )),
        "nextSibling" => Ok(serialized(
            dom.next_sibling(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "isConnected" => Ok(Value::Bool(
            dom.is_connected(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "textContent" => Ok(Value::String(
            dom.text_content(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setTextContent" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_text_content(node, bridge_arg(arguments, 1, "text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "innerHTML" => Ok(Value::String(
            dom.inner_html(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setInnerHTML" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inner_html(node, bridge_arg(arguments, 1, "HTML")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "getAttribute" => Ok(dom
            .attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "setAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.set_attribute(node, &name, bridge_arg(arguments, 2, "attribute value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.remove_attribute(node, &name).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "hasAttribute" => Ok(Value::Bool(
            dom.attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .is_some(),
        )),
        "styleGet" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSet" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                bridge_arg(arguments, 1, "property")?,
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleRemove" => Ok(Value::String(
            dom.remove_inline_style(
                handle(runtime, arguments, 0)?,
                bridge_arg(arguments, 1, "property")?,
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleText" => Ok(Value::String(
            dom.inline_style_text(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setStyleText" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style_text(node, bridge_arg(arguments, 1, "CSS text")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "styleGetJs" => Ok(Value::String(
            dom.inline_style(
                handle(runtime, arguments, 0)?,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
            )
            .map_err(dom_error)?
            .unwrap_or_default(),
        )),
        "styleSetJs" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_inline_style(
                node,
                &js_property_to_css(bridge_arg(arguments, 1, "property")?),
                bridge_arg(arguments, 2, "value")?,
            )
            .map_err(dom_error)?;
            Ok(Value::Null)
        }
        _ => Err(JsError::new(format!(
            "unknown DOM bridge operation: {operation}"
        ))),
    }
}

fn json_string(env: sys::napi_env, value: &Value) -> Result<Unknown<'static>, JsError> {
    let value = serde_json::to_string(value).map_err(|error| JsError::new(error.to_string()))?;
    let length = isize::try_from(value.len())
        .map_err(|_| JsError::new("DOM bridge result exceeds Node-API string limits"))?;
    let mut result = std::ptr::null_mut();
    check(
        unsafe { sys::napi_create_string_utf8(env, value.as_ptr().cast(), length, &mut result) },
        "serialize DOM bridge result",
    )?;
    Ok(unknown(env, result))
}
