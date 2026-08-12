use super::*;

/// `<img>` end to end: fetch, decode, intrinsic sizing and paint.
///
/// The intrinsic size is what CSS resolves the unspecified dimension
/// against, so a decode that silently produced nothing would lay the
/// element out at zero height rather than fail visibly.
#[test]
fn images_decode_paint_and_report_their_intrinsic_size() {
    let mut dom = fixture_document(
        r#"<img id="swatch" src="swatch.png" style="display: block; width: 80px">"#,
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let swatch = dom.get_element_by_id("swatch").unwrap().unwrap();
    assert_eq!(
        dom.image_state(swatch, snapshot),
        Ok(ImageState::decoded(8, 4))
    );
    let rect = dom.bounding_rect(swatch, snapshot).unwrap();
    assert_eq!(
        (rect.width, rect.height),
        (80.0, 40.0),
        "the intrinsic ratio resolves the dimension CSS left out"
    );

    let pixels = render(&mut dom, 400, 200);
    assert_eq!(pixel(&pixels, 400, 20, 20), [220, 20, 20, 255]);
    assert_eq!(pixel(&pixels, 400, 60, 20), [20, 40, 220, 255]);
    assert_eq!(inked_bounds(&pixels, 400), Some((0, 0, 80, 40)));
}

/// Bundlers inline small assets, so a drop-in build's icons arrive as
/// `data:` URLs rather than files.
#[test]
fn images_decode_from_inlined_data_urls() {
    // The `swatch.png` fixture, encoded the way a bundler would inline it.
    let inlined = concat!(
        "data:image/png;base64,",
        "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAECAYAAACzzX7wAAAAF0lEQVR42mO4IyLy",
        "HxmLaNxBwQy0VwAAw8RBoVkySsgAAAAASUVORK5CYII="
    );
    let mut dom = fixture_document(
        &format!(r#"<img id="inlined" src="{inlined}" style="display: block; width: 80px">"#),
        None,
    );
    let snapshot = dom.flush_layout().unwrap();
    let inlined = dom.get_element_by_id("inlined").unwrap().unwrap();
    assert_eq!(
        dom.image_state(inlined, snapshot),
        Ok(ImageState::decoded(8, 4))
    );
    assert_eq!(
        inked_bounds(&render(&mut dom, 400, 200), 400),
        Some((0, 0, 80, 40))
    );
}

/// A `background-image` is only discovered once style resolves, which is
/// after the pass that would have applied it. It still has to be in the
/// frame that asked for it, or every backdrop flashes empty for one frame.
#[test]
fn background_images_paint_in_the_frame_that_asks_for_them() {
    let mut dom = fixture_document(
        r#"<div style="width: 80px; height: 40px; background-image: url('swatch.png');
                 background-size: 80px 40px"></div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    assert_eq!(pixel(&pixels, 400, 20, 20), [220, 20, 20, 255]);
    assert_eq!(pixel(&pixels, 400, 60, 20), [20, 40, 220, 255]);
}

/// An image that will never arrive must not be reported as still arriving:
/// `complete` is what a script polls, and a stuck `false` never resolves.
#[test]
fn a_failed_image_is_complete_and_errored_rather_than_loading_forever() {
    let dom = fixture_document(
        r#"<img id="missing" src="does-not-exist.png">
               <img id="remote" src="https://example.com/logo.png">
               <img id="undecodable" src="data:image/png;base64,bm90IGEgcG5n">
               <img id="sourceless">
               <p id="paragraph">not an image</p>"#,
        None,
    );
    let mut dom = dom;
    let snapshot = dom.flush_layout().unwrap();
    let state = |id: &str| dom.image_state(dom.get_element_by_id(id).unwrap().unwrap(), snapshot);
    assert_eq!(state("missing"), Ok(ImageState::FAILED));
    assert_eq!(
        state("remote"),
        Ok(ImageState::FAILED),
        "a refused remote fetch is an error, not an unfinished one"
    );
    assert_eq!(
        state("undecodable"),
        Ok(ImageState::FAILED),
        "bytes that arrived but did not decode are an error too"
    );
    assert_eq!(
        state("sourceless"),
        Ok(ImageState::IDLE),
        "an image with nothing to load is already complete"
    );
    assert_eq!(state("paragraph"), Err(DomError::InvalidNodeType));
}

/// The state a script actually observes on a cold window: in flight first,
/// decoded afterwards, with the frame that decodes it also painting it.
#[test]
fn an_image_still_in_flight_is_not_complete() {
    let network = DeferredResources::default();
    let mut dom = fixture_document(
        r#"<img id="swatch" src="swatch.png" style="display: block; width: 80px">"#,
        Some(Arc::new(network.clone())),
    );
    let snapshot = dom.flush_layout().unwrap();
    let swatch = dom.get_element_by_id("swatch").unwrap().unwrap();
    assert_eq!(dom.image_state(swatch, snapshot), Ok(ImageState::LOADING));
    assert_eq!(inked_bounds(&render(&mut dom, 400, 200), 400), None);

    network.deliver();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.image_state(swatch, snapshot),
        Ok(ImageState::decoded(8, 4))
    );
    assert_eq!(
        inked_bounds(&render(&mut dom, 400, 200), 400),
        Some((0, 0, 80, 40))
    );
}

/// `window.stop()` at the renderer: an image the document is still waiting
/// on has to reach a settled state, or it blocks the frame forever and the
/// script that asked to stop loading is worse off than before it asked.
#[test]
fn stopping_settles_a_subresource_that_is_still_in_flight() {
    let network = DeferredResources::default();
    let mut dom = fixture_document(
        r#"<img id="swatch" src="swatch.png" style="display: block; width: 80px">"#,
        Some(Arc::new(network.clone())),
    );
    let snapshot = dom.flush_layout().unwrap();
    let swatch = dom.get_element_by_id("swatch").unwrap().unwrap();
    assert_eq!(dom.image_state(swatch, snapshot), Ok(ImageState::LOADING));

    assert_eq!(dom.stop_loading(), 1);
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.image_state(swatch, snapshot),
        Ok(ImageState::FAILED),
        "a stopped image is complete and errored, not loading forever"
    );

    network.deliver();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.image_state(swatch, snapshot),
        Ok(ImageState::FAILED),
        "bytes that were already on the wire do not undo the stop"
    );
    assert_eq!(
        dom.stop_loading(),
        0,
        "stopping a document with nothing in flight aborts nothing"
    );
}

/// The `new Image()` path: an element built by script, given a source and
/// then connected, has to load exactly like a parsed one.
#[test]
fn a_scripted_image_loads_when_its_source_is_set() {
    let mut dom = fixture_document(r#"<div id="host"></div>"#, None);
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    let image = dom.create_element(&DomName::html("img")).unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.image_state(image, snapshot),
        Ok(ImageState::IDLE),
        "a detached image with no source has nothing to wait for"
    );

    dom.set_attribute(image, &DomName::attribute("src"), "swatch.png")
        .unwrap();
    dom.append_child(host, image).unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.image_state(image, snapshot),
        Ok(ImageState::decoded(8, 4))
    );

    dom.set_attribute(image, &DomName::attribute("alt"), "swatch")
        .unwrap();
    assert_eq!(
        dom.image_state(image, snapshot),
        Err(DomError::LayoutNotFlushed),
        "decode state is applied while layout resolves, so it is snapshot gated"
    );
}
