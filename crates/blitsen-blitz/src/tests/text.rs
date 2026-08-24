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

/// Arabic exercises both halves of the complex-text stack: Unicode bidi puts
/// the run right-to-left, and HarfRust applies the joining features in the
/// author font. The fixture's four beh forms have distinct glyph IDs, so three
/// cmap hits cannot pass this assertion without shaping.
#[test]
fn an_author_font_shapes_arabic_joining_and_visual_rtl_order() {
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "World";
                              src: url("block-world.ttf") format("truetype") }
                 div { font: 48px "World"; direction: rtl; color: #000 }
               </style>
               <div id="run">ببب</div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let node = dom.get_element_by_id("run").unwrap().unwrap();
    let inline = dom
        .node(node)
        .unwrap()
        .element_data()
        .unwrap()
        .inline_layout_data
        .as_ref()
        .expect("the Arabic run was laid out");
    assert!(
        inline.layout.is_rtl(),
        "the paragraph base direction is RTL"
    );
    let runs = inline.layout.get(0).unwrap().runs().collect::<Vec<_>>();
    assert_eq!(runs.len(), 1, "one face and style make one Arabic run");
    let run = &runs[0];
    assert!(run.is_rtl(), "the Arabic shaping run is RTL");
    let glyphs = run
        .clusters()
        .flat_map(|cluster| cluster.glyphs().map(|glyph| glyph.id))
        .collect::<Vec<_>>();
    assert_eq!(glyphs.len(), 3, "three joined letters remain three glyphs");
    assert!(
        glyphs.iter().all(|glyph| *glyph != 0),
        "no letter is .notdef"
    );
    let mut forms = glyphs;
    forms.sort_unstable();
    forms.dedup();
    assert_eq!(
        forms.len(),
        3,
        "initial, medial and final forms were selected"
    );
    assert_eq!(
        run.visual_clusters()
            .map(|cluster| cluster.text_range().start)
            .collect::<Vec<_>>(),
        vec![4, 2, 0],
        "visual traversal reverses the logical UTF-8 cluster order"
    );
    assert!(inked_bounds(&render(&mut dom, 400, 120), 400).is_some());
}

/// Font fallback is per character cluster and follows the CSS family list.
/// The first fixture covers ASCII only and the second covers one CJK scalar,
/// making the selected face observable without depending on host fonts.
#[test]
fn a_cjk_cluster_falls_through_to_the_next_author_family() {
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "ASCII";
                              src: url("block-ascii.ttf") format("truetype") }
                 @font-face { font-family: "World";
                              src: url("block-world.ttf") format("truetype") }
                 div { font: 40px "ASCII", "World", sans-serif; color: #000 }
               </style>
               <div id="run">A中A</div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let node = dom.get_element_by_id("run").unwrap().unwrap();
    let inline = dom
        .node(node)
        .unwrap()
        .element_data()
        .unwrap()
        .inline_layout_data
        .as_ref()
        .expect("the mixed run was laid out");
    let runs = inline.layout.get(0).unwrap().runs().collect::<Vec<_>>();
    assert_eq!(runs.len(), 3, "ASCII, CJK fallback, then ASCII again");
    assert_eq!(runs[0].font(), runs[2].font());
    assert_ne!(runs[0].font(), runs[1].font(), "CJK uses the next family");
    assert!(
        runs.iter()
            .flat_map(|run| run.clusters())
            .flat_map(|cluster| cluster.glyphs())
            .all(|glyph| glyph.id != 0),
        "each author font covers the cluster it was selected for"
    );
    let (_, _, width, _) =
        inked_bounds(&render(&mut dom, 400, 120), 400).expect("the fallback run painted");
    assert_eq!(width, 120, "three one-em author glyphs painted at 40px");
}

/// Emoji selection is a distinct Parley cluster path. This fixture proves an
/// author-provided outline emoji is shaped and painted; it deliberately says
/// nothing about platform colour-font formats or multi-codepoint ZWJ sequences.
#[test]
fn an_author_outline_emoji_is_clustered_and_painted() {
    let mut dom = fixture_document(
        r#"<style>
                 @font-face { font-family: "World";
                              src: url("block-world.ttf") format("truetype") }
                 div { font: 64px "World", emoji; color: #000 }
               </style>
               <div id="run">🙂</div>"#,
        None,
    );
    dom.flush_layout().unwrap();
    let node = dom.get_element_by_id("run").unwrap().unwrap();
    let inline = dom
        .node(node)
        .unwrap()
        .element_data()
        .unwrap()
        .inline_layout_data
        .as_ref()
        .expect("the emoji was laid out");
    let runs = inline.layout.get(0).unwrap().runs().collect::<Vec<_>>();
    let clusters = runs
        .iter()
        .flat_map(|run| run.clusters())
        .collect::<Vec<_>>();
    assert_eq!(clusters.len(), 1);
    assert!(
        clusters[0].is_emoji(),
        "the scalar takes the emoji font path"
    );
    assert!(clusters[0].glyphs().all(|glyph| glyph.id != 0));
    assert!(inked_bounds(&render(&mut dom, 160, 120), 160).is_some());
}

/// No universal runtime fallback is hidden behind the author stack. When no
/// discovered or declared face maps a scalar, shaping keeps the selected
/// face's glyph zero and paints that face's `.notdef` outline.
#[test]
fn an_uncovered_scalar_uses_notdef_instead_of_an_invented_fallback() {
    let html = r#"<style>
                     @font-face { font-family: "World";
                                  src: url("block-world.ttf") format("truetype") }
                     div { font: 48px "World"; color: #000 }
                   </style>
                   <div id="run">MISSING</div>"#
        .replace("MISSING", "\u{0378}");
    let mut dom = fixture_document(&html, None);
    dom.flush_layout().unwrap();
    let node = dom.get_element_by_id("run").unwrap().unwrap();
    let inline = dom
        .node(node)
        .unwrap()
        .element_data()
        .unwrap()
        .inline_layout_data
        .as_ref()
        .expect("the missing scalar was laid out");
    let runs = inline.layout.get(0).unwrap().runs().collect::<Vec<_>>();
    let glyphs = runs
        .iter()
        .flat_map(|run| run.clusters())
        .flat_map(|cluster| cluster.glyphs())
        .collect::<Vec<_>>();
    assert_eq!(glyphs.len(), 1);
    assert_eq!(glyphs[0].id, 0, "the face's .notdef glyph is explicit");
    assert!(inked_bounds(&render(&mut dom, 120, 100), 120).is_some());
}
