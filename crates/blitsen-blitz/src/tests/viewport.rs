use super::*;

#[test]
fn viewport_elements_are_replaced_boxes_with_a_physical_pixel_surface() {
    let mut dom = viewport_document(
        r#"<blitsen-view id="default"></blitsen-view>
               <blitsen-view id="sized" style="width: 80px; height: 40px"><b>fallback</b></blitsen-view>"#,
        2.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let default = dom.get_element_by_id("default").unwrap().unwrap();
    let sized = dom.get_element_by_id("sized").unwrap().unwrap();
    assert_eq!(dom.native_viewports().unwrap(), vec![default, sized]);

    let unsized_surface = dom.native_viewport_surface(default, snapshot).unwrap();
    assert_eq!(
        (unsized_surface.rect.width, unsized_surface.rect.height),
        (300.0, 150.0),
        "an unsized viewport uses the default object size"
    );
    assert_eq!((unsized_surface.width, unsized_surface.height), (600, 300));
    assert_eq!(unsized_surface.device_pixel_ratio, 2.0);
    assert_eq!(unsized_surface.byte_length(), 600 * 300 * 4);

    let sized_surface = dom.native_viewport_surface(sized, snapshot).unwrap();
    assert_eq!(
        (sized_surface.rect.width, sized_surface.rect.height),
        (80.0, 40.0)
    );
    assert_eq!((sized_surface.width, sized_surface.height), (160, 80));
    assert_eq!(
        sized_surface.rect.y, 150.0,
        "a viewport is a block box that displaces the ones after it"
    );

    let body = dom.body().unwrap();
    assert_eq!(
        dom.native_viewport_surface(body, snapshot),
        Err(DomError::InvalidNodeType),
        "only viewport elements have a surface"
    );

    let created = dom.create_element(&DomName::html("blitsen-view")).unwrap();
    assert_eq!(
        dom.native_viewport_surface(created, snapshot),
        Err(DomError::InvalidNodeType),
        "a detached viewport has no box and therefore no surface"
    );
    dom.append_child(body, created).unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.native_viewport_surface(created, snapshot)
            .unwrap()
            .width,
        600,
        "a scripted viewport gets its surface at the next layout flush"
    );
}

