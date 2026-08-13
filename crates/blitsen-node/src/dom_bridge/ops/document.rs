//! Document-wide concerns: loading, and the document's own address.

// These are slices of what was one `match`, so they share the helpers and
// imports their parent gathers rather than restating them.
use super::*;

pub(super) fn dispatch(
    _runtime: &DomRuntime,
    dom: &mut BlitzDom,
    operation: &str,
    arguments: &[String],
) -> Answer {
    let value = match operation {
        // `window.stop()`: every subresource still loading settles here, so a
        // stopped document paints what it has rather than waiting on requests
        // nobody is going to answer.
        "stopLoading" => Ok(json!(dom.stop_loading())),
        "documentUrl" => Ok(Value::String(web_url::DOCUMENT_URL.into())),
        // The *real* base, which is not `documentUrl`: JavaScript sees
        // `blitsen://app/`, while subresources resolve against the directory the
        // document was loaded from. Anything that has to read a file the
        // application shipped needs the second one.
        "documentBase" => Ok(dom
            .base_url()
            .map(|base| Value::String(base.to_owned()))
            .unwrap_or(Value::Null)),
        "urlParts" => web_url::components(bridge_arg(arguments, 0, "URL")?).map_err(JsError::new),
        "resolveUrl" => web_url::resolve(
            bridge_arg(arguments, 0, "base URL")?,
            bridge_arg(arguments, 1, "URL")?,
        )
        .map_err(JsError::new),
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
