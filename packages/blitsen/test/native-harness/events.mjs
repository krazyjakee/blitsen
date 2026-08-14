import { strict as assert } from "node:assert";
import { join } from "node:path";

import { native } from "./addon.mjs";

const eventSurface = JSON.parse(native.runBridgeHarness(
  `<div id="event-surface"></div>`,
  `{ const target = document.getElementById("event-surface");
     const event = new Event("submit", { bubbles: true, cancelable: true });
     if (event.type !== "submit" || event.target !== null || event.currentTarget !== null ||
         event.eventPhase !== 0 || !event.bubbles || !event.cancelable || event.defaultPrevented ||
         typeof event.timeStamp !== "number") throw new Error("Event property surface");
     event.preventDefault();
     if (!event.defaultPrevented) throw new Error("preventDefault");
     const mouse = new MouseEvent("click", { clientX: 10, clientY: 11, offsetX: 2, offsetY: 3,
       screenX: 50, screenY: 60, button: 1, buttons: 2, ctrlKey: true, shiftKey: true, view: window });
     if (mouse.clientX !== 10 || mouse.clientY !== 11 || mouse.offsetX !== 2 || mouse.offsetY !== 3 ||
         mouse.screenX !== 50 || mouse.screenY !== 60 || mouse.button !== 1 || mouse.buttons !== 2 ||
         !mouse.ctrlKey || !mouse.shiftKey || mouse.altKey || mouse.metaKey || mouse.view !== window)
       throw new Error("MouseEvent property surface");
     const keyboard = new KeyboardEvent("keydown", { key: "A", code: "KeyA", repeat: true,
       altKey: true, metaKey: true });
     if (keyboard.key !== "A" || keyboard.code !== "KeyA" || keyboard.keyCode !== 65 ||
         keyboard.which !== 65 || keyboard.charCode !== 0 || !keyboard.repeat ||
         keyboard.ctrlKey || keyboard.shiftKey || !keyboard.altKey || !keyboard.metaKey)
       throw new Error("KeyboardEvent property surface");
     const backspace = new KeyboardEvent("keydown", { key: "Backspace", code: "Backspace" });
     if (backspace.keyCode !== 8 || backspace.which !== 8)
       throw new Error("KeyboardEvent legacy command identity");
     const detail = { answer: 42 };
     if (new CustomEvent("answer", { detail }).detail !== detail) throw new Error("CustomEvent detail");
     for (const unsupported of ["pageX", "pageY", "relatedTarget", "isTrusted"])
       if (unsupported in mouse || unsupported in keyboard || unsupported in event)
         throw new Error("unsupported event property must be absent: " + unsupported);
     target.setAttribute("data-events", "ok"); }`,
  100,
  60,
));
assert.equal(eventSurface.nodes.find(node => node.attributes.id === "event-surface").attributes["data-events"], "ok");

