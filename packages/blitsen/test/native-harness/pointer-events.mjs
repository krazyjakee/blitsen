import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

// Pointer events, driven through the same entry point the native window calls.
//
// `__blitsenInjectPointerAt` hit tests the laid-out tree and hands the result to
// `__blitsenDispatchPointerEvent`, which is exactly what `drain_pointer_input`
// does with a winit event — so what these checks exercise is the shipping path,
// not a parallel one written for them. What the *host* does with a touch before
// that point is covered in Rust, in `pointer_input`'s tests.

const touchTap = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     #tap { display: block; width: 200px; height: 40px }
   </style>
   <button id="tap">tap</button>`,
  `{ const tap = document.getElementById("tap");
     const box = tap.getBoundingClientRect();
     const x = box.x + box.width / 2, y = box.y + box.height / 2;
     const seen = [];
     for (const type of ["pointerdown", "mousedown", "pointermove", "mousemove",
       "pointerup", "mouseup", "click"])
       tap.addEventListener(type, event => seen.push(type));
     let down, move, up;
     tap.addEventListener("pointerdown", event => { down = event; });
     tap.addEventListener("pointermove", event => { move = event; });
     tap.addEventListener("pointerup", event => { up = event; });
     let compatibility;
     tap.addEventListener("mousedown", event => { compatibility = event; });

     const finger = { pointerId: 7, pointerType: "touch", isPrimary: true, force: 0.75 };
     __blitsenInjectPointerAt("pointerdown", x, y, finger);
     __blitsenInjectPointerAt("pointermove", x + 1, y, { ...finger, force: 0.8 });
     __blitsenInjectPointerAt("pointerup", x + 1, y, finger);

     // Pointer first, then the mouse event synthesised behind it, then the
     // click the lift produced. This is the whole compatibility claim.
     if (seen.join(",") !== "pointerdown,mousedown,pointermove,mousemove,pointerup,mouseup,click")
       throw new Error("pointer and compatibility mouse event order: " + seen);
     if (!(down instanceof PointerEvent) || !(down instanceof MouseEvent))
       throw new Error("a pointer event is a mouse event");
     if (compatibility instanceof PointerEvent || !(compatibility instanceof MouseEvent))
       throw new Error("the compatibility event is a MouseEvent and not a PointerEvent");
     if (down.pointerType !== "touch" || down.pointerId !== 7 || !down.isPrimary)
       throw new Error("pointer identity: " + [down.pointerType, down.pointerId, down.isPrimary]);
     if (down.pressure !== 0.75 || move.pressure !== 0.8)
       throw new Error("measured pressure: " + [down.pressure, move.pressure]);
     if (up.pressure !== 0)
       throw new Error("a lifted finger presses with nothing: " + up.pressure);
     if (down.buttons !== 1 || move.buttons !== 1 || up.buttons !== 0)
       throw new Error("buttons through the gesture: " + [down.buttons, move.buttons, up.buttons]);
     if (down.button !== 0 || move.button !== -1 || up.button !== 0)
       throw new Error("button through the gesture: " + [down.button, move.button, up.button]);
     if (compatibility.buttons !== 1 || compatibility.button !== 0)
       throw new Error("the compatibility mousedown carries the same button state");
     // The default action of the synthesised mousedown, which is the point of
     // synthesising it: a finger focuses a control exactly as a mouse does.
     if (document.activeElement !== tap)
       throw new Error("a tap focuses the control it landed on");

     // A mouse with no pressure sensor reports the value the spec substitutes.
     let mouseDown;
     tap.addEventListener("pointerdown", event => { mouseDown = event; });
     __blitsenInjectPointerAt("pointerdown", x, y, { pointerId: 1, pointerType: "mouse" });
     if (mouseDown.pressure !== 0.5 || mouseDown.pointerType !== "mouse" || mouseDown.pointerId !== 1)
       throw new Error("mouse pressure substitution: " + [mouseDown.pressure, mouseDown.pointerId]);
     __blitsenInjectPointerAt("pointerup", x, y, { pointerId: 1, pointerType: "mouse" });

     tap.setAttribute("data-pointer", "ok"); }`,
  300,
  120,
));
assert.equal(touchTap.nodes.find(node => node.attributes.id === "tap").attributes["data-pointer"],
  "ok");

const cancelledPress = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     #claim { display: block; width: 200px; height: 40px }
   </style>
   <button id="claim">claim</button>`,
  `{ const claim = document.getElementById("claim");
     const box = claim.getBoundingClientRect();
     const x = box.x + box.width / 2, y = box.y + box.height / 2;
     const seen = [];
     for (const type of ["pointerdown", "mousedown", "pointermove", "mousemove",
       "pointerup", "mouseup", "click"])
       claim.addEventListener(type, () => seen.push(type));
     const refuse = event => event.preventDefault();
     claim.addEventListener("pointerdown", refuse);

     const finger = { pointerId: 3, pointerType: "touch", isPrimary: true };
     __blitsenInjectPointerAt("pointerdown", x, y, finger);
     __blitsenInjectPointerAt("pointermove", x, y + 1, finger);
     __blitsenInjectPointerAt("pointerup", x, y + 1, finger);
     // A refused press takes the whole compatibility sequence with it for the
     // rest of that contact — which is how an application that has decided to
     // drive the gesture itself stops a stray click at the end of it.
     if (seen.join(",") !== "pointerdown,pointermove,pointerup")
       throw new Error("a cancelled pointerdown suppresses the mouse events: " + seen);
     if (document.activeElement === claim)
       throw new Error("a suppressed mousedown does not focus");

     claim.removeEventListener("pointerdown", refuse);
     seen.length = 0;
     const second = { pointerId: 4, pointerType: "touch", isPrimary: true };
     __blitsenInjectPointerAt("pointerdown", x, y, second);
     __blitsenInjectPointerAt("pointerup", x, y, second);
     if (seen.join(",") !== "pointerdown,mousedown,pointerup,mouseup,click")
       throw new Error("suppression is per contact, not permanent: " + seen);
     claim.setAttribute("data-cancelled", "ok"); }`,
  300,
  120,
));
assert.equal(
  cancelledPress.nodes.find(node => node.attributes.id === "claim").attributes["data-cancelled"],
  "ok");

