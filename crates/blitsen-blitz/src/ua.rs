//! User-agent rules Blitz's own default sheet leaves out.
//!
//! Blitz ships a trimmed copy of Gecko's `html.css` and a short form-control
//! shim; Gecko's `forms.css` and `ua.css` have no counterpart. What is missing
//! is not decoration — it is the part of the baseline an application never
//! writes down because every browser already provides it. Without these rules
//! a button hovers with the text caret, a disabled control looks live, a
//! `<fieldset>` lays out inline with its legend running into its contents, and
//! an `<a>` with no `href` is painted as a link.
//!
//! Only rules that this engine can actually honour are declared here. The
//! controls Blitz has no widget for — `<select>`, `<meter>`, `<progress>`,
//! `input[type=range|color|number]` — need painting before a UA rule would
//! mean anything, and `::placeholder` and `:focus-visible` need engine support
//! that does not exist yet, so none of them are addressed here.
//!
//! Declarations are kept to the ones Gecko uses, except where a value would
//! resolve to nothing: `ThreeDFace` for the `<fieldset>` border is written out
//! as the light-theme colour Gecko and Blink both resolve it to, because this
//! engine has no system-colour table behind that keyword.

/// The baseline Blitz omits, appended after its default sheet.
///
/// Author styles win over all of it: this is user-agent origin, which the
/// cascade places below anything a page declares.
pub(crate) const BASELINE_UA_CSS: &str = "\
/* Controls are not text. Gecko forms.css gives every button-flavoured control
   an arrow cursor and an unselectable label; without it the hit on a button's
   own text falls through to the text caret. */
button,
input[type=\"button\"],
input[type=\"submit\"],
input[type=\"reset\"],
input[type=\"checkbox\"],
input[type=\"radio\"],
input[type=\"color\"],
input[type=\"file\"],
input[type=\"range\"],
input[type=\"image\"],
select {
  cursor: default;
  user-select: none;
}

label {
  cursor: default;
}

/* A control that cannot be used says so. */
button:disabled,
input:disabled,
select:disabled,
textarea:disabled,
option:disabled,
optgroup:disabled {
  color: GrayText;
  cursor: default;
}

/* Blitz colours and underlines every <a>; only a link is a link.
   Written against the attribute rather than `:not(:any-link)`, which this
   engine cannot be trusted with: link-ness is matched ad hoc and never reaches
   `ElementState`, so stylo's style-sharing cache happily hands one anchor's
   style to a sibling anchor of the opposite kind. An attribute selector is a
   revalidation selector, so a shared candidate is re-checked against it. */
a:not([href]) {
  color: inherit;
  text-decoration: none;
}

/* Gecko forms.css: a textarea preserves the whitespace of its contents and
   sits on the text baseline rather than hanging below it. */
textarea {
  white-space: pre-wrap;
  vertical-align: text-bottom;
}
";
