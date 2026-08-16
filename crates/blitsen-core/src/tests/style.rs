use super::*;

#[test]
fn javascript_style_names_map_to_css_names() {
    assert_eq!(js_property_to_css("backgroundColor"), "background-color");
    assert_eq!(js_property_to_css("WebkitTransform"), "-webkit-transform");
    assert_eq!(js_property_to_css("--brandColor"), "--brandColor");
    assert_eq!(js_property_to_css("cssFloat"), "float");
}
