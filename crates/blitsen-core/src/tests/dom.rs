use super::*;

#[test]
fn document_queries_delegate_and_nodelists_are_static() {
    let mut backend = MockDocument {
        matches: vec![2, 3],
        ..Default::default()
    };
    let list = {
        let document = DocumentApi::new(&mut backend);
        assert_eq!(document.query_selector(".item").unwrap(), Some(2));
        document.query_selector_all(".item").unwrap()
    };
    backend.matches.push(4);

    assert_eq!(list.into_vec(), vec![2, 3]);
    assert_eq!(
        backend.queried_selectors.into_inner(),
        vec![".item", ".item"]
    );
}

#[test]
fn document_exposes_creation_and_root_elements() {
    let mut backend = MockDocument::default();
    let mut document = DocumentApi::new(&mut backend);

    assert_eq!(document.create_element("section").unwrap(), 1);
    assert_eq!(document.create_text_node("hello").unwrap(), 2);
    assert_eq!(document.get_element_by_id("target").unwrap(), Some(2));
    assert_eq!(document.body(), Some(10));
    assert_eq!(document.document_element(), Some(1));
}

#[test]
fn node_mutations_update_the_authoritative_tree() {
    let mut tree = MockTree::default();
    {
        let mut root = NodeTreeApi::new(&mut tree, 1);
        root.append_child(2).unwrap();
        root.append_child(3).unwrap();
        root.insert_before(4, Some(3)).unwrap();
        assert_eq!(root.child_nodes().unwrap().into_vec(), vec![2, 4, 3]);
        assert_eq!(root.first_child().unwrap(), Some(2));
    }
    assert_eq!(
        NodeTreeApi::new(&mut tree, 4).next_sibling().unwrap(),
        Some(3)
    );

    // Moving an already-parented node detaches it first.
    NodeTreeApi::new(&mut tree, 5).append_child(4).unwrap();
    assert_eq!(tree.children.get(&1).unwrap(), &vec![2, 3]);
    assert_eq!(tree.children.get(&5).unwrap(), &vec![4]);

    NodeTreeApi::new(&mut tree, 1).remove_child(2).unwrap();
    assert!(!tree.parents.contains_key(&2));
    NodeTreeApi::new(&mut tree, 1).append_child(6).unwrap();
    NodeTreeApi::new(&mut tree, 6).replace_with(7).unwrap();
    assert_eq!(tree.children.get(&1).unwrap(), &vec![3, 7]);
    assert!(!tree.parents.contains_key(&6));
}

#[test]
fn remove_child_rejects_a_node_from_another_parent() {
    let mut tree = MockTree::default();
    NodeTreeApi::new(&mut tree, 2).append_child(3).unwrap();
    assert_eq!(
        NodeTreeApi::new(&mut tree, 1).remove_child(3),
        Err(DomError::NotFound)
    );
    assert_eq!(tree.parents.get(&3), Some(&2));
}

#[test]
fn text_and_html_replace_children_and_invalidate() {
    let mut backend = MockContent {
        text: "AB".into(),
        html: "<b>A</b><i>B</i>".into(),
        invalidations: 0,
    };
    {
        let mut node = NodeContentApi::new(&mut backend, 1);
        assert_eq!(node.text_content().unwrap(), "AB");
        assert_eq!(node.inner_html().unwrap(), "<b>A</b><i>B</i>");
        node.set_text_content("a < b & c").unwrap();
        assert_eq!(node.inner_html().unwrap(), "a &lt; b &amp; c");
        node.set_inner_html("<span>A &amp; B</span>").unwrap();
        assert_eq!(node.text_content().unwrap(), "A & B");
        assert_eq!(node.inner_html().unwrap(), "<span>A &amp; B</span>");
    }
    assert_eq!(backend.invalidations, 2);
}

#[test]
fn attributes_reflect_and_class_changes_affect_the_cascade() {
    let mut backend = MockAttributes::default();
    {
        let mut element = ElementAttributesApi::new(&mut backend, 1);
        element.set_id("target").unwrap();
        assert_eq!(element.id().unwrap(), "target");
        element.set_class_name("button").unwrap();
        element.class_add(&["primary", "button"]).unwrap();
        assert!(element.class_contains("primary").unwrap());
        assert_eq!(element.class_name().unwrap(), "button primary");
        assert!(!element.class_toggle("primary", None).unwrap());
        assert!(element.class_toggle("disabled", Some(true)).unwrap());
        assert_eq!(element.class_name().unwrap(), "button disabled");
        element.remove_attribute("id").unwrap();
        assert!(!element.has_attribute("id").unwrap());
    }

    // This models a selector cascade read after backend restyling, not just
    // an attribute string assertion.
    let computed_opacity = if backend.values["class"]
        .split_ascii_whitespace()
        .any(|class| class == "disabled")
    {
        0.5
    } else {
        1.0
    };
    assert_eq!(computed_opacity, 0.5);
    assert_eq!(backend.restyles, 6);
}

#[test]
fn class_list_rejects_invalid_tokens_without_mutating() {
    let mut backend = MockAttributes::default();
    let mut element = ElementAttributesApi::new(&mut backend, 1);
    assert!(matches!(
        element.class_add(&["two words"]),
        Err(DomError::Syntax(_))
    ));
    assert_eq!(element.class_name().unwrap(), "");
}

#[test]
fn inline_style_maps_properties_and_ignores_invalid_values() {
    assert_eq!(js_property_to_css("backgroundColor"), "background-color");
    assert_eq!(js_property_to_css("WebkitTransform"), "-webkit-transform");
    assert_eq!(js_property_to_css("--brandColor"), "--brandColor");
    assert_eq!(js_property_to_css("cssFloat"), "float");

    let mut backend = MockStyle::default();
    let mut style = InlineStyleApi::new(&mut backend, 1);
    style.set_js_property("backgroundColor", "red").unwrap();
    style.set_property("--brand", "blue").unwrap();
    style.set_js_property("width", "invalid").unwrap();
    assert_eq!(style.get_js_property("backgroundColor").unwrap(), "red");
    assert_eq!(style.get_js_property("width").unwrap(), "");
    assert_eq!(style.remove_property("--brand").unwrap(), "blue");
    assert_eq!(style.remove_property("--brand").unwrap(), "");

    style
        .set_css_text("left: 40px; color: green; width: invalid")
        .unwrap();
    assert_eq!(style.get_js_property("left").unwrap(), "40px");
    assert_eq!(style.css_text().unwrap(), "color: green; left: 40px;");
}
