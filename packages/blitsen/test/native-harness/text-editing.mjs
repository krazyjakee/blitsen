import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

// Typing, and the caret. Two halves that have to agree: the selection API
// answers about the same editor the keys mutate and the renderer paints, so
// every assertion below reads the state back through `value`, `selectionStart`
// and `selectionEnd` rather than through anything this script kept for itself.
const textEditing = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     #field, #notes, #ro { font: 16px monospace; width: 300px; border: 0; padding: 0 }
     #notes { height: 80px }
     #field:focus { background-color: rgb(0, 128, 0) }
   </style>
   <input id="field" value="hello">
   <textarea id="notes">one
two</textarea>
   <input id="date" type="date" value="2026-01-01">
   <input id="ro" value="fixed" readonly>`,
  `{ const expect = (actual, wanted, what) => {
       if (JSON.stringify(actual) !== JSON.stringify(wanted))
         throw new Error(what + ": " + JSON.stringify(actual) + " is not " + JSON.stringify(wanted));
     };
     const byId = id => document.getElementById(id);
     const field = byId("field");
     const notes = byId("notes");
     const selection = element =>
       [element.selectionStart, element.selectionEnd, element.selectionDirection];
     const key = (key, init = {}) => __blitsenDispatchKeyboardEvent("keydown",
       { bubbles: true, cancelable: true, key, code: key, repeat: false, ...init });

     expect(selection(field), [0, 0, "none"], "a fresh control has a collapsed caret at the start");

     // HTML's value setter puts the caret at the end of what it wrote.
     field.value = "abcdef";
     expect(selection(field), [6, 6, "none"], "assigning value moves the caret to the end");

     field.setSelectionRange(1, 3);
     expect(selection(field), [1, 3, "none"], "a range set from script has no direction");
     field.setSelectionRange(1, 3, "backward");
     expect(selection(field), [1, 3, "backward"], "setSelectionRange carries a direction");
     field.setSelectionRange(4, 2);
     expect(selection(field), [2, 2, "none"], "an inside-out range collapses to its end");
     field.setSelectionRange(0, 99);
     expect(selection(field), [0, 6, "none"], "a range past the value clamps to it");
     field.selectionStart = 5;
     expect(selection(field), [5, 6, "none"], "the start setter pushes the end ahead of it");
     field.selectionEnd = 2;
     expect(selection(field), [2, 2, "none"], "the end setter pulls the start back with it");
     field.selectionDirection = "forward";
     field.setSelectionRange(1, 4, "forward");
     expect(field.selectionDirection, "forward", "the direction survives a round trip");
     field.select();
     expect(selection(field), [0, 6, "none"], "select() takes the whole value");

     // The types with no caret in them answer null and refuse to be given one.
     const date = byId("date");
     expect([date.selectionStart, date.selectionEnd, date.selectionDirection], [null, null, null],
       "a control with no text selection reports null rather than zero");
     let refused = null;
     try { date.setSelectionRange(0, 1); } catch (error) { refused = error.name; }
     expect(refused, "InvalidStateError", "setting a selection on such a control throws");

     // Typing. The keydown's default action is the edit, and the pair of input
     // events brackets it.
     const seen = [];
     const record = event => seen.push([event.type, event.inputType, event.data,
       event.cancelable, event.bubbles]);
     for (const type of ["beforeinput", "input"]) field.addEventListener(type, record);

     field.focus();
     expect(document.activeElement, field, "focus()");
     expect(getComputedStyle(field).backgroundColor, "rgb(0, 128, 0)",
       "the renderer is told what has focus, so :focus matches");

     field.value = "hi";
     field.setSelectionRange(2, 2);
     if (!key("!")) throw new Error("an uncancelled keydown reports true");
     expect([field.value, ...selection(field)], ["hi!", 3, 3, "none"], "a printable key types");
     expect(seen, [["beforeinput", "insertText", "!", true, true],
       ["input", "insertText", "!", false, true]], "beforeinput then input, around the mutation");

     seen.length = 0;
     key("Backspace");
     expect([field.value, field.selectionStart], ["hi", 2], "Backspace deletes before the caret");
     expect(seen, [["beforeinput", "deleteContentBackward", null, true, true],
       ["input", "deleteContentBackward", null, false, true]], "a deletion carries no data");

     // Nothing to delete is nothing to report: beforeinput still announces the
     // attempt, input does not claim a change that did not happen.
     seen.length = 0;
     field.setSelectionRange(0, 0);
     key("Backspace");
     expect([field.value, seen.length], ["hi", 1], "a deletion that changed nothing fires no input");

     // Cancelling either event leaves the value alone.
     seen.length = 0;
     const cancel = event => event.preventDefault();
     field.addEventListener("beforeinput", cancel);
     key("X");
     expect([field.value, seen.length], ["hi", 1], "a cancelled beforeinput makes no edit");
     field.removeEventListener("beforeinput", cancel);
     field.addEventListener("keydown", cancel);
     key("X");
     expect([field.value, seen.length], ["hi", 1], "a cancelled keydown never reaches beforeinput");
     field.removeEventListener("keydown", cancel);
     for (const type of ["beforeinput", "input"]) field.removeEventListener(type, record);

     // Caret motion. Not an edit: it announces nothing and cannot be cancelled
     // by a beforeinput, and shift is what makes it a selection.
     field.value = "abcdef";
     field.setSelectionRange(3, 3);
     key("ArrowLeft");
     expect(selection(field), [2, 2, "none"], "ArrowLeft moves the caret");
     key("ArrowRight", { shiftKey: true });
     expect(selection(field), [2, 3, "forward"], "Shift+ArrowRight selects forwards");
     field.setSelectionRange(3, 3);
     key("ArrowLeft", { shiftKey: true });
     expect(selection(field), [2, 3, "backward"], "Shift+ArrowLeft selects backwards");
     key("Home");
     expect(selection(field), [0, 0, "none"], "Home collapses to the line start");
     key("End");
     expect(selection(field), [6, 6, "none"], "End collapses to the line end");
     key("a", { ctrlKey: true });
     expect(selection(field), [0, 6, "forward"], "Ctrl+A selects the value");
     // A selection is what the next character replaces.
     key("z");
     expect([field.value, field.selectionStart], ["z", 1], "typing replaces the selection");

     // Enter is the textarea's key and not the single-line field's.
     field.value = "one";
     field.setSelectionRange(3, 3);
     key("Enter");
     expect(field.value, "one", "Enter does not break a line in a single-line field");
     notes.focus();
     notes.value = "one";
     notes.setSelectionRange(3, 3);
     key("Enter");
     expect([notes.value, notes.selectionStart], ["one\\n", 4], "Enter breaks a line in a textarea");
     key("ArrowUp");
     expect(notes.selectionStart, 0, "ArrowUp reaches the line above");
     key("ArrowDown");
     expect(notes.selectionStart, 4, "ArrowDown reaches the line below");

     // A character outside the basic plane is one character to delete and two
     // code units to count, and the two answers have to be the same one.
     field.focus();
     field.value = "a\\u{1F600}b";
     expect(field.value.length, 4, "an astral character is two code units of value");
     field.setSelectionRange(3, 3);
     key("Backspace");
     expect([field.value, field.selectionStart], ["ab", 1],
       "Backspace deletes a whole character rather than half a surrogate pair");
     field.value = "\\u{1F600}abc";
     field.setSelectionRange(2, 3);
     expect(field.value.slice(field.selectionStart, field.selectionEnd), "a",
       "offsets are the ones value.slice indexes by");

     // A readonly field takes a caret and no characters.
     const readOnly = byId("ro");
     readOnly.focus();
     readOnly.setSelectionRange(5, 5);
     key("Backspace");
     key("q");
     expect([readOnly.value, readOnly.selectionStart], ["fixed", 5], "readonly refuses every edit");
     key("Home");
     expect(readOnly.selectionStart, 0, "readonly still moves its caret");

     // The keys a field takes are not also the document's: a space typed into
     // one must not page what is behind it.
     field.focus();
     field.value = "";
     field.setSelectionRange(0, 0);
     key(" ");
     expect([field.value, document.scrollingElement.scrollTop], [" ", 0],
       "a space types rather than scrolling");

     // A controlled component writes the value back on every keystroke. Writing
     // the same string must leave the caret where the user put it — assigning
     // moves it to the end, and doing that here would send the caret to the end
     // of the field after every character typed in the middle of it.
     field.value = "abcd";
     field.setSelectionRange(1, 1);
     const echo = event => { event.target.value = event.target.value; };
     field.addEventListener("input", echo);
     key("X");
     expect([field.value, field.selectionStart], ["aXbcd", 2],
       "a controlled component writing the value back does not move the caret");
     field.removeEventListener("input", echo);

     // Typing raises HTML's dirty value flag, exactly as an assignment does, and
     // the two ways a default reaches a control afterwards must both be refused:
     // a later write of the \`value\` attribute, and the pass that gives a
     // textarea its child text every time layout settles. Reading a box forces
     // that pass, which is what makes this assertable in one script.
     const fresh = document.createElement("input");
     fresh.setAttribute("value", "default");
     document.body.appendChild(fresh);
     fresh.getBoundingClientRect();
     fresh.focus();
     key("End");
     key("!");
     fresh.setAttribute("value", "another default");
     fresh.getBoundingClientRect();
     expect([fresh.value, fresh.defaultValue], ["default!", "another default"],
       "a typed value is the control's, and the attribute is only its default");

     const typed = document.createElement("textarea");
     typed.textContent = "child text";
     document.body.appendChild(typed);
     typed.getBoundingClientRect();
     typed.focus();
     key("End");
     key("?");
     typed.getBoundingClientRect();
     expect([typed.value, typed.defaultValue], ["child text?", "child text"],
       "and a textarea's child text stops reaching it once it has been typed into");

     field.setAttribute("data-typing", "ok"); }`,
  400,
  200,
));
assert.equal(
  textEditing.nodes.find(node => node.attributes.id === "field").attributes["data-typing"], "ok");

// Clicking into a field puts the caret where the click landed. Asserted through
// the injector that hit-tests the laid-out tree, so this is the path a real
// press takes rather than a target picked by name.
const caretFromPointer = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     #click-field { font: 16px monospace; width: 300px; border: 0; padding: 0; margin: 0 }
   </style>
   <input id="click-field" value="mmmmmmmmmm">`,
  `{ const field = document.getElementById("click-field");
     const rect = field.getBoundingClientRect();
     const before = field.selectionStart;
     // A few characters in, not half way across the box: the value is ten
     // monospace characters in a field three hundred pixels wide, so the middle
     // of the box is past the end of the text and would answer the last offset
     // truthfully.
     const hit = __blitsenInjectPointerAt("mousedown", rect.x + 25,
       rect.y + rect.height / 2, { button: 0, buttons: 1 });
     if (hit === null || hit.target !== field) throw new Error("the press must land on the field");
     const middle = field.selectionStart;
     if (!(middle > before) || middle >= field.value.length)
       throw new Error("a press inside the text puts the caret there, not at an end: " + middle);
     // Dragging from there selects, and the drag belongs to the field it started
     // in even after the pointer has left it.
     __blitsenInjectPointerAt("mousemove", rect.x + rect.width - 1, rect.y + rect.height / 2,
       { buttons: 1 });
     if (field.selectionStart !== middle || field.selectionEnd <= middle)
       throw new Error("dragging extends the selection from where the press landed: "
         + field.selectionStart + "-" + field.selectionEnd);
     field.setAttribute("data-caret", "ok"); }`,
  400,
  120,
));
assert.equal(
  caretFromPointer.nodes.find(node => node.attributes.id === "click-field")
    .attributes["data-caret"],
  "ok");

