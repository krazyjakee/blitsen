//! OS clipboard access, backed by `arboard`.
//!
//! One clipboard is created per thread and kept for the life of the process on
//! purpose. On X11 and Wayland the process that wrote the selection is the one
//! that serves it, so dropping the clipboard takes the copied text with it.
//!
//! Absent on Android. The compile error comes first — `arboard` has no Android
//! backend, its `platform` module resolves to nothing there, and the crate does
//! not build — but a shim over `ClipboardManager` would not settle it either,
//! because the two disagree about what an empty answer means. Every reader here
//! returns an `Option`, and `None` says the clipboard holds nothing of that
//! flavour. Android has refused clipboard reads to any application that does not
//! hold focus since API 29, so the same `None` would also mean "you asked while
//! in the background" — a refusal the caller has to be able to tell from an
//! empty clipboard, and one these signatures have nowhere to put. Writing is not
//! symmetric either: a `ClipData` is typed by MIME rather than by the three
//! flavours below, and the paste confirmation the system shows is not ours to
//! suppress.
//!
//! So the Android clipboard is a module of its own to design, on a JNI surface
//! that does not exist until there is an entry point holding an `AndroidApp`
//! (#142). Until then it is absent, which the bootstrap reports as `undefined`
//! and a feature test reads correctly (#147).

use std::cell::RefCell;

use arboard::Clipboard;

use crate::PlatformError;

thread_local! {
    static CLIPBOARD: RefCell<Option<Clipboard>> = const { RefCell::new(None) };
}

impl From<arboard::Error> for PlatformError {
    fn from(error: arboard::Error) -> Self {
        PlatformError::new(error.to_string())
    }
}

/// Pixels as the clipboard carries them: 8-bit RGBA, row-major, unpadded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Exactly `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Reads the clipboard as plain text, or `None` when it holds no text.
pub fn read_text() -> Result<Option<String>, PlatformError> {
    optional(|clipboard| clipboard.get_text())
}

/// Reads the clipboard's HTML flavour, or `None` when it has none.
pub fn read_html() -> Result<Option<String>, PlatformError> {
    optional(|clipboard| clipboard.get().html())
}

/// Reads the clipboard as an image, or `None` when it holds no image in a
/// format this platform can decode.
pub fn read_image() -> Result<Option<Image>, PlatformError> {
    let image = optional(|clipboard| clipboard.get_image())?;
    Ok(image.map(|image| Image {
        width: image.width,
        height: image.height,
        rgba: image.bytes.into_owned(),
    }))
}

/// Replaces the clipboard contents with plain text.
pub fn write_text(text: &str) -> Result<(), PlatformError> {
    with(|clipboard| clipboard.set_text(text))
}

/// Replaces the clipboard contents with HTML, plus the plain text an
/// application that cannot read HTML pastes instead.
pub fn write_html(html: &str, alternative: Option<&str>) -> Result<(), PlatformError> {
    with(|clipboard| clipboard.set_html(html, alternative))
}

/// Replaces the clipboard contents with an image.
pub fn write_image(image: &Image) -> Result<(), PlatformError> {
    let expected = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4));
    if image.width == 0 || image.height == 0 || expected != Some(image.rgba.len()) {
        return Err(PlatformError::new(format!(
            "a {}x{} image is {} RGBA bytes, not {}",
            image.width,
            image.height,
            expected.unwrap_or_default(),
            image.rgba.len()
        )));
    }
    with(|clipboard| {
        clipboard.set_image(arboard::ImageData {
            width: image.width,
            height: image.height,
            bytes: image.rgba.as_slice().into(),
        })
    })
}

/// Empties the clipboard.
pub fn clear() -> Result<(), PlatformError> {
    with(|clipboard| clipboard.clear())
}

fn with<T>(
    operation: impl FnOnce(&mut Clipboard) -> Result<T, arboard::Error>,
) -> Result<T, PlatformError> {
    connected(|clipboard| operation(clipboard).map_err(PlatformError::from))
}

/// An empty clipboard is an answer rather than a failure; anything else is one.
fn optional<T>(
    operation: impl FnOnce(&mut Clipboard) -> Result<T, arboard::Error>,
) -> Result<Option<T>, PlatformError> {
    connected(|clipboard| match operation(clipboard) {
        Ok(value) => Ok(Some(value)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    })
}

/// Runs `operation` against this thread's clipboard, connecting on first use.
///
/// Connecting is deferred because a headless process has no clipboard to
/// connect to and must still start: the refusal belongs at the call that wanted
/// the clipboard, not at startup.
fn connected<T>(
    operation: impl FnOnce(&mut Clipboard) -> Result<T, PlatformError>,
) -> Result<T, PlatformError> {
    CLIPBOARD.with(|cell| {
        let mut cell = cell.borrow_mut();
        let clipboard = match cell.as_mut() {
            Some(clipboard) => clipboard,
            None => cell.insert(Clipboard::new().map_err(|error| {
                PlatformError::new(format!("the system clipboard is unavailable: {error}"))
            })?),
        };
        operation(clipboard)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_must_carry_its_own_pixels() {
        let short = Image {
            width: 2,
            height: 2,
            rgba: vec![0; 8],
        };
        assert!(write_image(&short).is_err());
        let empty = Image {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(write_image(&empty).is_err());
    }
}
