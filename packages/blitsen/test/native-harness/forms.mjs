import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

// Form controls. The whole of this is the attribute/property distinction: the
// attribute is the control's default and the property is its current state, so
// each half of every pair below is asserted to move without the other.
const formControls = JSON.parse(native.runBridgeHarness(
  `<form id="form">
     <input id="text" name="who" value="start">
     <input id="box" type="checkbox" checked>
     <input id="radio-a" type="radio" name="pick" checked><input id="radio-b" type="radio" name="pick">
     <textarea id="notes">typed in</textarea>
     <select id="choice"><option id="first" value="a">A</option>
       <option id="second" value="b" selected>B</option></select>
     <button id="send" type="submit" value="go">Send</button>
   </form>`,
  `{ const expect = (actual, wanted, what) => {
       if (JSON.stringify(actual) !== JSON.stringify(wanted))
         throw new Error(what + ": " + JSON.stringify(actual) + " is not " + JSON.stringify(wanted));
     };
     const byId = id => document.getElementById(id);

     const text = byId("text");
     expect(text.value, "start", "value starts at the attribute");
     expect(text.defaultValue, "start", "defaultValue is the attribute");
     text.value = "edited";
     expect(text.value, "edited", "the property holds what was assigned");
     expect(text.getAttribute("value"), "start", "assigning value must not write the attribute");
     expect(text.defaultValue, "start", "defaultValue still reads the attribute");
     text.setAttribute("value", "new default");
     expect(text.value, "edited", "a later attribute write must not clobber the value");
     expect(text.defaultValue, "new default", "defaultValue follows the attribute");
     expect([text.type, text.name, text.disabled, text.form === byId("form")],
       ["text", "who", false, true], "the reflected basics");

     const box = byId("box");
     expect([box.checked, box.defaultChecked], [true, true], "checked starts at the attribute");
     box.checked = false;
     expect([box.checked, box.hasAttribute("checked"), box.defaultChecked], [false, true, true],
       "checkedness and the checked attribute diverge");
     box.removeAttribute("checked");
     expect([box.checked, box.defaultChecked], [false, false], "removing the default leaves the state");
     box.setAttribute("checked", "");
     expect([box.checked, box.defaultChecked], [false, true], "restoring the default leaves the state");

     byId("radio-b").checked = true;
     expect(byId("radio-a").checked, false, "a radio group has one member checked");

     const notes = byId("notes");
     expect([notes.value, notes.defaultValue], ["typed in", "typed in"],
       "a textarea's child text is its value and its default");
     notes.value = "rewritten";
     expect([notes.value, notes.defaultValue, notes.textContent],
       ["rewritten", "typed in", "typed in"], "a textarea's value leaves its children alone");

     const choice = byId("choice");
     const second = byId("second");
     expect([choice.options.length, choice.length], [2, 2], "options is a collection of the options");
     expect([choice.value, choice.selectedIndex], ["b", 1], "the select reads its selected option");
     expect([choice.options[0].index, second.index], [0, 1], "an option's index is its position");
     expect([choice.options[0].text, choice.options[0].value], ["A", "a"], "option text and value");
     choice.value = "a";
     expect([choice.value, choice.selectedIndex, choice.selectedOptions.length], ["a", 0, 1],
       "assigning the select's value moves the selection");
     expect([second.selected, second.hasAttribute("selected"), second.defaultSelected],
       [false, true, true], "selectedness and the selected attribute diverge");
     expect(choice.querySelector(":checked") === choice.options[0], true,
       "the selected option is the one :checked matches");
     const added = document.createElement("option");
     added.value = "c";
     choice.appendChild(added);
     expect(choice.options.length, 3, "a re-read of options sees what was added");

     const form = byId("form");
     expect(form.elements.length, 7, "form.elements lists the controls it owns");
     expect("submit" in form, false, "the navigating half of submission stays absent");
     let submits = 0;
     let submitters = [];
     form.addEventListener("submit", event => {
       submits++;
       submitters.push(event.submitter && event.submitter.id);
       event.preventDefault();
     });
     form.requestSubmit();
     __blitsenInjectMouseEvent("click", byId("send"), { bubbles: true, cancelable: true });
     expect([submits, submitters], [2, [null, "send"]],
       "requestSubmit and a submit button both raise a cancelable submit event");
     expect([byId("send").value, byId("send").type], ["go", "submit"], "a button's value and type");

     // The legacy event factory, in the shape Svelte's custom_event helper uses.
     const legacy = document.createEvent("CustomEvent");
     legacy.initCustomEvent("ping", true, true, { n: 7 });
     let detail = null;
     form.addEventListener("ping", event => { detail = event.detail.n; });
     form.dispatchEvent(legacy);
     expect([legacy.type, legacy.bubbles, detail], ["ping", true, 7],
       "createEvent + initCustomEvent produce a dispatchable event carrying its detail");
     let refused;
     try { document.createEvent("MouseEvents"); } catch (error) { refused = error.name; }
     expect(refused, "NotSupportedError", "an interface the factory does not answer is refused");

     // A control's own activation runs only when the click was not cancelled.
     const cancel = event => event.preventDefault();
     box.addEventListener("click", cancel);
     __blitsenInjectMouseEvent("click", box, { bubbles: true, cancelable: true });
     expect(box.checked, false, "a cancelled click does not toggle the checkbox");
     box.removeEventListener("click", cancel);
     __blitsenInjectMouseEvent("click", box, { bubbles: true, cancelable: true });
     expect(box.checked, true, "clicking a checkbox toggles it");

     form.setAttribute("data-form-controls", "ok"); }`,
  400,
  300,
));
assert.equal(formControls.nodes.find(node => node.attributes.id === "form")
  .attributes["data-form-controls"], "ok");