const multiTouch = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     button { display: block; width: 200px; height: 40px }
   </style>
   <button id="first">first</button><button id="second">second</button>`,
  `{ const first = document.getElementById("first");
     const second = document.getElementById("second");
     const centre = element => {
       const box = element.getBoundingClientRect();
       return [box.x + box.width / 2, box.y + box.height / 2];
     };
     const [firstX, firstY] = centre(first);
     const [secondX, secondY] = centre(second);
     if (firstY === secondY) throw new Error("the two controls must not overlap");
     const seen = [];
     for (const element of [first, second])
       for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup", "click"])
         element.addEventListener(type, event =>
           seen.push(element.id + ":" + type + ":" + event.pointerId));
     const ids = new Set();
     for (const element of [first, second])
       element.addEventListener("pointerdown", event => ids.add(event.pointerId));

     // Two fingers down at once, on two different controls. The first is the
     // primary contact; the second is not, and so synthesises nothing.
     const one = { pointerId: 11, pointerType: "touch", isPrimary: true };
     const two = { pointerId: 12, pointerType: "touch", isPrimary: false };
     __blitsenInjectPointerAt("pointerdown", firstX, firstY, one);
     __blitsenInjectPointerAt("pointerdown", secondX, secondY, two);
     __blitsenInjectPointerAt("pointerup", secondX, secondY, two);
     __blitsenInjectPointerAt("pointerup", firstX, firstY, one);

     if (ids.size !== 2) throw new Error("each contact is its own pointer");
     const expected = [
       "first:pointerdown:11", "first:mousedown:undefined",
       "second:pointerdown:12",
       "second:pointerup:12",
       "first:pointerup:11", "first:mouseup:undefined", "first:click:undefined",
     ];
     // The load-bearing line is the last one. The second finger lifted first,
     // and if the press it ended were held per *button* rather than per pointer
     // it would have taken the first finger's press with it and there would be
     // no click at all — which is what a single shared "the mouse is down on X"
     // does to multi-touch.
     if (seen.join(" ") !== expected.join(" "))
       throw new Error("multi-touch dispatch:\\n  " + seen.join("\\n  "));
     first.setAttribute("data-multitouch", "ok"); }`,
  300,
  160,
));
assert.equal(
  multiTouch.nodes.find(node => node.attributes.id === "first").attributes["data-multitouch"],
  "ok");

const capture = JSON.parse(native.runBridgeHarness(
  `<style>
     body { margin: 0 }
     div { width: 200px; height: 40px }
   </style>
   <div id="handle">handle</div><div id="elsewhere">elsewhere</div>`,
  `{ const handle = document.getElementById("handle");
     const elsewhere = document.getElementById("elsewhere");
     const centre = element => {
       const box = element.getBoundingClientRect();
       return [box.x + box.width / 2, box.y + box.height / 2];
     };
     const [handleX, handleY] = centre(handle);
     const [awayX, awayY] = centre(elsewhere);
     const seen = [];
     for (const element of [handle, elsewhere])
       for (const type of ["pointerdown", "pointermove", "pointerup", "click",
         "gotpointercapture", "lostpointercapture"])
         element.addEventListener(type, event => seen.push(element.id + ":" + type));

     const finger = { pointerId: 21, pointerType: "touch", isPrimary: true };
     let capturedDuringDown;
     const take = () => {
       handle.setPointerCapture(21);
       // Capture is *pending* until the next pointer event, so the element does
       // not become this event's target retroactively and nothing has been
       // announced yet.
       capturedDuringDown = [handle.hasPointerCapture(21), seen.includes("handle:gotpointercapture")];
     };
     handle.addEventListener("pointerdown", take);
     __blitsenInjectPointerAt("pointerdown", handleX, handleY, finger);
     handle.removeEventListener("pointerdown", take);
     if (capturedDuringDown.join(",") !== "true,false")
       throw new Error("capture is requested during the event and settled after it: "
         + capturedDuringDown);

     // Every subsequent event from this pointer arrives at the handle, however
     // far from it the finger has moved.
     __blitsenInjectPointerAt("pointermove", awayX, awayY, finger);
     __blitsenInjectPointerAt("pointerup", awayX, awayY, finger);
     const expected = ["handle:pointerdown", "handle:gotpointercapture", "handle:pointermove",
       "handle:pointerup", "handle:click", "handle:lostpointercapture"];
     if (seen.join(" ") !== expected.join(" "))
       throw new Error("captured dispatch:\\n  " + seen.join("\\n  "));
     if (handle.hasPointerCapture(21))
       throw new Error("a contact that ended releases its capture");

     // A pointer that is not on the screen cannot be captured, and asking is an
     // error rather than a silent no-op — the finger has already lifted here.
     let refused = null;
     try { handle.setPointerCapture(21); } catch (error) { refused = error.name; }
     if (refused !== "NotFoundError") throw new Error("capturing a dead pointer: " + refused);

     // A cancellation releases the capture too, and produces no click.
     seen.length = 0;
     const second = { pointerId: 22, pointerType: "touch", isPrimary: true };
     const grab = () => handle.setPointerCapture(22);
     handle.addEventListener("pointerdown", grab);
     __blitsenInjectPointerAt("pointerdown", handleX, handleY, second);
     handle.removeEventListener("pointerdown", grab);
     __blitsenInjectPointerAt("pointermove", awayX, awayY, second);
     __blitsenInjectPointerAt("pointercancel", awayX, awayY, second);
     const cancelled = ["handle:pointerdown", "handle:gotpointercapture", "handle:pointermove",
       "handle:lostpointercapture"];
     if (seen.join(" ") !== cancelled.join(" "))
       throw new Error("cancelled capture:\\n  " + seen.join("\\n  "));

     // Releasing by hand puts the next event back where the finger actually is.
     seen.length = 0;
     const third = { pointerId: 23, pointerType: "touch", isPrimary: true };
     const hold = () => handle.setPointerCapture(23);
     handle.addEventListener("pointerdown", hold);
     __blitsenInjectPointerAt("pointerdown", handleX, handleY, third);
     handle.removeEventListener("pointerdown", hold);
     __blitsenInjectPointerAt("pointermove", awayX, awayY, third);
     handle.releasePointerCapture(23);
     __blitsenInjectPointerAt("pointermove", awayX, awayY, third);
     const released = ["handle:pointerdown", "handle:gotpointercapture", "handle:pointermove",
       "handle:lostpointercapture", "elsewhere:pointermove"];
     if (seen.join(" ") !== released.join(" "))
       throw new Error("released capture:\\n  " + seen.join("\\n  "));
     __blitsenInjectPointerAt("pointerup", awayX, awayY, third);
     handle.setAttribute("data-capture", "ok"); }`,
  300,
  160,
));
assert.equal(
  capture.nodes.find(node => node.attributes.id === "handle").attributes["data-capture"], "ok");