const eventDispatch = JSON.parse(native.runBridgeHarness(
  `<div id="outer"><button id="event-target">go</button></div>`,
  `{ const outer = document.getElementById("outer");
     const target = document.getElementById("event-target");
     if (!(target instanceof EventTarget) || !(document instanceof EventTarget) ||
         typeof window.addEventListener !== "function") throw new Error("EventTarget installation");
     const order = [];
     const listen = (current, name, capture = false) => current.addEventListener("probe", function(event) {
       if (this !== current || event.currentTarget !== current || event.target !== target)
         throw new Error("listener target identity");
       order.push(name + ":" + event.eventPhase);
       if (name === "outer-bubble") event.preventDefault();
     }, capture);
     listen(window, "window-capture", true);
     listen(document, "document-capture", true);
     listen(outer, "outer-capture", true);
     listen(target, "target-capture", true);
     listen(target, "target-bubble");
     listen(outer, "outer-bubble");
     listen(document, "document-bubble");
     listen(window, "window-bubble");
     let event;
     target.addEventListener("probe", current => { event = current; }, { once: true });
     if (__blitsenInjectMouseEvent("probe", target, { bubbles: true, cancelable: true }) !== false ||
         !event.defaultPrevented || event.currentTarget !== null || event.eventPhase !== 0 ||
         event.view !== window)
       throw new Error("injected event result or final state");
     const expected = ["window-capture:1", "document-capture:1", "outer-capture:1",
       "target-capture:2", "target-bubble:2", "outer-bubble:3", "document-bubble:3", "window-bubble:3"];
     if (order.join(",") !== expected.join(",")) throw new Error("propagation order: " + order);

     let once = 0;
     const onceCallback = () => once++;
     target.addEventListener("once", onceCallback, { once: true });
     target.addEventListener("once", onceCallback, { once: false, passive: true });
     __blitsenInjectMouseEvent("once", target); __blitsenInjectMouseEvent("once", target);
     if (once !== 1) throw new Error("once or de-duplication");

     const mutation = [];
     const added = () => mutation.push("added");
     const removed = () => mutation.push("removed");
     target.addEventListener("mutation", () => {
       mutation.push("first");
       target.removeEventListener("mutation", removed);
       target.addEventListener("mutation", added);
     });
     target.addEventListener("mutation", removed);
     __blitsenInjectMouseEvent("mutation", target);
     if (mutation.join(",") !== "first") throw new Error("listener removal/addition during dispatch");
     __blitsenInjectMouseEvent("mutation", target);
     if (mutation.join(",") !== "first,first,added") throw new Error("deferred listener addition");

     const oldError = console.error; let reported = 0; console.error = () => reported++;
     let afterThrow = false;
     target.addEventListener("throw", () => { throw new Error("listener failure"); });
     target.addEventListener("throw", () => { afterThrow = true; });
     __blitsenInjectMouseEvent("throw", target); console.error = oldError;
     if (reported !== 1 || !afterThrow) throw new Error("per-listener exception isolation");

     const stopped = [];
     target.addEventListener("stop", event => { stopped.push("first"); event.stopImmediatePropagation(); });
     target.addEventListener("stop", () => stopped.push("peer"));
     outer.addEventListener("stop", () => stopped.push("ancestor"));
     __blitsenInjectMouseEvent("stop", target, { bubbles: true });
     if (stopped.join(",") !== "first") throw new Error("stopImmediatePropagation");

     const propagation = [];
     target.addEventListener("stop-propagation", event => { propagation.push("first"); event.stopPropagation(); });
     target.addEventListener("stop-propagation", () => propagation.push("peer"));
     outer.addEventListener("stop-propagation", () => propagation.push("ancestor"));
     __blitsenInjectMouseEvent("stop-propagation", target, { bubbles: true });
     if (propagation.join(",") !== "first,peer") throw new Error("stopPropagation");

     let passive;
     target.addEventListener("passive", event => event.preventDefault(), { passive: true });
     target.addEventListener("passive", event => { passive = event; });
     if (!__blitsenInjectMouseEvent("passive", target, { cancelable: true }) || passive.defaultPrevented)
       throw new Error("passive listener");
     let handled = false;
     target.addEventListener("object", { handleEvent(event) { handled = event.currentTarget === target; } });
     __blitsenInjectMouseEvent("object", target);
     if (!handled) throw new Error("object event listener");
     target.setAttribute("data-dispatch", "ok"); }`,
  200,
  100,
));
assert.equal(eventDispatch.nodes.find(node => node.attributes.id === "event-target").attributes["data-dispatch"], "ok");

const keyboardDispatch = JSON.parse(native.runBridgeHarness(
  `<button id="first-key">first</button><button id="second-key">second</button>`,
  `{ const first = document.getElementById("first-key");
     const second = document.getElementById("second-key");
     if (document.activeElement !== document.body) throw new Error("initial activeElement");
     const focusOrder = [];
     first.addEventListener("focus", () => focusOrder.push("first-focus"));
     first.addEventListener("blur", () => focusOrder.push("first-blur"));
     second.addEventListener("focus", () => focusOrder.push("second-focus"));
     first.focus();
     if (document.activeElement !== first || focusOrder.join(",") !== "first-focus") throw new Error("focus()");
     let keyRecord;
     first.addEventListener("keydown", event => {
       keyRecord = [event.key, event.code, event.keyCode, event.which, event.repeat, event.ctrlKey,
         event.currentTarget === first];
     });
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "a", code: "KeyA", repeat: true, ctrlKey: true });
     if (keyRecord.join(",") !== "a,KeyA,65,65,true,true,true")
       throw new Error("native KeyboardEvent dispatch");
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false });
     if (document.activeElement !== second || focusOrder.join(",") !== "first-focus,first-blur,second-focus")
       throw new Error("Tab focus traversal");
     const cancelTab = event => event.preventDefault();
     second.addEventListener("keydown", cancelTab);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false, shiftKey: true });
     if (document.activeElement !== second) throw new Error("preventDefault Tab");
     second.removeEventListener("keydown", cancelTab);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "Tab", code: "Tab", repeat: false, shiftKey: true });
     if (document.activeElement !== first) throw new Error("Shift+Tab focus traversal");
     first.blur();
     if (document.activeElement !== document.body) throw new Error("blur()");
     first.setAttribute("data-keyboard", "ok"); }`,
  200,
  80,
));
assert.equal(keyboardDispatch.nodes.find(node => node.attributes.id === "first-key").attributes["data-keyboard"], "ok");

