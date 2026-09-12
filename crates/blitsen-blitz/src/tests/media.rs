use blitsen_dom::MediaPreferences;

use super::*;

/// Serves the fixtures the way the synchronous provider does, and remembers
/// what was asked for.
#[derive(Default)]
struct CountingResources(Arc<Mutex<Vec<String>>>);

impl NetProvider for CountingResources {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        self.0.lock().unwrap().push(request.url.to_string());
        LocalResources.fetch(doc_id, request, handler);
    }
}

impl CountingResources {
    fn fetched(&self, name: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.ends_with(name))
            .count()
    }
}

/// Both preferences reach the cascade through one setter, and `matchMedia`
/// evaluates each feature from the same state the cascade does.
#[test]
fn the_cascade_and_match_media_follow_the_same_preferences() {
    let mut dom = fixture_document(
        r#"<style>
             #scheme, #motion, #still { color: rgb(1, 1, 1) }
             @media (prefers-color-scheme: dark) { #scheme { color: rgb(2, 2, 2) } }
             @media (prefers-reduced-motion: reduce) { #motion { color: rgb(2, 2, 2) } }
             @media (prefers-reduced-motion: no-preference) { #still { color: rgb(3, 3, 3) } }
           </style>
           <div id="scheme"></div><div id="motion"></div><div id="still"></div>"#,
        None,
    );
    assert_eq!(dom.media_preferences(), MediaPreferences::default());
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "scheme", "color"), "rgb(1, 1, 1)");
    assert_eq!(resolved(&dom, snapshot, "motion", "color"), "rgb(1, 1, 1)");
    assert_eq!(resolved(&dom, snapshot, "still", "color"), "rgb(3, 3, 3)");
    for (query, matches) in [
        ("(prefers-color-scheme: light)", true),
        ("(prefers-color-scheme: dark)", false),
        ("(prefers-reduced-motion: reduce)", false),
        ("(prefers-reduced-motion: no-preference)", true),
        ("(prefers-reduced-motion)", false),
        (
            "screen and (prefers-reduced-motion: reduce) and (min-width: 1px)",
            false,
        ),
    ] {
        let evaluated = dom.media_query(query).unwrap();
        assert_eq!(evaluated.matches, matches, "{query}");
        assert_eq!(evaluated.media, query, "serialized as written");
    }

    dom.set_media_preferences(MediaPreferences {
        color_scheme: blitsen_dom::ColorScheme::Dark,
        reduced_motion: true,
    })
    .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "scheme", "color"), "rgb(2, 2, 2)");
    assert_eq!(resolved(&dom, snapshot, "motion", "color"), "rgb(2, 2, 2)");
    assert_eq!(resolved(&dom, snapshot, "still", "color"), "rgb(1, 1, 1)");
    for (query, matches) in [
        ("(prefers-color-scheme: light)", false),
        ("(prefers-color-scheme: dark)", true),
        ("(prefers-reduced-motion: reduce)", true),
        ("(prefers-reduced-motion: no-preference)", false),
        ("(prefers-reduced-motion)", true),
        (
            "screen and (prefers-reduced-motion: reduce) and (min-width: 1px)",
            true,
        ),
        ("not all and (prefers-reduced-motion: reduce)", false),
    ] {
        let evaluated = dom.media_query(query).unwrap();
        assert_eq!(evaluated.matches, matches, "{query}");
        assert_eq!(evaluated.media, query, "serialized as written");
    }

    // Back again, so the flip is a flip rather than a one-way rewrite.
    dom.set_media_preferences(MediaPreferences::default())
        .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "scheme", "color"), "rgb(1, 1, 1)");
    assert_eq!(resolved(&dom, snapshot, "motion", "color"), "rgb(1, 1, 1)");
    assert_eq!(resolved(&dom, snapshot, "still", "color"), "rgb(3, 3, 3)");
    assert!(
        !dom.media_query("(prefers-reduced-motion: reduce)")
            .unwrap()
            .matches
    );
}

/// A value the feature does not define stays the cascade's to refuse, and an
/// unknown feature still does not match. The engine keeps the text of an
/// expression it cannot evaluate, as the specification's `<general-enclosed>`
/// asks, so the query serializes as written; only an unparsable one is `not
/// all`.
#[test]
fn an_unknown_feature_or_value_still_does_not_match() {
    let mut dom = backend();
    dom.set_media_preferences(MediaPreferences {
        color_scheme: blitsen_dom::ColorScheme::Light,
        reduced_motion: true,
    })
    .unwrap();
    for query in [
        "(prefers-reduced-motion: sometimes)",
        "(prefers-contrast: more)",
        "(forced-colors: active)",
    ] {
        let evaluated = dom.media_query(query).unwrap();
        assert!(!evaluated.matches, "{query}");
        assert_eq!(evaluated.media, query);
    }
    assert_eq!(dom.media_query("!!!").unwrap().media, "not all");
}

/// A sheet written by a script after load is normalised the same way one the
/// parser saw is, and follows a later change.
#[test]
fn a_scripted_sheet_follows_the_preference() {
    let mut dom = backend();
    let style = dom.create_element(&DomName::html("style")).unwrap();
    dom.set_text_content(
        style,
        "@media (prefers-reduced-motion: reduce) { #x { color: rgb(7, 7, 7) } }",
    )
    .unwrap();
    let head = dom.query_selector(dom.document(), "head").unwrap().unwrap();
    dom.append_child(head, style).unwrap();
    dom.set_text_content(dom.get_element_by_id("host").unwrap().unwrap(), "")
        .unwrap();
    let body = dom.query_selector(dom.document(), "body").unwrap().unwrap();
    let x = dom.create_element(&DomName::html("div")).unwrap();
    dom.set_attribute(x, &DomName::attribute("id"), "x")
        .unwrap();
    dom.append_child(body, x).unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "x", "color"), "rgb(0, 0, 0)");
    dom.set_media_preferences(MediaPreferences {
        color_scheme: blitsen_dom::ColorScheme::Light,
        reduced_motion: true,
    })
    .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "x", "color"), "rgb(7, 7, 7)");
}

/// A linked sheet is bytes the loader handed to the cascade, so a change has
/// to fetch it again — and only the sheets that carried the feature.
#[test]
fn a_linked_sheet_is_reloaded_when_the_preference_changes() {
    let network = Arc::new(CountingResources::default());
    let mut dom = fixture_document(
        r#"<link rel="stylesheet" href="motion.css">
           <link rel="stylesheet" href="linked.css">
           <div id="linked"></div>"#,
        Some(Arc::clone(&network) as Arc<dyn NetProvider>),
    );
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "linked", "width"), "10px");
    let fetched = |_: &BlitzDom, name: &str| network.fetched(name);
    let motion_loads = fetched(&dom, "motion.css");
    let linked_loads = fetched(&dom, "linked.css");
    dom.set_media_preferences(MediaPreferences {
        color_scheme: blitsen_dom::ColorScheme::Light,
        reduced_motion: true,
    })
    .unwrap();
    let snapshot = dom.flush_layout().unwrap();
    assert_eq!(resolved(&dom, snapshot, "linked", "width"), "20px");
    assert_eq!(
        fetched(&dom, "motion.css"),
        motion_loads + 1,
        "the sheet with the feature"
    );
    assert_eq!(
        fetched(&dom, "linked.css"),
        linked_loads,
        "and not the one without"
    );
}
