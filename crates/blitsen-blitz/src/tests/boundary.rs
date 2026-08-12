use super::*;

#[test]
fn implements_the_complete_boundary_over_one_blitz_tree() {
    let mut dom = backend();
    let document = dom.document();
    let html = dom.document_element().unwrap();
    let body = dom.body().unwrap();
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    let old = dom.get_element_by_id("x").unwrap().unwrap();
    assert_eq!(dom.node_kind(document), Ok(NodeKind::Document));
    assert_eq!(dom.element_name(html).unwrap().local, "html");
    assert!(dom.is_connected(old).unwrap());

    let replacement = dom.create_element(&DomName::html("section")).unwrap();
    dom.set_attribute(replacement, &DomName::attribute("id"), "replacement")
        .unwrap();
    dom.set_text_content(replacement, "hello").unwrap();
    dom.insert_before(host, replacement, Some(old)).unwrap();
    assert_eq!(dom.previous_sibling(old).unwrap(), Some(replacement));
    assert_eq!(dom.next_sibling(replacement).unwrap(), Some(old));
    assert_eq!(dom.parent(replacement).unwrap(), Some(host));

    assert!(dom.set_inline_style(replacement, "width", "120px").unwrap());
    assert!(
        !dom.set_inline_style(replacement, "width", "invalid")
            .unwrap()
    );
    assert_eq!(
        dom.inline_style(replacement, "width").unwrap().as_deref(),
        Some("120px")
    );
    dom.set_inner_html(replacement, "<b>A</b><i>B</i>").unwrap();
    assert_eq!(dom.text_content(replacement).unwrap(), "AB");
    assert!(dom.inner_html(replacement).unwrap().contains("<b>"));
    assert_eq!(
        dom.remove_inline_style(replacement, "width")
            .unwrap()
            .as_deref(),
        Some("120px")
    );

    let snapshot = dom.flush_layout().unwrap();
    assert!(dom.bounding_rect(replacement, snapshot).unwrap().width > 0.0);
    assert!(dom.hit_test(1.0, 1.0, snapshot).unwrap().is_some());
    dom.set_attribute(replacement, &DomName::attribute("class"), "wide")
        .unwrap();
    assert_eq!(
        dom.bounding_rect(replacement, snapshot),
        Err(DomError::LayoutNotFlushed)
    );
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.bounding_rect(replacement, snapshot).unwrap().width,
        240.0
    );
    let (metrics, full_document) = dom.last_frame_invalidation();
    assert!(metrics.restyled_nodes > 0);
    assert!(metrics.relaid_out_nodes >= metrics.restyled_nodes);
    assert!(!full_document);
    dom.flush_layout().unwrap();
    assert_eq!(
        dom.last_frame_invalidation(),
        (blitsen_dom::InvalidationMetrics::default(), false)
    );

    dom.append_child(body, replacement).unwrap();
    assert_eq!(dom.parent(replacement).unwrap(), Some(body));
}

#[test]
fn detached_nodes_follow_javascript_wrapper_lifetime() {
    let mut dom = backend();
    let node = dom.get_element_by_id("x").unwrap().unwrap();
    dom.retain_for_js(node).unwrap();
    dom.remove(node).unwrap();
    assert_eq!(dom.text_content(node).unwrap(), "old");
    assert!(!dom.is_connected(node).unwrap());
    assert!(dom.release_from_js(node).unwrap());
    assert_eq!(dom.text_content(node), Err(DomError::StaleNode));
}

#[test]
fn fragment_parsing_adopts_real_contextual_nodes() {
    let mut dom = backend();
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    let nodes = dom
        .parse_fragment(host, "<span id=one>one</span><span>two</span>")
        .unwrap();
    assert_eq!(nodes.len(), 2);
    dom.append_child(host, nodes[0]).unwrap();
    assert_eq!(dom.get_element_by_id("one").unwrap(), Some(nodes[0]));
}

#[test]
fn reports_the_real_full_document_fallback_mode() {
    let mut dom = BlitzDom::from_html(
        "<body><main id='host'><p>child</p></main></body>",
        DocumentConfig {
            incremental: Some(false),
            ..Default::default()
        },
    );
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    dom.set_attribute(host, &DomName::attribute("class"), "changed")
        .unwrap();
    dom.flush_layout().unwrap();
    let (metrics, full_document) = dom.last_frame_invalidation();
    assert!(full_document);
    assert_eq!(metrics.restyled_nodes, dom.document_ref().tree().len());
    assert_eq!(metrics.relaid_out_nodes, metrics.restyled_nodes);
}

#[test]
fn hit_testing_returns_paint_order_transforms_clipping_and_the_dom_path() {
    let mut dom = BlitzDom::from_html(
        r#"
            <style>
              html, body { margin: 0; width: 400px; height: 300px }
              .box { position: absolute; width: 100px; height: 100px }
              #low { left: 0; top: 0 }
              #high { left: 20px; top: 20px; z-index: 2 }
              #high-child { width: 100%; height: 100% }
              #transparent { left: 20px; top: 20px; z-index: 3; pointer-events: none }
              #transformed { left: 150px; top: 0; transform: translateX(40px) }
              #clip { left: 0; top: 150px; width: 40px; height: 40px; overflow: hidden }
              #outside { position: absolute; left: 60px; top: 0; width: 20px; height: 20px }
              #nested-low { left: 250px; top: 150px; z-index: 1 }
              #nested-child { width: 100%; height: 100%; position: relative; z-index: 100 }
              #nested-high { left: 250px; top: 150px; z-index: 2 }
            </style>
            <body>
              <div id="low" class="box"></div>
              <div id="high" class="box"><div id="high-child"></div></div>
              <div id="transparent" class="box"></div>
              <div id="transformed" class="box"></div>
              <div id="clip" class="box"><div id="outside"></div></div>
              <div id="nested-low" class="box"><div id="nested-child"></div></div>
              <div id="nested-high" class="box"></div>
            </body>
            "#,
        DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    let snapshot = dom.flush_layout().unwrap();
    let document = dom.document();
    let body = dom.body().unwrap();
    let high = dom.get_element_by_id("high-child").unwrap().unwrap();
    let transformed = dom.get_element_by_id("transformed").unwrap().unwrap();

    let overlap = dom.hit_test(30.0, 30.0, snapshot).unwrap().unwrap();
    assert_eq!(overlap.target, high);
    assert_eq!(overlap.path.first(), Some(&document));
    assert_eq!(overlap.path.last(), Some(&high));

    let transformed_hit = dom.hit_test(195.0, 10.0, snapshot).unwrap().unwrap();
    assert_eq!(transformed_hit.target, transformed);

    let clipped = dom.hit_test(65.0, 160.0, snapshot).unwrap().unwrap();
    assert_eq!(clipped.target, body);
    assert_eq!(
        clipped.path,
        vec![document, dom.document_element().unwrap(), body]
    );

    let nested_high = dom.get_element_by_id("nested-high").unwrap().unwrap();
    assert_eq!(
        dom.hit_test(260.0, 160.0, snapshot)
            .unwrap()
            .unwrap()
            .target,
        nested_high,
        "a child cannot escape its ancestor's lower stacking context"
    );
}
