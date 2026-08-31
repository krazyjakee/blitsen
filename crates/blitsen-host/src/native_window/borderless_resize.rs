//! The resize frame the runtime supplies for an undecorated window.
//!
//! Native decorations normally own the window-edge hit area; without them the
//! application surface reaches the window edge and the runtime provides the
//! resize border itself.

use blitsen_js::JsEngine;
use winit::event::{ButtonSource, ElementState, MouseButton, WindowEvent};
use winit::window::{ResizeDirection, Window, WindowId};

use super::WindowApplication;

/// Width of the resize border supplied for an undecorated window, in logical
/// pixels. Native decorations normally own this hit area; without them the
/// application surface reaches the window edge and the runtime must provide it.
const BORDERLESS_RESIZE_INSET: f64 = 6.0;

fn resize_direction_at(
    physical_x: f64,
    physical_y: f64,
    width: u32,
    height: u32,
    scale: f64,
) -> Option<ResizeDirection> {
    let inset = (BORDERLESS_RESIZE_INSET * scale).max(1.0);
    let horizontal = if physical_x < inset {
        Some(ResizeDirection::West)
    } else if physical_x >= f64::from(width) - inset {
        Some(ResizeDirection::East)
    } else {
        None
    };
    let vertical = if physical_y < inset {
        Some(ResizeDirection::North)
    } else if physical_y >= f64::from(height) - inset {
        Some(ResizeDirection::South)
    } else {
        None
    };
    match (horizontal, vertical) {
        (Some(ResizeDirection::West), Some(ResizeDirection::North)) => {
            Some(ResizeDirection::NorthWest)
        }
        (Some(ResizeDirection::East), Some(ResizeDirection::North)) => {
            Some(ResizeDirection::NorthEast)
        }
        (Some(ResizeDirection::West), Some(ResizeDirection::South)) => {
            Some(ResizeDirection::SouthWest)
        }
        (Some(ResizeDirection::East), Some(ResizeDirection::South)) => {
            Some(ResizeDirection::SouthEast)
        }
        (Some(direction), None) | (None, Some(direction)) => Some(direction),
        _ => None,
    }
}

pub(super) fn borderless_resize_direction(
    window: &dyn Window,
    physical_x: f64,
    physical_y: f64,
) -> Option<ResizeDirection> {
    if window.is_decorated()
        || !window.is_resizable()
        || window.is_maximized()
        || window.fullscreen().is_some()
    {
        return None;
    }
    let size = window.surface_size();
    resize_direction_at(
        physical_x,
        physical_y,
        size.width,
        size.height,
        window.scale_factor(),
    )
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Starts the platform resize loop for a press in the implicit frame of an
    /// undecorated window. The press belongs to that frame, not to the DOM.
    pub(super) fn start_borderless_resize(&self, window_id: WindowId, event: &WindowEvent) -> bool {
        if crate::dom_bridge::window::web_pointer_locked() {
            return false;
        }
        let WindowEvent::PointerButton {
            position,
            state: ElementState::Pressed,
            button: ButtonSource::Mouse(MouseButton::Left),
            ..
        } = event
        else {
            return false;
        };
        let Some(view) = self.inner.windows.get(&window_id) else {
            return false;
        };
        let Some(direction) =
            borderless_resize_direction(view.window.as_ref(), position.x, position.y)
        else {
            return false;
        };
        match view.window.drag_resize_window(direction) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("blitsen: could not start borderless window resize: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResizeDirection, resize_direction_at};

    #[test]
    fn resolves_each_edge_and_corner() {
        let direction = |x, y| resize_direction_at(x, y, 200, 100, 1.0);

        assert_eq!(direction(100.0, 50.0), None);
        assert_eq!(direction(3.0, 50.0), Some(ResizeDirection::West));
        assert_eq!(direction(197.0, 50.0), Some(ResizeDirection::East));
        assert_eq!(direction(100.0, 3.0), Some(ResizeDirection::North));
        assert_eq!(direction(100.0, 97.0), Some(ResizeDirection::South));
        assert_eq!(direction(3.0, 3.0), Some(ResizeDirection::NorthWest));
        assert_eq!(direction(197.0, 3.0), Some(ResizeDirection::NorthEast));
        assert_eq!(direction(3.0, 97.0), Some(ResizeDirection::SouthWest));
        assert_eq!(direction(197.0, 97.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn resize_inset_is_scaled_to_physical_pixels() {
        assert_eq!(
            resize_direction_at(10.0, 100.0, 400, 200, 2.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(resize_direction_at(12.0, 100.0, 400, 200, 2.0), None);
    }
}
