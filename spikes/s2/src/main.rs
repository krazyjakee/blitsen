use blitz_dom::{DocumentConfig, NodeId, qual_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

#[derive(Debug, PartialEq)]
struct Trace {
    initial: Geometry,
    after_class: Geometry,
    after_attribute: Geometry,
    after_inline_style: Geometry,
    after_insert: Geometry,
    after_remove: Geometry,
    stale_handle_rejected: bool,
    survivor_handle_preserved: bool,
    id_index_consistent: bool,
}

#[derive(Debug, PartialEq)]
struct Geometry {
    target_width: f32,
    target_height: f32,
    target_y: f32,
    tail_y: f32,
}

fn geometry(document: &HtmlDocument, target: NodeId, tail: NodeId) -> Geometry {
    let target_layout = document.get_node(target).unwrap().final_layout();
    let tail_layout = document.get_node(tail).unwrap().final_layout();
    Geometry {
        target_width: target_layout.size.width,
        target_height: target_layout.size.height,
        target_y: target_layout.location.y,
        tail_y: tail_layout.location.y,
    }
}

fn run(incremental: bool) -> Trace {
    let html = r#"
        <style>
            html, body { margin: 0; }
            #host { width: 400px; }
            .item { display: block; width: 100px; height: 20px; }
            .item.wide { width: 240px; }
            .item[title] { height: 60px; }
        </style>
        <div id="host">
            <div id="target" class="item"></div><div id="tail" class="item"></div>
        </div>
    "#;
    let mut document = HtmlDocument::from_html(
        html,
        DocumentConfig {
            incremental: Some(incremental),
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    document.resolve(0.0);

    let host = document.get_element_by_id("host").unwrap();
    let target = document.get_element_by_id("target").unwrap();
    let tail = document.get_element_by_id("tail").unwrap();
    let initial = geometry(&document, target, tail);

    {
        let mut mutation = document.mutate();
        mutation.set_attribute(target, qual_name!("class"), "item wide");
    }
    document.resolve(0.0);
    let after_class = geometry(&document, target, tail);

    {
        let mut mutation = document.mutate();
        mutation.set_attribute(target, qual_name!("title"), "tall");
    }
    document.resolve(0.0);
    let after_attribute = geometry(&document, target, tail);

    {
        let mut mutation = document.mutate();
        mutation.set_style_property(target, "height", "80px");
    }
    document.resolve(0.0);
    let after_inline_style = geometry(&document, target, tail);

    let inserted = {
        let mut mutation = document.mutate();
        let inserted = mutation.create_element(qual_name!("div"), vec![]);
        mutation.set_attribute(inserted, qual_name!("class"), "item");
        mutation.set_style_property(inserted, "height", "30px");
        mutation.insert_nodes_before(target, &[inserted]);
        inserted
    };
    document.resolve(0.0);
    let after_insert = geometry(&document, target, tail);

    let (stale_handle_rejected, survivor_handle_preserved) = {
        let mut mutation = document.mutate();
        mutation.remove_and_drop_node(inserted).unwrap();
        let replacement = mutation.create_element(qual_name!("aside"), vec![]);
        mutation.append_children(host, &[replacement]);
        (
            mutation.doc.get_node(inserted).is_none() && replacement != inserted,
            mutation.doc.get_node(target).is_some(),
        )
    };
    document.resolve(0.0);
    let after_remove = geometry(&document, target, tail);

    {
        let mut mutation = document.mutate();
        mutation.set_attribute(target, qual_name!("id"), "renamed");
    }
    let id_index_consistent = document.get_element_by_id("renamed") == Some(target)
        && document.get_element_by_id("target").is_none();

    Trace {
        initial,
        after_class,
        after_attribute,
        after_inline_style,
        after_insert,
        after_remove,
        stale_handle_rejected,
        survivor_handle_preserved,
        id_index_consistent,
    }
}

fn main() {
    let incremental = run(true);
    let full = run(false);

    assert_eq!(incremental.initial.target_width, 100.0);
    assert_eq!(incremental.initial.target_height, 20.0);
    assert_eq!(incremental.after_class.target_width, 240.0);
    assert_eq!(incremental.after_attribute.target_height, 60.0);
    assert_eq!(incremental.after_inline_style.target_height, 80.0);
    assert_eq!(incremental.after_inline_style.tail_y, 80.0);
    assert_eq!(incremental.after_insert.target_y, 30.0);
    assert_eq!(incremental.after_insert.tail_y, 110.0);
    assert_eq!(incremental.after_remove.target_y, 0.0);
    assert_eq!(incremental.after_remove.tail_y, 80.0);
    assert!(incremental.stale_handle_rejected);
    assert!(incremental.survivor_handle_preserved);
    assert_eq!(incremental, full, "incremental and full layout diverged");

    println!("{incremental:#?}");
}
