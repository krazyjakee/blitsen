//! Local asset validation for an exported application.

use std::path::Path;

use blitsen_blitz::BlitzDom;
use blitsen_dom::DomBackend;
use blitsen_js::JsError;

/// Refuses a document whose subresources are remote or escape the application
/// directory, before anything is rendered from it.
pub fn validate_local_assets(
    document: &BlitzDom,
    root: &Path,
    entrypoint: &Path,
) -> Result<(), JsError> {
    let root = root.canonicalize().map_err(|error| {
        JsError::new(format!("could not resolve application directory: {error}"))
    })?;
    let entrypoint_directory = entrypoint.parent().unwrap_or(&root);
    for (selector, attribute) in [
        ("script[src]", "src"),
        ("link[href]", "href"),
        ("img[src]", "src"),
        ("source[src]", "src"),
        ("audio[src]", "src"),
        ("video[src]", "src"),
        ("video[poster]", "poster"),
        ("track[src]", "src"),
        ("embed[src]", "src"),
        ("object[data]", "data"),
        ("input[src]", "src"),
    ] {
        for node in document
            .query_selector_all(document.document(), selector)
            .map_err(|error| JsError::new(error.to_string()))?
        {
            let Some(specifier) = document
                .attribute(node, &blitsen_dom::DomName::attribute(attribute))
                .map_err(|error| JsError::new(error.to_string()))?
            else {
                continue;
            };
            validate_local_asset(&root, entrypoint_directory, &specifier)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_local_asset(
    root: &Path,
    from: &Path,
    specifier: &str,
) -> Result<(), JsError> {
    // An inlined asset is already in the document; there is no file to find.
    // Bundlers inline small images by default, so rejecting `data:` would
    // refuse an ordinary framework build over an asset that cannot be missing.
    if specifier
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return Ok(());
    }
    let has_scheme = specifier.split_once(':').is_some_and(|(scheme, _)| {
        let mut characters = scheme.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && characters
                .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
    });
    if has_scheme {
        return Err(JsError::new(format!(
            "asset URL must be relative to index.html: {specifier}"
        )));
    }
    let local = specifier.split(['?', '#']).next().unwrap_or_default();
    if local.is_empty() {
        return Ok(());
    }
    // A server-root URL names a file at the application root, which is what
    // `blitsen build` rewrites it to at ingest and what the application origin
    // already means inside a shipped executable. Refusing it here was the one
    // place the three disagreed, and it refused the default `vite build` output:
    // `blitsen build dist` exported the directory that `blitsen dist` would not
    // open.
    let asset = match local.strip_prefix('/') {
        Some(from_root) => root.join(from_root),
        None => from.join(local),
    }
        .canonicalize()
        .map_err(|_| JsError::new(format!("unreadable asset from index.html: {specifier}")))?;
    if !asset.starts_with(root) {
        return Err(JsError::new(format!(
            "asset escapes application directory: {specifier}"
        )));
    }
    if !asset.is_file() {
        return Err(JsError::new(format!(
            "unreadable asset from index.html: {specifier}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use blitz::dom::DocumentConfig;

    use super::*;

    #[test]
    fn entrypoint_assets_are_preflighted_inside_the_application_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/blitsen/test/fixtures/scripts")
            .canonicalize()
            .unwrap();
        let entrypoint = root.join("index.html");
        let document = |source| {
            BlitzDom::from_html(
                source,
                DocumentConfig {
                    base_url: Some(format!("file://{}/", root.display())),
                    ..Default::default()
                },
            )
        };
        let valid = document(
            "<link href='#local'><img src='./dependency.js?cache=1'>\
             <img src='data:image/gif;base64,R0lGODlhAQABAAAAACw='>",
        );
        validate_local_assets(&valid, &root, &entrypoint).unwrap();

        // A server-root URL names a file at the application root: the same
        // meaning `blitsen build` rewrites it to and the application origin
        // already carries inside an export. Refusing it here is what made
        // `blitsen dist` reject the default `vite build` output that
        // `blitsen build dist` exports without complaint.
        let from_root = document("<script src='/dependency.js'></script><img src='/module.js'>");
        validate_local_assets(&from_root, &root, &entrypoint).unwrap();

        for (source, expected) in [
            ("<img src='./missing.png'>", "unreadable asset"),
            ("<script src='/nowhere.js'></script>", "unreadable asset"),
            ("<img src='https://example.com/a.png'>", "must be relative"),
            (
                "<img src='../../../../../Cargo.toml'>",
                "escapes application",
            ),
        ] {
            let invalid = document(source);
            let error = validate_local_assets(&invalid, &root, &entrypoint).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }
    }
}
