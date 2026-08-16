//! Property-name mapping shared with the host's inline-style bridge.

/// Maps a JavaScript camelCase style property to its CSS spelling.
pub fn js_property_to_css(property: &str) -> String {
    if property.starts_with("--") {
        return property.into();
    }
    if property == "cssFloat" {
        return "float".into();
    }
    let mut css = String::with_capacity(property.len() + 4);
    for (index, character) in property.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index == 0 || !css.ends_with('-') {
                css.push('-');
            }
            css.push(character.to_ascii_lowercase());
        } else {
            css.push(character);
        }
    }
    css
}
