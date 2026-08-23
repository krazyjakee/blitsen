use super::*;

/// The attribute is the default; the property is the state. Everything in
/// the form surface rests on the two moving independently.
#[test]
fn control_state_and_content_attribute_move_independently() {
    let mut dom = viewport_document(
        r#"<input id="text" value="start"><input id="box" type="checkbox" checked>
               <textarea id="notes">typed in</textarea>
               <select><option id="first" value="a">A</option>
               <option id="second" value="b" selected>B</option></select>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let value = DomName::attribute("value");
    let text = dom.get_element_by_id("text").unwrap().unwrap();
    let notes = dom.get_element_by_id("notes").unwrap().unwrap();
    let checkbox = dom.get_element_by_id("box").unwrap().unwrap();
    let second = dom.get_element_by_id("second").unwrap().unwrap();

    assert_eq!(dom.form_value(text).unwrap(), "start");
    assert_eq!(dom.form_value(notes).unwrap(), "typed in");
    assert!(dom.form_checked(checkbox).unwrap());
    assert!(dom.form_checked(second).unwrap());

    dom.set_form_value(text, "edited").unwrap();
    dom.set_form_checked(checkbox, false).unwrap();
    dom.set_form_checked(second, false).unwrap();
    assert_eq!(
        dom.attribute(text, &value).unwrap().as_deref(),
        Some("start"),
        "assigning a value must not write the attribute it defaulted from"
    );
    assert!(
        dom.attribute(checkbox, &DomName::attribute("checked"))
            .unwrap()
            .is_some()
    );
    assert!(
        dom.attribute(second, &DomName::attribute("selected"))
            .unwrap()
            .is_some()
    );

    // The other direction: a default that changes after the fact is still
    // only the default.
    dom.set_attribute(text, &value, "new default").unwrap();
    dom.remove_attribute(checkbox, &DomName::attribute("checked"))
        .unwrap();
    dom.flush_layout().unwrap();
    assert_eq!(dom.form_value(text).unwrap(), "edited");
    assert!(!dom.form_checked(checkbox).unwrap());
    assert!(!dom.form_checked(second).unwrap());

    // A control nothing has assigned to still follows its attribute.
    let untouched = dom.get_element_by_id("first").unwrap().unwrap();
    dom.set_attribute(untouched, &DomName::attribute("selected"), "")
        .unwrap();
    assert!(dom.form_checked(untouched).unwrap());
}

/// The value JavaScript reads is the value the user sees, because there is
/// only one of them: assigning it repaints the control.
#[test]
fn an_assigned_value_is_the_one_the_renderer_paints() {
    let mut dom = viewport_document(
        r#"<input id="field" style="font: 32px sans-serif; color: #000; width: 380px">"#,
        1.0,
    );
    let field = dom.get_element_by_id("field").unwrap().unwrap();
    dom.flush_layout().unwrap();
    // Glyph ink rather than every opaque pixel: the control's own box paints
    // whether or not there is anything in it.
    let inked = |pixels: Vec<u8>| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 0 && pixel[..3].iter().all(|channel| *channel < 100))
            .count()
    };
    let blank = inked(render(&mut dom, 400, 60));

    dom.set_form_value(field, "WRITTEN").unwrap();
    dom.flush_layout().unwrap();
    let written = inked(render(&mut dom, 400, 60));
    assert!(
        written > blank + 200,
        "an assigned value painted {written} pixels against {blank} blank; \
             the control state JavaScript writes is not the one being rendered"
    );
}

/// Parley's composing range is the preedit store and the painted text. This
/// proves an IME update does not live in an invisible bridge-side string, and
/// that committing replaces rather than appends to the marked range.
#[test]
fn ime_preedit_is_painted_and_commit_replaces_the_marked_range() {
    let mut dom = fixture_document(
        r#"<style>
             @font-face { font-family: "World";
                          src: url("block-world.ttf") format("truetype") }
             input { font: 40px "World"; color: #000; border: 0;
                     background: transparent; width: 300px; height: 60px }
           </style>
           <input id="field">"#,
        None,
    );
    dom.flush_layout().unwrap();
    let field = dom.get_element_by_id("field").unwrap().unwrap();
    dom.set_focused(Some(field)).unwrap();
    let dark_pixels = |pixels: Vec<u8>| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 0 && pixel[..3].iter().all(|channel| *channel < 80))
            .count()
    };
    let blank = dark_pixels(render(&mut dom, 320, 80));

    assert!(dom.set_form_composition(field, "🙂", Some((4, 4))).unwrap());
    assert_eq!(dom.form_value(field).unwrap(), "🙂");
    assert!(
        dom.document
            .get_node(field)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .editor
            .raw_compose()
            .is_some(),
        "the editor records a real marked range"
    );
    dom.flush_layout().unwrap();
    let preedit = dark_pixels(render(&mut dom, 320, 80));
    assert!(
        preedit > blank + 4,
        "the preedit added {preedit} dark pixels to a {blank}-pixel blank control"
    );

    dom.set_form_composition(field, "🙂🙂", Some((4, 8)))
        .unwrap();
    assert_eq!(dom.form_value(field).unwrap(), "🙂🙂");
    dom.commit_form_composition(field, "🙂").unwrap();
    assert_eq!(dom.form_value(field).unwrap(), "🙂");
    assert!(
        dom.document
            .get_node(field)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap()
            .editor
            .raw_compose()
            .is_none(),
        "commit removes the marked range state"
    );
}

/// Candidate-window placement follows the live editor caret and readonly
/// controls do not ask the platform for an IME.
#[test]
fn focused_editable_controls_expose_a_viewport_caret_for_the_native_ime() {
    let mut dom = viewport_document(
        r#"<style>body { margin: 0 } input { margin: 10px; padding: 4px; font: 20px sans-serif }</style>
           <input id="field" value="abc"><input id="locked" readonly value="no">"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let field = dom.get_element_by_id("field").unwrap().unwrap();
    let locked = dom.get_element_by_id("locked").unwrap().unwrap();

    dom.set_focused(Some(field)).unwrap();
    let (target, area) = dom
        .focused_form_cursor_area()
        .expect("an editable input has a candidate-window area");
    assert_eq!(target, field);
    assert!(
        area.x >= 10.0 && area.y >= 10.0,
        "area is viewport-relative: {area:?}"
    );
    assert!(
        area.width >= 1.0 && area.height > 10.0,
        "area is a caret: {area:?}"
    );
    dom.set_form_composition(field, "xy", Some((2, 2))).unwrap();
    dom.flush_layout().unwrap();
    let (_, marked_area) = dom
        .focused_form_cursor_area()
        .expect("an active preedit has a candidate-window area");
    assert!(
        marked_area.width > area.width,
        "the candidate area expands around marked text: {marked_area:?}"
    );

    dom.set_focused(Some(locked)).unwrap();
    assert_eq!(dom.focused_form_cursor_area(), None);
}
