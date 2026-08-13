//! What a document's subresources point at, checked before it renders.
//!
//! Only one thing here is fatal: a reference that leaves the application
//! directory. Everything else a directory being run can get wrong — a file that
//! is not there, a remote URL — is something the renderer already degrades, and
//! something an export degrades rather than refusing. A subresource is answered
//! with an empty body and the document paints without it, which is what makes a
//! broken `<img>` reach its errored state instead of blocking the frame.
//!
//! Refusing them here instead meant `blitsen <dir>` would not open a document
//! that `blitsen build` exports and runs quite happily — `examples/assets` is
//! one, and its missing image is deliberate. So what cannot be served is
//! reported and the document renders, and the check that remains is the one a
//! directory genuinely needs: an export can only serve what it collected, and
//! this keeps a directory run to the same files.

use std::path::Path;

use blitsen_blitz::BlitzDom;
use blitsen_dom::DomBackend;
use blitsen_js::JsError;

/// Refuses a document whose subresources escape the application directory, and
/// reports the ones it will not be able to serve.
///
/// The returned notes are for the reader, not the document: it renders either
/// way, and a subresource that silently contributed nothing would be a blank
/// panel with no explanation anywhere.
pub fn validate_local_assets(
    document: &BlitzDom,
    root: &Path,
    entrypoint: &Path,
) -> Result<Vec<String>, JsError> {
    let root = root.canonicalize().map_err(|error| {
        JsError::new(format!("could not resolve application directory: {error}"))
    })?;
    let entrypoint_directory = entrypoint.parent().unwrap_or(&root);
    let mut notes = Vec::new();
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
            if let Some(note) = validate_local_asset(&root, entrypoint_directory, &specifier)? {
                notes.push(note);
            }
        }
    }
    Ok(notes)
}

pub(crate) fn validate_local_asset(
    root: &Path,
    from: &Path,
    specifier: &str,
) -> Result<Option<String>, JsError> {
    // An inlined asset is already in the document; there is no file to find.
    // Bundlers inline small images by default, so rejecting `data:` would
    // refuse an ordinary framework build over an asset that cannot be missing.
    if specifier
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return Ok(None);
    }
    let has_scheme = specifier.split_once(':').is_some_and(|(scheme, _)| {
        let mut characters = scheme.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && characters
                .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
    });
    // A remote subresource is not fetched, and the renderer answers it with
    // nothing, so the page renders without that stylesheet, font or image —
    // the same answer an export gives, where `doctor` has already warned.
    if has_scheme {
        return Ok(Some(format!(
            "not fetching a remote subresource, so it renders without it: {specifier}"
        )));
    }
    let local = specifier.split(['?', '#']).next().unwrap_or_default();
    if local.is_empty() {
        return Ok(None);
    }
    // A server-root URL names a file at the application root, which is what
    // `blitsen build` rewrites it to at ingest and what the application origin
    // already means inside a shipped executable. Refusing it here was the one
    // place the three disagreed, and it refused the default `vite build` output:
    // `blitsen build dist` exported the directory that `blitsen dist` would not
    // open.
    let target = match local.strip_prefix('/') {
        Some(from_root) => root.join(from_root),
        None => from.join(local),
    };
    // Canonicalising is how the escape is detected, so a path that does not
    // exist cannot be checked — and is not the thing being checked for. It is
    // reported and the document renders, exactly as the export does with it.
    let Ok(asset) = target.canonicalize() else {
        return Ok(Some(format!(
            "the application does not ship this subresource, so it renders \
             without it: {specifier}"
        )));
    };
    if !asset.starts_with(root) {
        return Err(JsError::new(format!(
            "asset escapes application directory: {specifier}"
        )));
    }
    if !asset.is_file() {
        return Ok(Some(format!(
            "this subresource is not a file, so it renders without it: {specifier}"
        )));
    }
    Ok(None)
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

        // Reported, and rendered anyway. The renderer degrades each of these to
        // an empty body, and an export does too — refusing them here would mean
        // a document that runs once exported and will not open from the
        // directory it was exported from. `examples/assets` is exactly that
        // document, and its missing image is the point of it.
        for (source, expected) in [
            ("<img src='./missing.png'>", "does not ship this subresource"),
            (
                "<script src='/nowhere.js'></script>",
                "does not ship this subresource",
            ),
            (
                "<img src='https://example.com/a.png'>",
                "not fetching a remote subresource",
            ),
            ("<img src='./'>", "not a file"),
        ] {
            let notes = validate_local_assets(&document(source), &root, &entrypoint).unwrap();
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].contains(expected), "{}", notes[0]);
        }

        // The one thing a directory can get wrong that an export cannot: an
        // export only serves what it collected, and this keeps a directory run
        // to the same files.
        let escaping = document("<img src='../../../../../Cargo.toml'>");
        let error = validate_local_assets(&escaping, &root, &entrypoint).unwrap_err();
        assert!(
            error.message().contains("escapes application"),
            "{}",
            error.message()
        );
    }
}