const defaultActions = JSON.parse(native.runBridgeHarness(
  `<style>
     #scroller { width:120px; height:60px; overflow:auto }
     #content { height:300px }
   </style>
   <div id="scroller"><div id="content"><button id="focusable"><span id="click-target">target</span></button></div></div>
   <button id="cancelled"><span id="cancel-target">cancel</span></button>
   <input id="keeper"><div id="inert">nothing focusable here</div>`,
  `{ const clickTarget = document.getElementById("click-target");
     const focusable = document.getElementById("focusable");
     // Focus is mousedown's default action, not click's, and it runs after the
     // event has finished propagating.
     let focusDuringBubble;
     window.addEventListener("mousedown", () => { focusDuringBubble = document.activeElement; });
     __blitsenInjectMouseEvent("mousedown", clickTarget, { bubbles: true, cancelable: true });
     if (focusDuringBubble !== document.body || document.activeElement !== focusable)
       throw new Error("mousedown focus must follow bubbling and choose the nearest focusable ancestor");
     __blitsenInjectMouseEvent("click", clickTarget, { bubbles: true, cancelable: true });
     if (document.activeElement !== focusable)
       throw new Error("the click that follows must not take focus again");

     // Cancelling the mousedown keeps focus where the application put it. This
     // is the whole of how a component that focuses something of its own — a
     // code editor moving the caret into a hidden textarea — holds onto it, and
     // a click cannot stand in for it: by then the focus has already moved.
     const cancelTarget = document.getElementById("cancel-target");
     const keeper = document.getElementById("keeper");
     const claim = event => { keeper.focus(); event.preventDefault(); };
     cancelTarget.addEventListener("mousedown", claim);
     __blitsenInjectMouseEvent("mousedown", cancelTarget, { bubbles: true, cancelable: true });
     __blitsenInjectMouseEvent("click", cancelTarget, { bubbles: true, cancelable: true });
     if (document.activeElement !== keeper)
       throw new Error("a cancelled mousedown leaves focus where a listener put it: "
         + document.activeElement.id);
     cancelTarget.removeEventListener("mousedown", claim);

     // And an uncancelled press on nothing focusable still drops focus, the way
     // clicking off a field in a browser does — the divergence that would keep
     // a dropdown listening for focusout open for ever.
     __blitsenInjectMouseEvent("mousedown", document.getElementById("inert"),
       { bubbles: true, cancelable: true });
     if (document.activeElement !== document.body)
       throw new Error("pressing on nothing focusable blurs what was focused: "
         + document.activeElement.id);

     __blitsenInjectMouseEvent("wheel", clickTarget,
       { bubbles: true, cancelable: true, deltaY: 40 });
     const cancelWheel = event => event.preventDefault();
     clickTarget.addEventListener("wheel", cancelWheel);
     __blitsenInjectMouseEvent("wheel", clickTarget,
       { bubbles: true, cancelable: true, deltaY: 40 });
     clickTarget.removeEventListener("wheel", cancelWheel);

     focusable.focus();
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "ArrowDown", code: "ArrowDown" });
     const cancelKey = event => event.preventDefault();
     focusable.addEventListener("keydown", cancelKey);
     __blitsenDispatchKeyboardEvent("keydown", { bubbles: true, cancelable: true,
       key: "ArrowDown", code: "ArrowDown" });
     focusable.removeEventListener("keydown", cancelKey);
     document.getElementById("scroller").setAttribute("data-defaults", "ok"); }`,
  200,
  120,
));
const defaultScroller = defaultActions.nodes.find(node => node.attributes.id === "scroller");
assert.equal(defaultScroller.attributes["data-defaults"], "ok");
assert.equal(defaultScroller.scroll_y, 80,
  "wheel and keyboard scroll the nearest ancestor while prevented defaults do nothing");
