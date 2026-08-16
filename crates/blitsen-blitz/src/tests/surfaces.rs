use std::rc::Rc;

use super::*;

#[test]
fn replacement_elements_attach_before_stale_surface_states_are_swept() {
    let mut dom = viewport_document(
        r#"<canvas id="old-canvas"></canvas>
           <blitsen-view id="old-view"></blitsen-view>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let old_canvas = dom.get_element_by_id("old-canvas").unwrap().unwrap();
    let old_view = dom.get_element_by_id("old-view").unwrap().unwrap();
    let canvas = dom.create_element(&DomName::html("canvas")).unwrap();
    dom.set_attribute(canvas, &DomName::attribute("width"), "42")
        .unwrap();
    let view = dom.create_element(&DomName::html("blitsen-view")).unwrap();

    dom.replace(old_canvas, canvas).unwrap();
    dom.replace(old_view, view).unwrap();
    assert!(dom.canvases.contains_key(&old_canvas));
    assert!(dom.native_viewports.contains_key(&old_view));
    dom.flush_layout().unwrap();

    assert!(!dom.canvases.contains_key(&old_canvas));
    assert!(!dom.native_viewports.contains_key(&old_view));
    assert_eq!(dom.canvases[&canvas].borrow().size(), (42, 150));
    assert!(dom.native_viewports.contains_key(&view));
}

#[test]
fn detached_surface_states_survive_reparenting_until_the_nodes_are_stale() {
    let mut dom = viewport_document(
        r#"<canvas id="canvas"></canvas><blitsen-view id="view"></blitsen-view>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let body = dom.body().unwrap();
    let canvas = dom.get_element_by_id("canvas").unwrap().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let canvas_state = Rc::clone(&dom.canvases[&canvas]);
    let viewport_state = Rc::clone(&dom.native_viewports[&view]);
    dom.retain_for_js(canvas).unwrap();
    dom.retain_for_js(view).unwrap();

    dom.remove(canvas).unwrap();
    dom.remove(view).unwrap();
    dom.flush_layout().unwrap();
    assert!(Rc::ptr_eq(&canvas_state, &dom.canvases[&canvas]));
    assert!(Rc::ptr_eq(&viewport_state, &dom.native_viewports[&view]));

    dom.append_child(body, canvas).unwrap();
    dom.append_child(body, view).unwrap();
    dom.flush_layout().unwrap();
    assert!(Rc::ptr_eq(&canvas_state, &dom.canvases[&canvas]));
    assert!(Rc::ptr_eq(&viewport_state, &dom.native_viewports[&view]));

    dom.remove(canvas).unwrap();
    dom.remove(view).unwrap();
    assert!(dom.release_from_js(canvas).unwrap());
    assert!(dom.release_from_js(view).unwrap());
    dom.flush_layout().unwrap();
    assert!(!dom.canvases.contains_key(&canvas));
    assert!(!dom.native_viewports.contains_key(&view));
}