// The shape a code editor has: text painted by ordinary elements, and one
// off-screen textarea that every key actually goes to. Nothing in the tree the
// user presses on is focusable, so the editor focuses the textarea itself from
// its `mousedown` handler and cancels the event to keep it. Taking focus at
// `click` instead undid that one event later and sent every keystroke to the
// body — which is Monaco, and any editor built the same way.
const editorShaped = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     #surface { width: 300px; height: 80px }
     #hidden { position: absolute; top: -1000px; width: 1px; height: 1px }
   </style>
   <div id="surface"><span id="glyphs">function greet() {}</span></div>
   <textarea id="hidden"></textarea>`,
  `{ const surface = document.getElementById("surface");
     const hidden = document.getElementById("hidden");
     // Exactly what an editor's mouse handler does: put the caret in the
     // textarea it owns, then refuse the default that would take it away.
     surface.addEventListener("mousedown", event => { hidden.focus(); event.preventDefault(); });
     const rect = surface.getBoundingClientRect();
     const at = (type, init) => __blitsenInjectPointerAt(type, rect.x + rect.width / 2,
       rect.y + rect.height / 2, init);
     at("mousedown", { button: 0, buttons: 1 });
     if (document.activeElement !== hidden)
       throw new Error("the editor's own focus call must survive its mousedown");
     at("mouseup", { button: 0, buttons: 0 });
     at("click", { button: 0, buttons: 0 });
     if (document.activeElement !== hidden)
       throw new Error("and the click that follows must not take it back: "
         + (document.activeElement.id || document.activeElement.nodeName));
     // Which is the only reason the keys land anywhere useful.
     for (const key of ["h", "i"]) __blitsenDispatchKeyboardEvent("keydown",
       { bubbles: true, cancelable: true, key, code: "Key" + key.toUpperCase() });
     if (hidden.value !== "hi")
       throw new Error("keys must reach the focused textarea: " + JSON.stringify(hidden.value));
     surface.setAttribute("data-editor", "ok"); }`,
  400,
  120,
));
assert.equal(
  editorShaped.nodes.find(node => node.attributes.id === "surface").attributes["data-editor"], "ok");

// And it is painted, which is the half a reported offset cannot stand in for.
// The renderer highlights a selection only inside the focused control, so the
// same range in the same document either paints or does not depending on where
// focus is — which is exactly the invariant the focus mirroring exists for.
const selectionPixels = script => JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0; background: #fff }
     input { font: 16px monospace; width: 300px; border: 0; padding: 0; margin: 0; color: #000 }
   </style>
   <input id="painted" value="mmmmmmmmmm"><input id="elsewhere">`,
  script,
  400,
  60,
)).paint_colors.find(color => color.rgba === "#b4d5ffff")?.pixels ?? 0;

const highlighted = selectionPixels(
  `{ const field = document.getElementById("painted");
     field.focus();
     field.setSelectionRange(0, field.value.length); }`);
const unfocused = selectionPixels(
  `{ const field = document.getElementById("painted");
     field.setSelectionRange(0, field.value.length);
     document.getElementById("elsewhere").focus(); }`);
assert.ok(highlighted > 500,
  `a selection in the focused field is painted, not merely reported: ${highlighted} pixels`);
assert.equal(unfocused, 0, "and nothing is highlighted in a field that does not have focus");
