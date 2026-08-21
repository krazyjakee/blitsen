use super::*;

/// Guards the `system-fonts` feature on `blitz-dom`.
///
/// Without it Parley has no font sources, every glyph paints nothing, and the
/// failure is invisible to any assertion that reads the DOM instead of the
/// frame. That is exactly how it went unnoticed until a demo was recorded.
#[test]
fn text_paints_glyphs_rather_than_nothing() {
    let mut dom = viewport_document(
        r#"<div style="font: 48px sans-serif; color: #000">HELLO 12345</div>"#,
        1.0,
    );
    dom.flush_layout().unwrap();
    let pixels = anyrender::render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, 300, 80, 0, 0);
        },
        300,
        80,
    );
    let inked = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 0)
        .count();
    assert!(
        inked > 200,
        "text rendered {inked} non-transparent pixels; system fonts are not loaded"
    );
}

/// Guards `@font-face` end to end: fetch, WOFF2 decompression, registration
/// under the CSS family name, and shaping with the registered face.
///
/// Real framework output almost always ships a web font, so a build that
/// quietly fell back to the system UI font would not look like itself. Every
/// letter in the fixture is a solid em block, which no fallback paints, so
/// the frame says which font was used rather than merely that one was.
#[test]
fn web_fonts_load_from_woff2_and_replace_the_fallback() {
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.woff2") format("woff2") }
                 div { font: 50px "Block", sans-serif; color: #000 }
               </style>
               <div>AAAA</div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    let (x, y, width, height) = inked_bounds(&pixels, 400).expect("the run painted nothing");
    assert_eq!(
        (x, width, height),
        (0, 200, 50),
        "four 50px em blocks, so the web font shaped and drew the run"
    );
    assert!(
        (y..y + height)
            .flat_map(|row| (x..x + width).map(move |column| (column, row)))
            .all(|(column, row)| pixel(&pixels, 400, column, row) == [0, 0, 0, 255]),
        "the block glyph is solid, so nothing else contributed to the run"
    );
}

/// Faces of one family are told apart by `@font-face` descriptor, not by
/// the metadata inside the font file.
///
/// The three fixtures are internally indistinguishable — same family name,
/// same "Regular" style, same weight 400, none of them the family the CSS
/// declares — so only the descriptors can pick one. They differ only in
/// block height, which turns a wrong match into a wrong painted height.
///
/// Also covers an uncompressed `truetype` source alongside the WOFF2 above.
#[test]
fn font_face_descriptors_select_the_face_within_a_family() {
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.ttf") format("truetype") }
                 @font-face { font-family: "Block"; font-weight: 700;
                              src: url("block-bold.ttf") format("truetype") }
                 @font-face { font-family: "Block"; font-style: italic;
                              src: url("block-italic.ttf") format("truetype") }
                 div { position: absolute; left: 0; font: 50px "Block"; color: #000 }
                 #bold { top: 60px; font-weight: bold }
                 #italic { top: 120px; font-style: italic }
               </style>
               <div id="regular">A</div>
               <div id="bold">A</div>
               <div id="italic">A</div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let pixels = render(&mut dom, 400, 200);
    let band = |top: usize, bottom: usize| {
        inked_bounds(&pixels[top * 400 * 4..bottom * 400 * 4], 400).expect("a run painted nothing")
    };
    assert_eq!(band(0, 60).3, 50, "the 400 face fills the em box");
    assert_eq!(band(60, 120).3, 25, "the 700 face fills half of it");
    assert_eq!(
        band(120, 200).3,
        13,
        "the italic face fills a quarter of it"
    );
    assert_eq!(band(0, 60).2, band(60, 120).2, "every face advances one em");
}

/// Nothing registers a font as a critical resource, so a document paints
/// while its web fonts are still in flight: Blitsen is FOUT, never FOIT.
///
/// The alternative — withholding the frame until the font arrives — would
/// trade a restyle for a blank window on every cold start.
#[test]
fn text_paints_in_the_fallback_while_a_web_font_is_still_loading() {
    let network = DeferredResources::default();
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "Block"; src: url("block-regular.woff2") format("woff2") }
                 div { font: 50px "Block", sans-serif; color: #000 }
               </style>
               <div>AAAA</div>"#,
        Some(Arc::new(network.clone())),
    );
    dom.flush_layout().unwrap();
    let waiting = render(&mut dom, 400, 200);
    let (_, _, _, fallback_height) =
        inked_bounds(&waiting, 400).expect("no text painted while the web font loaded");
    assert_ne!(
        fallback_height, 50,
        "the fallback face painted, not the block glyph"
    );

    network.deliver();
    dom.flush_layout().unwrap();
    let loaded = render(&mut dom, 400, 200);
    assert_eq!(
        inked_bounds(&loaded, 400).map(|bounds| (bounds.2, bounds.3)),
        Some((200, 50)),
        "the arriving font reshapes the already-painted run"
    );
}