#[test]
fn viewport_surfaces_follow_resize_and_display_density() {
    let mut dom = viewport_document(
        r#"<blitsen-view id="view" style="width: 100px; height: 50px"></blitsen-view>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let first = dom.native_viewport_surface(view, snapshot).unwrap();
    assert_eq!((first.width, first.height), (100, 50));

    dom.flush_layout().unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(
        dom.native_viewport_surface(view, snapshot).unwrap(),
        first,
        "a frame that changes nothing does not invalidate the surface"
    );

    dom.set_inline_style(view, "width", "120px").unwrap();
    let snapshot = dom.flush_layout().unwrap();
    let resized = dom.native_viewport_surface(view, snapshot).unwrap();
    assert_eq!((resized.width, resized.height), (120, 50));
    assert_eq!(resized.generation, first.generation + 1);

    let mut viewport = dom.document_ref().viewport().clone();
    viewport.set_hidpi_scale(3.0);
    dom.document_mut().set_viewport(viewport);
    let snapshot = dom.flush_layout().unwrap();
    let dense = dom.native_viewport_surface(view, snapshot).unwrap();
    assert_eq!((dense.width, dense.height), (360, 150));
    assert_eq!(dense.device_pixel_ratio, 3.0);
    assert_eq!(
        dense.rect.width, 120.0,
        "CSS geometry is density-independent"
    );
    assert_eq!(dense.generation, resized.generation + 1);
}

#[test]
fn viewport_writes_must_be_one_complete_frame() {
    let mut dom = viewport_document(
        r#"<blitsen-view id="view" style="width: 4px; height: 2px"></blitsen-view>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let surface = dom.native_viewport_surface(view, snapshot).unwrap();
    assert_eq!(surface.byte_length(), 32);

    assert_eq!(
        dom.write_native_viewport(view, &[0; 16]),
        Err(DomError::Backend(
            "<blitsen-view> surface needs 32 RGBA bytes, received 16".into()
        ))
    );
    assert!(dom.write_native_viewport(view, &[0; 32]).is_ok());

    let body = dom.body().unwrap();
    assert_eq!(
        dom.write_native_viewport(body, &[0; 32]),
        Err(DomError::InvalidNodeType)
    );

    // A resize invalidates the frame the application drew for the old size.
    dom.set_inline_style(view, "width", "8px").unwrap();
    dom.flush_layout().unwrap();
    assert!(dom.write_native_viewport(view, &[0; 32]).is_err());
    assert!(dom.write_native_viewport(view, &[0; 64]).is_ok());
}

#[test]
fn viewport_elements_hit_test_like_any_other_element() {
    let mut dom = viewport_document(
        r#"<div id="backdrop" style="position: absolute; left: 0; top: 0;
                  width: 400px; height: 300px"></div>
               <div id="host" style="position: relative">
                 <blitsen-view id="view" style="position: absolute; left: 20px; top: 10px;
                    width: 100px; height: 50px"></blitsen-view>
               </div>
               <blitsen-view id="transparent" style="position: absolute; left: 20px; top: 10px;
                  width: 100px; height: 50px; pointer-events: none"></blitsen-view>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let backdrop = dom.get_element_by_id("backdrop").unwrap().unwrap();
    let host = dom.get_element_by_id("host").unwrap().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();

    let hit = dom.hit_test(50.0, 25.0, snapshot).unwrap().unwrap();
    assert_eq!(
        hit.target, view,
        "a later viewport with pointer-events: none does not swallow the hit"
    );
    assert_eq!((hit.offset_x, hit.offset_y), (30.0, 15.0));
    assert_eq!(hit.path.last(), Some(&view));
    assert!(
        hit.path.contains(&host),
        "propagation reaches a viewport through its ordinary ancestors"
    );
    assert_eq!(
        dom.hit_test(10.0, 5.0, snapshot).unwrap().unwrap().target,
        backdrop,
        "a viewport claims no more than its own box"
    );
}

/// Clip shapes in force where the composited surface is recorded, together
/// with the paint order of the surrounding solid DOM fills.
///
/// Each clip is returned in scene coordinates so a document-space point can
/// be tested against every layer that encloses the surface.
fn composited_surface(scene: &Scene) -> (Vec<&'static str>, Vec<BezPath>) {
    let positioned = |transform: kurbo::Affine, clip: &BezPath| {
        let mut path = clip.clone();
        path.apply_affine(transform);
        // Blitz leaves clip subpaths implicitly closed; `contains` counts
        // windings over explicit segments only.
        if !matches!(path.elements().last(), Some(kurbo::PathEl::ClosePath)) {
            path.close_path();
        }
        path
    };
    let mut clips: Vec<BezPath> = Vec::new();
    let mut active: Vec<BezPath> = Vec::new();
    let mut order = Vec::new();
    for command in &scene.commands {
        match command {
            RenderCommand::PushLayer(layer) => {
                active.push(positioned(layer.transform, &layer.clip));
            }
            RenderCommand::PushClipLayer(clip) => {
                active.push(positioned(clip.transform, &clip.clip));
            }
            RenderCommand::PopLayer => {
                active.pop();
            }
            RenderCommand::Fill(fill) => match &fill.brush {
                Paint::Solid(color) => {
                    let rgba = color.to_rgba8();
                    match [rgba.r, rgba.g, rgba.b] {
                        [230, 30, 30] => order.push("below"),
                        [30, 60, 230] => order.push("above"),
                        _ => {}
                    }
                }
                Paint::Image(_) => {
                    order.push("surface");
                    clips = active.clone();
                }
                _ => {}
            },
            _ => {}
        }
    }
    (order, clips)
}

#[test]
fn viewport_contents_composite_between_dom_layers_and_inside_every_clip() {
    let mut dom = viewport_document(
        r#"<style>
                 #stage { position: relative; width: 80px; height: 300px; overflow: hidden }
                 #below { position: absolute; left: 0; top: 0; width: 200px; height: 200px;
                          background: rgb(230, 30, 30); z-index: -1 }
                 #view { position: absolute; left: 10px; top: 10px; width: 100px; height: 50px;
                         border-radius: 12px }
                 #above { position: absolute; left: 0; top: 0; width: 60px; height: 60px;
                          background: rgb(30, 60, 230); z-index: 1 }
               </style>
               <div id="stage">
                 <div id="below"></div>
                 <blitsen-view id="view"></blitsen-view>
                 <div id="above"></div>
               </div>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let surface = dom.native_viewport_surface(view, snapshot).unwrap();
    dom.write_native_viewport(view, &vec![0xff; surface.byte_length()])
        .unwrap();

    let mut scene = Scene::new();
    blitz_paint::paint_scene(&mut scene, dom.document_mut().as_mut(), 1.0, 400, 300, 0, 0);
    let (order, clips) = composited_surface(&scene);

    assert_eq!(order, ["below", "surface", "above"]);
    assert!(!clips.is_empty());
    assert!(
        clips
            .iter()
            .all(|clip| clip.contains(Point::new(50.0, 35.0))),
        "the middle of the surface survives every clip"
    );
    assert!(
        clips
            .iter()
            .any(|clip| !clip.contains(Point::new(11.0, 11.0))),
        "the element's own border-radius rounds the surface"
    );
    assert!(
        clips
            .iter()
            .any(|clip| !clip.contains(Point::new(90.0, 35.0))),
        "the ancestor scrollport clips the surface"
    );
}

/// Blob identity of every composited surface recorded into a scene.
///
/// Vello keys its image atlas on this id, so an id it has already seen is an
/// atlas hit and an id it has not is one upload of the surface.
fn surface_upload_ids(scene: &Scene) -> Vec<u64> {
    scene
        .commands
        .iter()
        .filter_map(|command| match command {
            RenderCommand::Fill(fill) => match &fill.brush {
                Paint::Image(brush) => Some(brush.image.data.id()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn viewport_costs_one_upload_per_written_frame_and_none_otherwise() {
    let mut dom = viewport_document(
        r#"<blitsen-view id="view" style="width: 40px; height: 20px"></blitsen-view>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let byte_length = dom
        .native_viewport_surface(view, snapshot)
        .unwrap()
        .byte_length();

    let paint = |dom: &mut BlitzDom| {
        let mut scene = Scene::new();
        blitz_paint::paint_scene(&mut scene, dom.document_mut().as_mut(), 1.0, 60, 40, 0, 0);
        surface_upload_ids(&scene)
    };

    assert!(
        paint(&mut dom).is_empty(),
        "a viewport the application has not drawn uploads nothing"
    );

    dom.write_native_viewport(view, &vec![0x11; byte_length])
        .unwrap();
    let first = paint(&mut dom);
    assert_eq!(
        first.len(),
        1,
        "one written frame is one composited surface"
    );

    // The application skips a frame. Blitz re-records the scene, but the
    // surface blob is the same allocation, so Vello re-uses its atlas entry
    // instead of copying the frame again.
    assert_eq!(
        paint(&mut dom),
        first,
        "a frame the application leaves alone costs no second upload"
    );

    // Even byte-identical contents are a new frame: the application said so by
    // writing, and nothing may assume its pixels are unchanged.
    dom.write_native_viewport(view, &vec![0x11; byte_length])
        .unwrap();
    let second = paint(&mut dom);
    assert_eq!(second.len(), 1);
    assert_ne!(second, first, "a written frame is uploaded once");

    // Both scene recordings hold the surface, so there is one composited image
    // and no second full-frame copy alongside it.
    assert_eq!(
        paint(&mut dom).len(),
        1,
        "the surface is composited once per frame, not blitted a second time"
    );
}

#[test]
fn viewport_pixels_reach_the_composited_frame() {
    let mut dom = viewport_document(
        r#"<blitsen-view id="view" style="width: 40px; height: 20px"></blitsen-view>"#,
        1.0,
    );
    let snapshot = dom.flush_layout().unwrap();
    let view = dom.get_element_by_id("view").unwrap().unwrap();
    let surface = dom.native_viewport_surface(view, snapshot).unwrap();
    let frame: Vec<u8> = std::iter::repeat_n([0, 200, 40, 255], surface.byte_length() / 4)
        .flatten()
        .collect();
    dom.write_native_viewport(view, &frame).unwrap();

    let pixels = anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, 60, 40, 0, 0);
        },
        60,
        40,
    );
    let pixel = |x: usize, y: usize| {
        let start = (y * 60 + x) * 4;
        [
            pixels[start],
            pixels[start + 1],
            pixels[start + 2],
            pixels[start + 3],
        ]
    };
    assert_eq!(pixel(20, 10), [0, 200, 40, 255]);
    assert_eq!(
        pixel(50, 30),
        [0, 0, 0, 0],
        "the surface does not paint outside its own box"
    );
}
