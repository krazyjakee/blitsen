//! URL resolution behind `location`, `history` and `fetch`.
//!
//! Parsing lives in Rust rather than in the bootstrap because `URL` is a Web
//! IDL API, not an ECMAScript one: the Phase 1 host happens to provide it and a
//! bare engine context does not. Keeping it here means the Phase 2 swap does not
//! change how an application's router reads its own address.

use serde_json::{Value, json};
use url::Url;

/// Synthetic document address. There is no server and no origin behind an
/// exported application, so the URL is stable, path-rooted — which is all a
/// client-side router needs — and obviously not an HTTP origin.
pub(super) const DOCUMENT_URL: &str = "blitsen://app/";

/// Serializes a URL into the component set `location` and `URL` expose.
///
/// `location` reads a subset; the credentials and the `opaque` flag are here for
/// `URL`, which has to be able to put a URL back together after a setter has
/// changed one component of it — and cannot, for a URL whose path is opaque
/// (`mailto:`, `data:`), which is exactly what the flag says.
fn parts(url: &Url) -> Value {
    let port = url.port().map(|port| port.to_string()).unwrap_or_default();
    let host = url.host_str().unwrap_or_default();
    let origin = format!("{}://{host}", url.scheme());
    json!({
        "href": url.as_str(),
        "protocol": format!("{}:", url.scheme()),
        "username": url.username(),
        "password": url.password().unwrap_or_default(),
        "host": if port.is_empty() { host.to_string() } else { format!("{host}:{port}") },
        "hostname": host,
        "port": port,
        "origin": if port.is_empty() { origin } else { format!("{origin}:{port}") },
        "pathname": if url.path().is_empty() { "/" } else { url.path() },
        "search": url.query().filter(|query| !query.is_empty()).map_or_else(String::new, |query| format!("?{query}")),
        "hash": url.fragment().filter(|fragment| !fragment.is_empty()).map_or_else(String::new, |fragment| format!("#{fragment}")),
        "opaque": url.cannot_be_a_base(),
    })
}

/// Splits an absolute URL into `location` components.
pub(super) fn components(href: &str) -> Result<Value, String> {
    Url::parse(href)
        .map(|url| parts(&url))
        .map_err(|error| format!("invalid URL {href}: {error}"))
}

/// Resolves `relative` against `base`, reporting whether it stayed in origin.
///
/// The caller decides what a cross-origin result means; `history` rejects it the
/// way a browser does, while `fetch` accepts any absolute HTTP URL.
pub(super) fn resolve(base: &str, relative: &str) -> Result<Value, String> {
    let base = Url::parse(base).map_err(|error| format!("invalid base URL {base}: {error}"))?;
    let resolved = base
        .join(relative)
        .map_err(|error| format!("invalid URL {relative}: {error}"))?;
    let same_origin = resolved.scheme() == base.scheme()
        && resolved.host_str() == base.host_str()
        && resolved.port_or_known_default() == base.port_or_known_default();
    let mut value = parts(&resolved);
    value["sameOrigin"] = Value::Bool(same_origin);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(value: &Value, name: &str) -> String {
        value[name].as_str().unwrap().to_string()
    }

    #[test]
    fn the_document_url_is_path_rooted_so_routers_start_at_the_root() {
        let parts = components(DOCUMENT_URL).unwrap();
        assert_eq!(field(&parts, "pathname"), "/");
        assert_eq!(field(&parts, "search"), "");
        assert_eq!(field(&parts, "hash"), "");
        assert_eq!(field(&parts, "origin"), "blitsen://app");
        assert_eq!(field(&parts, "protocol"), "blitsen:");
        assert_eq!(field(&parts, "host"), "app");
    }

    #[test]
    fn relative_targets_resolve_the_way_pushstate_arguments_do() {
        let base = "blitsen://app/settings/profile?tab=1#top";
        for (relative, expected) in [
            ("/reports", "blitsen://app/reports"),
            ("edit", "blitsen://app/settings/edit"),
            ("?tab=2", "blitsen://app/settings/profile?tab=2"),
            ("#anchor", "blitsen://app/settings/profile?tab=1#anchor"),
            ("../", "blitsen://app/"),
        ] {
            let resolved = resolve(base, relative).unwrap();
            assert_eq!(field(&resolved, "href"), expected, "{relative}");
            assert_eq!(resolved["sameOrigin"], Value::Bool(true), "{relative}");
        }
        let resolved = resolve(base, "?tab=2").unwrap();
        assert_eq!(field(&resolved, "search"), "?tab=2");
        assert_eq!(field(&resolved, "pathname"), "/settings/profile");
        assert_eq!(field(&resolved, "hash"), "");
    }

    #[test]
    fn a_different_origin_is_reported_rather_than_silently_accepted() {
        let resolved = resolve(DOCUMENT_URL, "https://example.com/data").unwrap();
        assert_eq!(resolved["sameOrigin"], Value::Bool(false));
        assert_eq!(field(&resolved, "origin"), "https://example.com");
        assert_eq!(field(&resolved, "port"), "");
        let ported = resolve(DOCUMENT_URL, "http://example.com:8080/x").unwrap();
        assert_eq!(field(&ported, "host"), "example.com:8080");
        assert_eq!(field(&ported, "origin"), "http://example.com:8080");
    }

    #[test]
    fn malformed_input_is_an_error_rather_than_a_default() {
        assert!(components("not a url").is_err());
        assert!(resolve("not a url", "/x").is_err());
    }
}
