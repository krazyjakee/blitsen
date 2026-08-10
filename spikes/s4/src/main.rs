use std::sync::Arc;

use blitz_dom::{DocumentConfig, NodeId};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport};

#[derive(Debug, PartialEq)]
struct Trace {
    replacement_width: f32,
    replacement_height: f32,
    following_y: f32,
    host_height: f32,
    old_handle_rejected: bool,
    ids_adopted: bool,
    table_context_correct: bool,
    select_context_correct: bool,
    live_counts_after_replacements: Vec<usize>,
}

fn tag_is(document: &HtmlDocument, node_id: NodeId, expected: &str) -> bool {
    document
        .get_node(node_id)
        .and_then(|node| node.element_data())
        .is_some_and(|element| element.name.local.as_ref() == expected)
}

fn run(incremental: bool) -> Trace {
    let mut document = HtmlDocument::from_html(
        r#"
            <style>
                html, body { margin: 0; }
                #host { width: 400px; }
                #host > span { display: block; width: 80px; height: 20px; }
                #host > .wide { width: 240px; height: 30px; }
            </style>
            <div id="host"><i id="old">old</i></div>
            <table><tbody id="rows"></tbody></table>
            <select id="choice"></select>
        "#,
        DocumentConfig {
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            incremental: Some(incremental),
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    document.resolve(0.0);

    let host = document.get_element_by_id("host").unwrap();
    let rows = document.get_element_by_id("rows").unwrap();
    let choice = document.get_element_by_id("choice").unwrap();
    let old = document.get_element_by_id("old").unwrap();

    {
        let mut mutation = document.mutate();
        mutation.set_inner_html(
            host,
            r#"<span id="replacement" class="wide">A</span><span id="following">B</span>"#,
        );
        mutation.set_inner_html(rows, r#"<tr id="row"><td id="cell">cell</td></tr>"#);
        mutation.set_inner_html(choice, r#"<option id="option" value="1">one</option>"#);
    }
    document.resolve(0.0);

    let replacement = document.get_element_by_id("replacement").unwrap();
    let following = document.get_element_by_id("following").unwrap();
    let row = document.get_element_by_id("row").unwrap();
    let cell = document.get_element_by_id("cell").unwrap();
    let option = document.get_element_by_id("option").unwrap();

    let replacement_layout = *document.get_node(replacement).unwrap().final_layout();
    let following_layout = *document.get_node(following).unwrap().final_layout();
    let host_layout = *document.get_node(host).unwrap().final_layout();

    let table_context_correct = tag_is(&document, row, "tr")
        && tag_is(&document, cell, "td")
        && document.get_node(row).unwrap().parent == Some(rows)
        && document.get_node(cell).unwrap().parent == Some(row);
    let select_context_correct = tag_is(&document, option, "option")
        && document.get_node(option).unwrap().parent == Some(choice);
    let old_handle_rejected = document.get_node(old).is_none();
    let ids_adopted = document.get_element_by_id("old").is_none()
        && document.get_element_by_id("replacement") == Some(replacement);

    let mut live_counts_after_replacements = vec![document.tree().len()];
    for iteration in 0..5 {
        let html = format!("<span class=\"wide\">replacement {iteration}</span><span>tail</span>");
        {
            let mut mutation = document.mutate();
            mutation.set_inner_html(host, &html);
        }
        document.resolve(0.0);
        live_counts_after_replacements.push(document.tree().len());
    }

    Trace {
        replacement_width: replacement_layout.size.width,
        replacement_height: replacement_layout.size.height,
        following_y: following_layout.location.y,
        host_height: host_layout.size.height,
        old_handle_rejected,
        ids_adopted,
        table_context_correct,
        select_context_correct,
        live_counts_after_replacements,
    }
}

fn main() {
    let incremental = run(true);
    let full = run(false);

    assert_eq!(incremental.replacement_width, 240.0);
    assert_eq!(incremental.replacement_height, 30.0);
    assert_eq!(incremental.following_y, 30.0);
    assert_eq!(incremental.host_height, 50.0);
    assert!(incremental.old_handle_rejected);
    assert!(incremental.ids_adopted);
    assert!(incremental.table_context_correct);
    assert!(incremental.select_context_correct);
    assert_eq!(incremental, full, "incremental and full layout diverged");

    println!("{incremental:#?}");
}
