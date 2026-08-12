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
            .chunks_exact(4)
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
