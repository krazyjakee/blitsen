  // Form controls. The attribute is the control's *default* and the property is
  // its current state: HTML calls the divergence the dirty value flag, and
  // getting it backwards would look like it worked. `value` and `checked` read
  // and write the state the renderer paints from — there is no second store
  // here that could disagree with the pixels — while `defaultValue` and
  // `defaultChecked` are the attribute reflections.
  const INPUT_TYPES = ["button", "checkbox", "color", "date", "datetime-local", "email", "file",
    "hidden", "image", "month", "number", "password", "radio", "range", "reset", "search",
    "submit", "tel", "text", "time", "url", "week"];
  // The types whose value is control state rather than the attribute. The rest
  // are HTML's default mode: `value` is the attribute and nothing else.
  const VALUE_TYPES = ["color", "date", "datetime-local", "email", "month", "number", "password",
    "range", "search", "tel", "text", "time", "url", "week"];
  const CHECKABLE_TYPES = ["checkbox", "radio"];
  const SUBMIT_TYPES = ["submit", "image"];
  // What `form.elements` lists, minus the form-associated custom elements this
  // runtime has no custom elements to have.
  const FORM_CONTROLS = "button, fieldset, input, object, output, select, textarea";
  const reflected = (element, name) => element.getAttribute(name) ?? "";
  const controlValue = element => call("formValue", element[handle]);
  const setControlValue = (element, value) => call("setFormValue", element[handle], value);
  const controlChecked = element => call("formChecked", element[handle]);
  const setControlChecked = (element, checked) => call("setFormChecked", element[handle], checked);
  // The form owner: an explicit `form` attribute naming one, else the ancestor.
  const formOwner = element => {
    const named = element.getAttribute("form");
    if (named === null) return element.closest("form");
    const owner = document.getElementById(named);
    return owner !== null && elementTag(owner) === "form" ? owner : null;
  };
  const listedControls = form =>
    [...document.querySelectorAll(FORM_CONTROLS)].filter(control => formOwner(control) === form);
  const options = select => [...select.querySelectorAll("option")];
  const isSubmitButton = element => {
    const type = (element.getAttribute("type") ?? "").toLowerCase();
    if (elementTag(element) === "button") return type === "" || type === "submit";
    return elementTag(element) === "input" && SUBMIT_TYPES.includes(type);
  };
  // A radio group has one member checked at a time, which is what makes it a
  // group: the siblings are written here rather than left disagreeing with what
  // is painted.
  const setChecked = (input, checked) => {
    setControlChecked(input, checked);
    if (!checked || input.type !== "radio") return;
    const name = input.getAttribute("name");
    if (!name) return;
    const owner = formOwner(input);
    for (const other of document.querySelectorAll('input[type="radio"]'))
      if (other !== input && other.getAttribute("name") === name && formOwner(other) === owner)
        setControlChecked(other, false);
  };
  const setSelected = (option, selected) => {
    setControlChecked(option, selected);
    const select = option.closest("select");
    if (!selected || select === null || select.multiple) return;
    for (const other of options(select)) if (other !== option) setControlChecked(other, false);
  };
  // There is nowhere to navigate to, so submission is the event and nothing
  // else — which is the half a single-page application actually uses, and the
  // half it can cancel. See COMPATIBILITY.md for why `submit()` is absent.
  const submitForm = (form, submitter) =>
    form.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true, submitter }));
  // The activation behaviour a control has of its own, run after the click and
  // only when the click was not cancelled — which is what makes preventDefault
  // on a checkbox or a submit button mean anything.
  const activateControl = target => {
    for (let element = target; element instanceof Element; element = element.parentNode) {
      if (element.hasAttribute("disabled")) return;
      if (elementTag(element) === "input" && CHECKABLE_TYPES.includes(element.type)) {
        if (element.type === "radio" && element.checked) return;
        setChecked(element, element.type === "radio" || !element.checked);
        element.dispatchEvent(new Event("input", { bubbles: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
        return;
      }
      if (isSubmitButton(element)) {
        const form = formOwner(element);
        if (form !== null) submitForm(form, element);
        return;
      }
    }
  };

  // Text selection. HTML gives it to `<textarea>` and to the input types whose
  // value is one line of plain text, and to nothing else: a date or a colour
  // has a value with structure and no caret in it, and `selectionStart` on one
  // is defined to answer null rather than 0 — which is what a component reads
  // before it tries to put a caret back after a re-render.
  const SELECTABLE_TYPES = ["text", "search", "url", "tel", "password"];
  const isSelectable = element => elementTag(element) === "textarea" ||
    SELECTABLE_TYPES.includes(element.type);
  // Read from and written to the renderer's own editor, for the reason `value`
  // is: it is what paints the caret and the highlight, so a range set here is a
  // range the user can see.
  const controlSelection = element => call("formSelection", element[handle]);
  const toOffset = value => {
    // `null` counts as 0, which is what HTML says of the nullable arguments.
    const number = Math.trunc(Number(value));
    return Number.isFinite(number) ? Math.max(number, 0) : 0;
  };
  // HTML's "set the selection range". The end is clamped to the value and the
  // start to the end, so a range cannot end up inside out however it was asked
  // for, and a direction that is neither name is `"none"` — a range with no
  // direction of its own, which is what one set from script has.
  const selectRange = (element, start, end, direction) => {
    if (!isSelectable(element)) return;
    const length = element.value.length;
    end = Math.min(toOffset(end), length);
    start = Math.min(toOffset(start), end);
    call("setFormSelection", element[handle], start, end,
      direction === "forward" || direction === "backward" ? direction : "none");
  };
  // The setters throw where the getters answer null: HTML makes reading a
  // caret off a control that has none a question with an answer, and writing
  // one to it a mistake.
  const requireSelectable = element => {
    if (!isSelectable(element))
      throw new DOMException("this control has no text selection", "InvalidStateError");
  };
  const selectionMembers = {
    start(element) { return isSelectable(element) ? controlSelection(element).start : null; },
    setStart(element, value) {
      requireSelectable(element);
      const { end, direction } = controlSelection(element);
      const start = toOffset(value);
      selectRange(element, start, Math.max(start, end), direction);
    },
    end(element) { return isSelectable(element) ? controlSelection(element).end : null; },
    setEnd(element, value) {
      requireSelectable(element);
      const { start, direction } = controlSelection(element);
      selectRange(element, start, value, direction);
    },
    direction(element) { return isSelectable(element) ? controlSelection(element).direction : null; },
    setDirection(element, value) {
      requireSelectable(element);
      const { start, end } = controlSelection(element);
      selectRange(element, start, end, String(value));
    },
    range(element, start, end, direction) {
      requireSelectable(element);
      selectRange(element, start, end, direction);
    },
    // A no-op on a control with no selection rather than a throw, which is the
    // one place HTML lets this pair disagree.
    all(element) { selectRange(element, 0, element.value.length, "none"); },
  };

  class HTMLFormControlElement extends Element {
    get name() { return reflected(this, "name"); }
    set name(value) { this.setAttribute("name", value); }
    get disabled() { return this.hasAttribute("disabled"); }
    set disabled(value) { this.toggleAttribute("disabled", Boolean(value)); }
    get form() { return formOwner(this); }
  }

  class HTMLInputElement extends HTMLFormControlElement {
    get type() {
      const type = (this.getAttribute("type") ?? "").toLowerCase();
      return INPUT_TYPES.includes(type) ? type : "text";
    }
    set type(value) { this.setAttribute("type", value); }
    get value() {
      if (VALUE_TYPES.includes(this.type)) return controlValue(this);
      // A checkbox submits "on" when it carries no value of its own.
      return this.getAttribute("value") ?? (CHECKABLE_TYPES.includes(this.type) ? "on" : "");
    }
    set value(value) {
      value = value === null ? "" : String(value);
      if (VALUE_TYPES.includes(this.type)) setControlValue(this, value);
      else this.setAttribute("value", value);
    }
    get defaultValue() { return reflected(this, "value"); }
    set defaultValue(value) { this.setAttribute("value", value); }
    get checked() { return controlChecked(this); }
    set checked(value) { setChecked(this, Boolean(value)); }
    get defaultChecked() { return this.hasAttribute("checked"); }
    set defaultChecked(value) { this.toggleAttribute("checked", Boolean(value)); }
    get selectionStart() { return selectionMembers.start(this); }
    set selectionStart(value) { selectionMembers.setStart(this, value); }
    get selectionEnd() { return selectionMembers.end(this); }
    set selectionEnd(value) { selectionMembers.setEnd(this, value); }
    get selectionDirection() { return selectionMembers.direction(this); }
    set selectionDirection(value) { selectionMembers.setDirection(this, value); }
    setSelectionRange(start, end, direction = "none") {
      selectionMembers.range(this, start, end, direction);
    }
    select() { selectionMembers.all(this); }
  }

  class HTMLTextAreaElement extends HTMLFormControlElement {
    get type() { return "textarea"; }
    get value() { return controlValue(this); }
    set value(value) { setControlValue(this, value === null ? "" : String(value)); }
    // A textarea's child text is its default value, where an input has an
    // attribute; the renderer is given it too, so an untouched textarea paints
    // what it reads.
    get defaultValue() { return this.textContent; }
    set defaultValue(value) { this.textContent = value; }
    get selectionStart() { return selectionMembers.start(this); }
    set selectionStart(value) { selectionMembers.setStart(this, value); }
    get selectionEnd() { return selectionMembers.end(this); }
    set selectionEnd(value) { selectionMembers.setEnd(this, value); }
    get selectionDirection() { return selectionMembers.direction(this); }
    set selectionDirection(value) { selectionMembers.setDirection(this, value); }
    setSelectionRange(start, end, direction = "none") {
      selectionMembers.range(this, start, end, direction);
    }
    select() { selectionMembers.all(this); }
  }

  class HTMLButtonElement extends HTMLFormControlElement {
    get type() {
      const type = (this.getAttribute("type") ?? "").toLowerCase();
      return type === "reset" || type === "button" ? type : "submit";
    }
    set type(value) { this.setAttribute("type", value); }
    get value() { return reflected(this, "value"); }
    set value(value) { this.setAttribute("value", value); }
  }

  class HTMLOptionElement extends Element {
    // Falling back to the text is the whole of what an option without a value
    // attribute submits.
    get value() { return this.getAttribute("value") ?? this.text; }
    set value(value) { this.setAttribute("value", value); }
    get text() { return this.textContent.replace(/\s+/g, " ").trim(); }
    set text(value) { this.textContent = value; }
    get label() { return this.getAttribute("label") ?? this.text; }
    set label(value) { this.setAttribute("label", value); }
    get selected() { return controlChecked(this); }
    set selected(value) { setSelected(this, Boolean(value)); }
    get defaultSelected() { return this.hasAttribute("selected"); }
    set defaultSelected(value) { this.toggleAttribute("selected", Boolean(value)); }
    get disabled() { return this.hasAttribute("disabled"); }
    set disabled(value) { this.toggleAttribute("disabled", Boolean(value)); }
    get index() {
      const select = this.closest("select");
      return select === null ? 0 : options(select).indexOf(this);
    }
    get form() {
      const select = this.closest("select");
      return select === null ? null : formOwner(select);
    }
  }

  class HTMLSelectElement extends HTMLFormControlElement {
    get type() { return this.multiple ? "select-multiple" : "select-one"; }
    get multiple() { return this.hasAttribute("multiple"); }
    set multiple(value) { this.toggleAttribute("multiple", Boolean(value)); }
    get size() { return Number(this.getAttribute("size")) || 0; }
    // Static, as every collection this runtime hands out is: a re-read sees the
    // options added since, the collection handed out before it does not.
    get options() { return new NodeList(options(this)); }
    get length() { return options(this).length; }
    get selectedOptions() { return new NodeList(options(this).filter(option => option.selected)); }
    // A drop-down always shows something, so one with nothing selected reports
    // its first enabled option rather than -1. That is the selectedness HTML
    // resets a drop-down to; what it does not do is stay at -1 after an
    // assignment that matched nothing. See COMPATIBILITY.md.
    get selectedIndex() {
      const list = options(this);
      const selected = list.findIndex(option => option.selected);
      if (selected >= 0 || this.multiple || this.size > 1) return selected;
      return list.findIndex(option => !option.disabled);
    }
    set selectedIndex(index) {
      index = Number(index);
      options(this).forEach((option, position) => setControlChecked(option, position === index));
    }
    get value() {
      const index = this.selectedIndex;
      return index < 0 ? "" : options(this)[index].value;
    }
    set value(value) {
      value = String(value);
      const list = options(this);
      const index = list.findIndex(option => option.value === value);
      list.forEach((option, position) => setControlChecked(option, position === index));
    }
  }

  class HTMLFormElement extends Element {
    get name() { return reflected(this, "name"); }
    set name(value) { this.setAttribute("name", value); }
    // Static, like every other collection here.
    get elements() { return new NodeList(listedControls(this)); }
    get length() { return listedControls(this).length; }
    requestSubmit(submitter = null) {
      if (submitter !== null) {
        if (!(submitter instanceof Element) || !isSubmitButton(submitter))
          throw new TypeError("the submitter must be a submit button");
        if (formOwner(submitter) !== this)
          throw new DOMException("the submitter does not belong to this form", "NotFoundError");
      }
      submitForm(this, submitter);
    }
  }

