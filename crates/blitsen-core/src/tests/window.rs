use super::*;

#[test]
fn window_state_tracks_logical_resize_dimensions() {
    let mut window = WindowState::new(800, 600, 2.0);
    assert_eq!((window.width(), window.height()), (800, 600));
    assert_eq!(window.device_pixel_ratio(), 2.0);
    window.resize(1024, 768);
    assert_eq!((window.width(), window.height()), (1024, 768));
}
