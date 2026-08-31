//! Language and platform services shared by document and worker realms.

use blitsen_js::{JsEngine, JsError, TypedArray, TypedArrayKind};
use serde_json::{Value, json};

use super::{argument, install_fetch, install_intl, json_value, web_url};

/// Installs the services available in a worker's global scope.
pub fn install_worker_services<E: JsEngine + 'static>(
    engine: &mut E,
    reader: Option<crate::app::AppReader>,
) -> Result<(), JsError> {
    install_text_codec(engine)?;
    install_fetch(engine, reader)?;
    // `Intl` is a language global rather than a document one, so a worker has
    // the same one — and formatting a table of numbers off the main thread is
    // exactly the work a worker is for.
    install_intl(engine)?;
    // The same three facts the document's `navigator` states. A worker has one
    // in a browser, and library code reaches for it to decide what it is running
    // on — Monaco's platform detection gives up without it.
    let navigator = json_value(engine, &navigator_state())?;
    engine.set_global("__blitsenNavigatorState", &navigator)?;
    engine.define_global_function(
        "__blitsenResolveUrl",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let base = argument(&mut engine, &call, 0, "base URL")?;
            let relative = argument(&mut engine, &call, 1, "URL")?;
            let resolved = web_url::resolve(&base, &relative).map_err(JsError::new)?;
            json_value(&mut engine, &resolved)
        }),
    )
}

/// The three facts `navigator` is allowed to state about this machine.
///
/// Identity, never capability: see COMPATIBILITY.md for why the rest of the
/// interface stays absent. The user-agent string names Blitsen rather than
/// impersonating a browser, because an application that sniffs it deserves a
/// true answer more than it deserves a code path written for someone else.
pub(super) fn navigator_state() -> Value {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "MacIntel".to_owned(),
        ("windows", _) => "Win32".to_owned(),
        (os, arch) => format!("{}{} {arch}", os[..1].to_uppercase(), &os[1..]),
    };
    // POSIX locales are `en_GB.UTF-8`; BCP 47 is `en-GB`.
    let language = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|locale| {
            locale
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .replace('_', "-")
        })
        .filter(|locale| !locale.is_empty() && locale != "C" && locale != "POSIX")
        .unwrap_or_else(|| "en-US".to_owned());
    json!({
        "userAgent": format!("Blitsen/{} ({platform})", blitsen_core::RELEASE_VERSION),
        "platform": platform,
        "language": language,
    })
}

/// Installs the UTF-8 conversions the body classes need.
///
/// `TextEncoder` and `TextDecoder` are Web IDL, not ECMAScript: relying on the
/// host's would make the request and response bodies change shape under the
/// Phase 2 engine.
pub(super) fn install_text_codec<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    engine.define_global_function(
        "__blitsenUtf8Encode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let text = argument(&mut engine, &call, 0, "text")?;
            let bytes = TypedArray::new(TypedArrayKind::Uint8, text.into_bytes())?;
            engine.typed_array(&bytes)
        }),
    )?;
    engine.define_global_function(
        "__blitsenUtf8Decode",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let bytes = call
                .arguments
                .first()
                .ok_or_else(|| JsError::new("missing bytes"))
                .and_then(|value| engine.to_typed_array(value))?;
            // Lossy unless the caller asked otherwise, which is what a body
            // wants: a malformed byte becomes U+FFFD rather than losing the
            // response. `new TextDecoder("utf-8", { fatal: true })` is the one
            // caller that asked to be told instead, and only this side can
            // tell — by the time the string exists the evidence is gone.
            let fatal = match call.arguments.get(1) {
                Some(value) => engine.to_boolean(value)?,
                None => false,
            };
            if fatal {
                let text = String::from_utf8(bytes.bytes)
                    .map_err(|error| JsError::new(format!("invalid UTF-8: {error}")))?;
                return engine.string(&text);
            }
            engine.string(&String::from_utf8_lossy(&bytes.bytes))
        }),
    )
}
