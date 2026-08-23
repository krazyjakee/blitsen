  // Typing into a control: what a key does to a field once the keyboard events
  // have been dispatched and nothing cancelled them. Keydown comes first and in
  // full, because this *is* its default action — an application that calls
  // `preventDefault` on a keydown has stopped the character from being typed,
  // which is how a field that accepts only digits is written.
  //
  // An edit is announced before it happens and reported after: `beforeinput` is
  // cancelable and names what is about to happen, `input` is not and says what
  // did. The gap between the two is where a controlled component says no, and
  // the mutation in between goes through the renderer's own editor — so the
  // value, the caret and the pixels cannot disagree about what was typed.

  // Wider than the types with a selection, and deliberately: `number` and
  // `email` are typed into and have no caret to report. That is HTML's own
  // inconsistency rather than one invented here.
  const TYPED_INPUT_TYPES = [...SELECTABLE_TYPES, "email", "number"];
  // A disabled control takes no input at all; a readonly one still takes a
  // caret, which is why the two questions are separate. Neither is asked of
  // anything but an `<input>` or a `<textarea>`.
  const textControl = element => {
    if (!(element instanceof Element) || element.hasAttribute("disabled")) return null;
    const tag = elementTag(element);
    if (tag === "textarea") return "textarea";
    return tag === "input" && TYPED_INPUT_TYPES.includes(element.type) ? "input" : null;
  };
  const editableControl = element =>
    element?.hasAttribute("readonly") ? null : textControl(element);

  // History is local to a control and bounded both by transaction count and
  // by the UTF-16 storage represented by its before/after snapshots. One
  // accepted key, cut or paste default action is one transaction; typing is
  // deliberately not time-coalesced, so its boundaries are deterministic.
  const MAX_HISTORY_TRANSACTIONS = 100;
  const MAX_HISTORY_CODE_UNITS = 1_000_000;
  const textHistories = new WeakMap();
  const textSnapshot = target => ({
    value: target.value,
    selection: { ...controlSelection(target) },
  });
  const historyState = target => {
    let state = textHistories.get(target);
    if (state === undefined) {
      state = { undo: [], redo: [], units: 0 };
      textHistories.set(target, state);
    }
    return state;
  };
  const historyWeight = entry => entry.before.value.length + entry.after.value.length;
  const discardHistoryStack = (state, name) => {
    for (const entry of state[name]) state.units -= historyWeight(entry);
    state[name] = [];
  };
  const recordTextTransaction = (target, before, after) => {
    if (before === null || before.value === after.value) return;
    const state = historyState(target);
    discardHistoryStack(state, "redo");
    const entry = { before, after };
    const units = historyWeight(entry);
    if (units > MAX_HISTORY_CODE_UNITS) {
      discardHistoryStack(state, "undo");
      return;
    }
    while (state.undo.length >= MAX_HISTORY_TRANSACTIONS ||
           state.units + units > MAX_HISTORY_CODE_UNITS) {
      state.units -= historyWeight(state.undo.shift());
    }
    state.undo.push(entry);
    state.units += units;
  };
  const restoreTextSnapshot = (target, snapshot) => {
    call("setFormValue", target[handle], snapshot.value);
    const { start, end, direction } = snapshot.selection;
    call("setFormSelection", target[handle], start, end, direction);
  };

  let compositionBefore = null;
  resetTextHistory = target => {
    textHistories.delete(target);
    // A controlled replacement during composition becomes the baseline of
    // the eventual commit; undo must never cross back into obsolete state.
    if (compositionTarget === target) compositionBefore = textSnapshot(target);
  };
  textCompositionOwns = target => compositionTarget === target;

  // One native composition belongs to the control that held focus when its
  // first preedit arrived. Updates keep targeting it until commit, cancellation
  // or a focus move; reading `activeElement` again midway would let an event
  // handler redirect half a composition into another field.
  let compositionTarget = null;

  const compositionInput = (target, operation, data, inputType, cursor = null) => {
    const before = new InputEvent("beforeinput", {
      bubbles: true, cancelable: operation !== "clear", data, inputType, isComposing: true,
    });
    if (!target.dispatchEvent(before)) return "canceled";
    // A beforeinput listener can move focus. That synchronously cancels this
    // composition; do not resurrect its preedit in the control that just
    // blurred.
    if (operation !== "clear" && compositionTarget !== target) return "aborted";
    const previous = target.value;
    if (operation === "set") {
      call("setFormComposition", target[handle], data ?? "",
        cursor === null ? "" : cursor[0], cursor === null ? "" : cursor[1]);
    } else if (operation === "commit") {
      call("commitFormComposition", target[handle], data ?? "");
      recordTextTransaction(target, compositionBefore, textSnapshot(target));
    } else {
      call("clearFormComposition", target[handle]);
    }
    if (target.value !== previous) {
      target.dispatchEvent(new InputEvent("input", {
        bubbles: true, data, inputType, isComposing: true,
      }));
    }
    return "applied";
  };

  const startComposition = target => {
    compositionTarget = target;
    compositionBefore = textSnapshot(target);
    target.dispatchEvent(new CompositionEvent("compositionstart", {
      bubbles: true, data: "",
    }));
  };

  cancelTextComposition = () => {
    const target = compositionTarget;
    if (target === null) return false;
    // Clear first so a focus change from a beforeinput listener cannot re-enter
    // cancellation for the same marked range.
    compositionTarget = null;
    compositionBefore = null;
    compositionInput(target, "clear", null, "deleteCompositionText");
    target.dispatchEvent(new CompositionEvent("compositionend", {
      bubbles: true, data: "",
    }));
    return true;
  };

  // Typed entry point for winit's `Ime` events. Cursor offsets stay UTF-8 byte
  // indices all the way to Parley; JavaScript does not reinterpret them as its
  // UTF-16 string offsets.
  const dispatchImeEvent = (kind, init = {}) => {
    kind = String(kind);
    if (kind === "enabled") return false;
    if (kind === "disabled") return cancelTextComposition();
    if (kind === "deleteSurrounding") return false;

    const data = String(init.data ?? "");
    let target = compositionTarget;
    if (target === null) {
      // Winit uses an empty preedit to clear an existing marked range. With no
      // range there is no composition to begin or DOM event to synthesize.
      if (kind === "preedit" && data === "") return false;
      target = editableControl(activeElement) === null ? null : activeElement;
      if (target === null) return false;
      startComposition(target);
      if (compositionTarget !== target) return false;
    }

    if (kind === "preedit") {
      target.dispatchEvent(new CompositionEvent("compositionupdate", {
        bubbles: true, data,
      }));
      if (compositionTarget !== target) return false;
      if (data === "") {
        compositionInput(target, "clear", null, "deleteCompositionText");
        if (compositionTarget !== target) return false;
        compositionTarget = null;
        compositionBefore = null;
        target.dispatchEvent(new CompositionEvent("compositionend", {
          bubbles: true, data: "",
        }));
        return true;
      }
      const cursor = init.cursorStart === null || init.cursorStart === undefined
        ? null : [Number(init.cursorStart), Number(init.cursorEnd)];
      return compositionInput(target, "set", data, "insertCompositionText", cursor) !== "aborted";
    }
    if (kind === "commit") {
      const result = compositionInput(target, "commit", data, "insertFromComposition");
      if (result === "aborted") return false;
      if (compositionTarget !== target) return false;
      // The platform has ended this composition even if script canceled the
      // committed insertion. Remove the marked text without synthesizing an
      // `input` for the canceled default action.
      if (result === "canceled") call("clearFormComposition", target[handle]);
      compositionTarget = null;
      compositionBefore = null;
      target.dispatchEvent(new CompositionEvent("compositionend", {
        bubbles: true, data,
      }));
      return true;
    }
    return false;
  };

  // What a navigation key means to a caret. `ArrowUp` in a single-line field is
  // the start of the value and `ArrowDown` its end, because there is no line
  // above or below one to reach. Ctrl is the modifier that widens each of them
  // — a word instead of a character, the whole value instead of a line — which
  // is what it does on the two platforms this runtime has a keyboard on.
  const caretMotion = (event, multiline) => {
    switch (event.key) {
      case "ArrowLeft": return event.ctrlKey ? "wordLeft" : "left";
      case "ArrowRight": return event.ctrlKey ? "wordRight" : "right";
      case "ArrowUp": return multiline && !event.ctrlKey ? "up" : "textStart";
      case "ArrowDown": return multiline && !event.ctrlKey ? "down" : "textEnd";
      case "Home": return event.ctrlKey ? "textStart" : "lineStart";
      case "End": return event.ctrlKey ? "textEnd" : "lineEnd";
      default: return null;
    }
  };

  // What an editing key means: the mutation to apply and the `inputType` the
  // events carry it under. `data` is the text the operation contributes, and a
  // deletion contributes none — `InputEvent.data` is null for every one of
  // them, which is what a listener switches on.
  const keyEdit = (event, multiline) => {
    switch (event.key) {
      case "Backspace": return event.ctrlKey
        ? { operation: "deleteWordBackward", inputType: "deleteWordBackward" }
        : { operation: "deleteBackward", inputType: "deleteContentBackward" };
      case "Delete": return event.ctrlKey
        ? { operation: "deleteWordForward", inputType: "deleteWordForward" }
        : { operation: "deleteForward", inputType: "deleteContentForward" };
      // A single-line field has no line to break: Enter in one is the form's
      // key, not the field's, and is left to the application.
      case "Enter": return multiline
        ? { operation: "insert", data: "\n", inputType: "insertLineBreak" } : null;
      default:
        // A printable key is one character long — `a` is text, `ArrowLeft`,
        // `Shift` and `F5` are not — and a character held with a command
        // modifier is a shortcut: Ctrl+S saves, it does not type an "s".
        if (event.key.length !== 1 || event.ctrlKey) return null;
        return { operation: "insert", data: event.key, inputType: "insertText" };
    }
  };

  // The edit, announced and then reported. A cancelled `beforeinput` still
  // counts as handled: the key was the field's and the field refused it, so it
  // must not fall through to scrolling the document behind it. `input` is
  // reported only when the value actually moved, which is why backspacing at
  // the start of a field is silent rather than a stream of empty edits.
  const applyTextEdit = (target, edit) => {
    const data = edit.data ?? null;
    const before = new InputEvent("beforeinput",
      { bubbles: true, cancelable: true, inputType: edit.inputType, data });
    if (!target.dispatchEvent(before)) return true;
    const snapshot = textSnapshot(target);
    const previous = target.value;
    call("editFormValue", target[handle], edit.operation, edit.data ?? "");
    if (target.value !== previous) {
      recordTextTransaction(target, snapshot, textSnapshot(target));
      target.dispatchEvent(new InputEvent("input",
        { bubbles: true, inputType: edit.inputType, data }));
    }
    return true;
  };

  const applyHistoryEdit = (target, inputType) => {
    // The first undo while a preedit is live discards that uncommitted marked
    // text. A second shortcut reaches the last committed transaction.
    if (compositionTarget === target) return cancelTextComposition();
    const state = historyState(target);
    const source = inputType === "historyUndo" ? state.undo : state.redo;
    const destination = inputType === "historyUndo" ? state.redo : state.undo;
    const entry = source[source.length - 1];
    if (entry === undefined) return true;
    const before = new InputEvent("beforeinput",
      { bubbles: true, cancelable: true, inputType, data: null });
    if (!target.dispatchEvent(before)) return true;
    // A listener can replace the controlled value, clear history or move
    // focus. In any of those cases its decision wins over this default action.
    if (activeElement !== target || editableControl(target) === null ||
        textHistories.get(target) !== state || source[source.length - 1] !== entry) return true;
    source.pop();
    destination.push(entry);
    restoreTextSnapshot(target, inputType === "historyUndo" ? entry.before : entry.after);
    target.dispatchEvent(new InputEvent("input",
      { bubbles: true, inputType, data: null }));
    return true;
  };

  const historyInputType = event => {
    if (event.altKey || !(event.ctrlKey || event.metaKey)) return null;
    const key = event.key.toLowerCase();
    if (key === "z") return event.shiftKey ? "historyRedo" : "historyUndo";
    return key === "y" && !event.shiftKey ? "historyRedo" : null;
  };

  // The default action of a keydown that reached a text control. Reports
  // whether the key was the control's, because a key it took is not also a
  // scroll: typing a space into a field must not page the document down behind
  // it, and Home must not jump to the top of it.
  const textEditingKeydown = (event, target) => {
    const kind = textControl(target);
    if (kind === null) return false;
    const history = historyInputType(event);
    if (history !== null)
      return editableControl(target) === null ? false : applyHistoryEdit(target, history);
    if (event.altKey || event.metaKey) return false;
    const multiline = kind === "textarea";
    // Select-all is a selection change and not an edit: there is no `input`
    // operation behind it and nothing for a `beforeinput` to cancel. Written
    // straight to the editor rather than through `setSelectionRange`, because a
    // `number` field has a selection the user can make and no API to read it.
    if (event.ctrlKey && event.key.toLowerCase() === "a") {
      call("setFormSelection", target[handle], 0, target.value.length, "forward");
      return true;
    }
    const motion = caretMotion(event, multiline);
    if (motion !== null)
      return call("moveFormSelection", target[handle], motion, Boolean(event.shiftKey));
    const edit = editableControl(target) === null ? null : keyEdit(event, multiline);
    return edit === null ? false : applyTextEdit(target, edit);
  };

  // Clicking into a field puts the caret where the click landed, and dragging
  // from there selects. The point crosses to the renderer unresolved: which
  // character a pixel is inside is a question only the laid-out text can
  // answer, and it is the same question the caret is painted from.
  //
  // Taken from viewport coordinates against the control's own box rather than
  // from the event's offsets, because a drag that left the field still extends
  // the selection inside it — and by then the event's target is something else.
  const caretFromMouse = (control, event, extend) => {
    const rect = control.getBoundingClientRect();
    call("moveFormCaret", control[handle], event.clientX - rect.left,
      event.clientY - rect.top, extend);
  };
  // The field a press landed in, held for as long as the button is. Not read
  // back off `activeElement`, because focus arrives with the *click* — a drag
  // that started in an unfocused field would find the wrong element under it,
  // or none.
  let caretDragControl = null;
  const textEditingMouse = (type, target, event) => {
    if (type === "mousedown" && event.button === 0) {
      caretDragControl = textControl(target) === null ? null : target;
      if (caretDragControl !== null) caretFromMouse(caretDragControl, event, event.shiftKey);
    } else if (type === "mousemove" && caretDragControl !== null) {
      if ((event.buttons & 1) === 0) caretDragControl = null;
      else caretFromMouse(caretDragControl, event, true);
    } else if (type === "mouseup") caretDragControl = null;
  };
