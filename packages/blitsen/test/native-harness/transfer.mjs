import { strict as assert } from "node:assert";

import { native } from "./addon.mjs";

// Clipboard events and drag and drop (issue #93).
//
// The drag half runs through `__blitsenInjectDragEvent`, which is the same
// `dispatchDragEvent` entry point `drain_drag_input` calls with a winit
// `DragEntered`/`DragMoved`/`DragDropped`/`DragLeft` — so the DOM sequence
// asserted below is the shipping one. What the host does before that point,
// including turning a `PathBuf` into a path and a `file:` URL, is covered in
// Rust by `drag_drop`'s own tests.

const dropped = JSON.parse(native.runBridgeHarness(
  `<div id="zone">drop here</div><div id="other">elsewhere</div>`,
  `{ const zone = document.getElementById("zone");
     const other = document.getElementById("other");
     const seen = [];
     for (const element of [zone, other])
       for (const type of ["dragenter", "dragover", "dragleave", "drop"])
         element.addEventListener(type, event => {
           seen.push(element.id + ":" + type);
           if (element === zone && type !== "drop") event.preventDefault();
         });
     const files = { paths: ["/tmp/a b.txt", "/tmp/two.png"],
       uris: ["file:///tmp/a%20b.txt", "file:///tmp/two.png"] };

     let over;
     zone.addEventListener("dragover", event => { over = event; }, { once: true });
     __blitsenInjectDragEvent("over", zone, files);
     if (!(over instanceof DragEvent) || !(over instanceof MouseEvent))
       throw new Error("a drag event is a mouse event");
     if (!(over.dataTransfer instanceof DataTransfer)) throw new Error("dataTransfer");
     // The divergence: real absolute paths, and no File anywhere.
     if (over.dataTransfer.paths.join("|") !== "/tmp/a b.txt|/tmp/two.png")
       throw new Error("dropped paths: " + over.dataTransfer.paths);
     if ("files" in over.dataTransfer || "items" in over.dataTransfer)
       throw new Error("File objects must be absent rather than empty");
     if (over.dataTransfer.types.join(",") !== "text/uri-list,Files")
       throw new Error("transfer types: " + over.dataTransfer.types);
     if (over.dataTransfer.getData("url") !== "file:///tmp/a%20b.txt\\r\\nfile:///tmp/two.png")
       throw new Error("uri list: " + JSON.stringify(over.dataTransfer.getData("url")));
     // Read-only for the duration of the drag, which is a no-op and not a throw.
     over.dataTransfer.setData("text/plain", "ignored");
     if (over.dataTransfer.getData("text") !== "") throw new Error("a drag store is read-only");

     // Moving to a second element leaves the first before entering the second.
     __blitsenInjectDragEvent("over", other, files);
     // The drop lands on the element that accepted it; the one that did not
     // cancel its dragover never sees a drop at all.
     __blitsenInjectDragEvent("drop", other, files);
     __blitsenInjectDragEvent("over", zone, files);
     let drop;
     zone.addEventListener("drop", event => { drop = event; event.preventDefault(); },
       { once: true });
     __blitsenInjectDragEvent("drop", zone, files);
     if (drop === undefined || drop.dataTransfer.paths.length !== 2)
       throw new Error("the accepted drop carries the same paths");
     const expected = ["zone:dragenter", "zone:dragover",
       "zone:dragleave", "other:dragenter", "other:dragover",
       "other:dragover",
       "zone:dragenter", "zone:dragover",
       "zone:dragover", "zone:drop"];
     if (seen.join(",") !== expected.join(","))
       throw new Error("drag sequence: " + seen.join(","));

     // A drag that leaves the window takes the highlight down with it.
     const before = seen.length;
     __blitsenInjectDragEvent("over", zone, files);
     __blitsenInjectDragEvent("leave");
     if (seen.slice(before).join(",") !== "zone:dragenter,zone:dragover,zone:dragleave")
       throw new Error("leaving: " + seen.slice(before));

     // What an application constructs itself is writable and carries no files.
     const own = new DataTransfer();
     own.setData("text", "typed");
     if (own.getData("text/plain") !== "typed" || own.types.join(",") !== "text/plain")
       throw new Error("an application's own transfer is writable");
     if (own.paths.length !== 0) throw new Error("a constructed transfer has no paths");
     own.clearData();
     if (own.types.length !== 0) throw new Error("clearData");
     if (new ClipboardEvent("copy").clipboardData !== null)
       throw new Error("a constructed clipboard event has no store behind it");
     zone.setAttribute("data-drag", "ok"); }`,
  200,
  120,
));
assert.equal(dropped.nodes.find(node => node.attributes.id === "zone").attributes["data-drag"],
  "ok");

// The clipboard events, which need a real selection owner to round-trip
// through. Skipped on a headless Linux host for the same reason the clipboard
// module's own round-trips are.
const displayed = process.platform !== "linux"
  || Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY);
if (displayed) {
  const edited = JSON.parse(native.runBridgeHarness(
    `<input id="field" value="hello world">`,
    `{ const field = document.getElementById("field");
       field.focus();
       field.setSelectionRange(0, 5);
       const seen = [];
       for (const type of ["copy", "cut", "paste"])
         field.addEventListener(type, event => seen.push(type));
       const press = key => __blitsenDispatchKeyboardEvent("keydown",
         { key, code: "Key" + key.toUpperCase(), bubbles: true, cancelable: true, ctrlKey: true });

       press("c");
       // The selection reached the platform clipboard, which is what makes the
       // paste below meaningful rather than a value copied in process.
       if (__blitsenNativeClipboardRead("text") !== "hello")
         throw new Error("copy wrote: " + __blitsenNativeClipboardRead("text"));
       if (field.value !== "hello world") throw new Error("a copy does not edit");

       field.setSelectionRange(6, 11);
       press("x");
       if (__blitsenNativeClipboardRead("text") !== "world")
         throw new Error("cut wrote: " + __blitsenNativeClipboardRead("text"));
       if (field.value !== "hello ") throw new Error("cut left: " + field.value);

       let pasted;
       field.addEventListener("paste", event => { pasted = event; }, { once: true });
       press("v");
       if (pasted.clipboardData.getData("text/plain") !== "world")
         throw new Error("the paste event carries what is on the clipboard");
       if (field.value !== "hello world") throw new Error("paste left: " + field.value);

       // A cancelled copy writes what the listener put in the store instead of
       // the selection, which is the whole reason that store is writable.
       field.addEventListener("copy", event => {
         event.clipboardData.setData("text/plain", "replaced");
         event.preventDefault();
       }, { once: true });
       field.setSelectionRange(0, 5);
       press("c");
       if (__blitsenNativeClipboardRead("text") !== "replaced")
         throw new Error("a cancelled copy writes the listener's data");

       // A cancelled paste leaves the field alone.
       field.addEventListener("paste", event => event.preventDefault(), { once: true });
       const before = field.value;
       press("v");
       if (field.value !== before) throw new Error("a cancelled paste edits nothing");

       if (seen.join(",") !== "copy,cut,paste,copy,paste")
         throw new Error("clipboard events: " + seen);
       field.setAttribute("data-clipboard", "ok"); }`,
    200,
    120,
  ));
  assert.equal(
    edited.nodes.find(node => node.attributes.id === "field").attributes["data-clipboard"], "ok");
} else {
  console.log("clipboard events skipped: no DISPLAY or WAYLAND_DISPLAY on this host");
}
