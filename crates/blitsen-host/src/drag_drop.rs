//! Files dragged from the desktop into the window, carrying real paths.
//!
//! winit reports one half of HTML drag and drop and it is the half that matters
//! here: a drag that *entered* this window from the file manager, moved over it,
//! and was released on it. There is no drag source — nothing in this module can
//! start a drag out to the desktop — and no in-document drag either, because
//! winit reports neither and inventing them would mean writing platform
//! drop-target code the crate already owns.
//!
//! What a drop carries is the divergence PRODUCT.md §7 argues for. A browser
//! hands the application a `File`, an opaque handle whose bytes must be read
//! back through an asynchronous reader, because a page must not learn where a
//! user keeps their files. An exported Blitsen application *is* the user's
//! program, so the honest answer is the one the platform gave: an absolute
//! filesystem path, which the application's own filesystem library opens
//! directly. `DataTransfer.files` is therefore absent rather than approximated,
//! and `DataTransfer.paths` is what a drop populates.
//!
//! A path the platform spells in bytes that are not UTF-8 is left out rather
//! than handed over lossily: `to_string_lossy` would produce a name that opens
//! nothing, which is worse than a drop the application can see is short.
//!
//! The DOM sequence — which element is entered, which one is left, and whether
//! the drop is accepted at all — is not here. It lives beside the pointer state
//! machine in `dom_bridge/bootstrap/transfer.js`, for the same reason: the
//! target is a node the DOM chose, and only the DOM knows which node the last
//! event was delivered to.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use blitsen_js::{JsEngine, JsError};
use serde::Serialize;
use url::Url;
use winit::event::WindowEvent;
use winit::window::WindowId;

use crate::DomRuntime;
use crate::native_window::{
    InputBootstrap, ModifierFlags, WindowApplication, css_pointer_coordinates, take_queued_for,
};

/// Where a drag is, and what it is doing there.
///
/// `Over` covers both winit's `DragEntered` and its `DragMoved`: which DOM
/// events that becomes — `dragenter`, `dragleave`, `dragover`, or all three —
/// depends on the element under the pointer, and the element is JavaScript's to
/// resolve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DragStage {
    Over { physical_x: f64, physical_y: f64 },
    Drop { physical_x: f64, physical_y: f64 },
    Leave,
}

impl DragStage {
    /// The bootstrap's name for this stage.
    fn name(self) -> &'static str {
        match self {
            Self::Over { .. } => "over",
            Self::Drop { .. } => "drop",
            Self::Leave => "leave",
        }
    }
}

/// One drag event, held until the frame turn that dispatches it.
///
/// The files travel with the event rather than being read back at dispatch,
/// because winit names them when the drag arrives and again when it is released
/// and never on the moves between: the session's list is what every event of it
/// must report, including the ones that arrive after a second drag has begun.
/// Shared rather than copied, so coalescing a move costs a refcount.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingDrag {
    stage: DragStage,
    paths: Rc<[PathBuf]>,
}

/// What a drag event tells JavaScript, beside the paths it carries.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DragEventInit {
    client_x: f64,
    client_y: f64,
    offset_x: f32,
    offset_y: f32,
    screen_x: f64,
    screen_y: f64,
    /// Absolute filesystem paths, in the order the platform listed them.
    paths: Vec<String>,
    /// The same files as `file:` URLs, which is what `text/uri-list` is.
    uris: Vec<String>,
    #[serde(flatten)]
    modifiers: ModifierFlags,
}

/// Reads a drag out of a window event, with the paths it announces.
///
/// The path list is `None` for everything but the two events winit names it on,
/// which is what makes the session's own list load-bearing.
pub(crate) fn classify_drag_event(event: &WindowEvent) -> Option<(DragStage, Option<&[PathBuf]>)> {
    match event {
        WindowEvent::DragEntered { paths, position } => Some((
            DragStage::Over {
                physical_x: position.x,
                physical_y: position.y,
            },
            Some(paths.as_slice()),
        )),
        WindowEvent::DragMoved { position } => Some((
            DragStage::Over {
                physical_x: position.x,
                physical_y: position.y,
            },
            None,
        )),
        WindowEvent::DragDropped { paths, position } => Some((
            DragStage::Drop {
                physical_x: position.x,
                physical_y: position.y,
            },
            Some(paths.as_slice()),
        )),
        WindowEvent::DragLeft { .. } => Some((DragStage::Leave, None)),
        _ => None,
    }
}

/// The paths and `file:` URLs a drop hands JavaScript.
///
/// A path that is not valid UTF-8 has no JavaScript spelling and no URL, so it
/// appears in neither list rather than in one of them.
fn transferable(paths: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let usable: Vec<&Path> = paths
        .iter()
        .filter(|path| path.to_str().is_some())
        .map(PathBuf::as_path)
        .collect();
    let uris = usable
        .iter()
        .filter_map(|path| Url::from_file_path(path).ok())
        .map(String::from)
        .collect();
    let paths = usable
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    (paths, uris)
}

impl<Rend: anyrender::WindowRenderer, E: JsEngine + Clone> WindowApplication<Rend, E> {
    /// Holds a drag event until the frame that will dispatch it.
    ///
    /// Reports whether anything was queued, which is what makes the window ask
    /// for the redraw that drains it.
    pub(crate) fn queue_drag_input(&mut self, window_id: WindowId, event: &WindowEvent) -> bool {
        let Some((stage, announced)) = classify_drag_event(event) else {
            return false;
        };
        if let Some(paths) = announced {
            self.drag_paths = paths.into();
        }
        // One move per turn, for the reason a pointer move is coalesced: a queue
        // of stale positions for the same drag only costs hit tests nothing will
        // read. An enter and a leave are kept, because each is a boundary the
        // document has to be told about in order.
        if matches!(stage, DragStage::Over { .. }) {
            self.pending_drag_input.retain(|(queued_window, queued)| {
                *queued_window != window_id || !matches!(queued.stage, DragStage::Over { .. })
            });
        }
        self.pending_drag_input.push((
            window_id,
            PendingDrag {
                stage,
                paths: Rc::clone(&self.drag_paths),
            },
        ));
        // A release ends the session: the next drag announces its own files, and
        // nothing between the two should be able to report these.
        if matches!(stage, DragStage::Drop { .. }) {
            self.drag_paths = Rc::from([]);
        }
        true
    }

    /// Dispatches everything the turn queued, at the tree the frame settled on.
    pub(crate) fn drain_drag_input(&mut self, window_id: WindowId) {
        let Some(drags) = take_queued_for(
            self.error.as_ref(),
            &mut self.pending_drag_input,
            &window_id,
        ) else {
            return;
        };
        if drags.is_empty() {
            return;
        }
        let Some((scale, screen_origin_x, screen_origin_y)) = self.window_geometry(window_id) else {
            return;
        };
        for drag in drags {
            let stage = drag.stage.name();
            let (physical_x, physical_y) = match drag.stage {
                DragStage::Over {
                    physical_x,
                    physical_y,
                }
                | DragStage::Drop {
                    physical_x,
                    physical_y,
                } => (physical_x, physical_y),
                // A drag that has left has no position and no element under it.
                // JavaScript still has to be told, because the element the last
                // event reached is holding a `dragover` highlight.
                DragStage::Leave => {
                    if let Err(error) = self.dispatch_drag(stage, None) {
                        self.park_error(error);
                        return;
                    }
                    continue;
                }
            };
            let (client_x, client_y, screen_x, screen_y) = css_pointer_coordinates(
                physical_x,
                physical_y,
                scale,
                screen_origin_x,
                screen_origin_y,
            );
            let hit = match self.hit_test(client_x, client_y) {
                Ok(Some(hit)) => hit,
                // A drag over a point with nothing under it — the window's own
                // margin — is the same as a drag that has left as far as the
                // document is concerned, and leaving it un-notified would strand
                // the highlight on the element it was last over.
                Ok(None) => {
                    if let Err(error) = self.dispatch_drag("leave", None) {
                        self.park_error(error);
                        return;
                    }
                    continue;
                }
                Err(error) => {
                    self.park_error(JsError::new(error.to_string()));
                    return;
                }
            };
            let (paths, uris) = transferable(&drag.paths);
            let init = DragEventInit {
                client_x,
                client_y,
                offset_x: hit.offset_x,
                offset_y: hit.offset_y,
                screen_x,
                screen_y,
                paths,
                uris,
                modifiers: self.modifier_flags(),
            };
            let target = DomRuntime::serialize_handle(hit.target);
            if let Err(error) = self.dispatch_drag(stage, Some((target, init))) {
                self.park_error(error);
                return;
            }
        }
    }

    /// Hands one stage of the drag to the bootstrap's drag state machine.
    fn dispatch_drag(
        &self,
        stage: &str,
        landed: Option<(String, DragEventInit)>,
    ) -> Result<bool, JsError> {
        match landed {
            Some((target, init)) => {
                self.call_input_bootstrap(InputBootstrap::Drag, &(stage, target, init))
            }
            None => self.call_input_bootstrap(InputBootstrap::Drag, &(stage,)),
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    #[test]
    fn a_move_inherits_the_paths_the_drag_was_announced_with() {
        let entered = WindowEvent::DragEntered {
            paths: vec![PathBuf::from("/tmp/one.txt")],
            position: PhysicalPosition::new(4.0, 8.0),
        };
        let (stage, announced) = classify_drag_event(&entered).expect("an entered drag is a drag");
        assert_eq!(
            stage,
            DragStage::Over {
                physical_x: 4.0,
                physical_y: 8.0
            }
        );
        assert_eq!(announced, Some([PathBuf::from("/tmp/one.txt")].as_slice()));
        let moved = WindowEvent::DragMoved {
            position: PhysicalPosition::new(6.0, 9.0),
        };
        let (stage, announced) = classify_drag_event(&moved).expect("a moved drag is a drag");
        assert_eq!(stage.name(), "over");
        assert!(
            announced.is_none(),
            "winit names the files once, so a move must not clear them"
        );
        assert!(classify_drag_event(&WindowEvent::CloseRequested).is_none());
    }

    /// `name` in the temporary directory, spelled the way this host spells a path.
    ///
    /// `Url::from_file_path` converts an absolute path and refuses every other
    /// one, and which paths are absolute is the platform's rule rather than
    /// POSIX's: `/tmp/a b.txt` names nothing absolute on Windows, where a path
    /// begins at a drive letter or a UNC share. The temporary directory is the
    /// one location a test can name that whichever host runs it already agrees
    /// is absolute, so the conversion under test is exercised everywhere instead
    /// of only where the literal happened to be well formed.
    fn absolute(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn a_drop_carries_absolute_paths_and_the_same_files_as_uris() {
        let spaced = absolute("a b.txt");
        let plain = absolute("plain.txt");
        let (paths, uris) = transferable(&[spaced.clone(), plain.clone()]);
        assert_eq!(
            paths,
            [
                spaced.to_str().expect("a temporary path spells in UTF-8"),
                plain.to_str().expect("a temporary path spells in UTF-8"),
            ]
        );
        // Every path has to come back out of its URL as the file the platform
        // announced: a `text/uri-list` entry that reads back as another name is
        // a file the application would open at the wrong one.
        let read_back = uris
            .iter()
            .map(|uri| {
                Url::parse(uri)
                    .expect("the uri list must be parseable")
                    .to_file_path()
                    .expect("a file: url must name its path again")
            })
            .collect::<Vec<_>>();
        assert_eq!(read_back, [spaced, plain], "the uri list must be parseable");
        // Percent-encoded by the URL parser rather than by hand: a space in a
        // file name is the first thing a `text/uri-list` reader trips over.
        assert!(
            uris[0].ends_with("/a%20b.txt") && !uris[0].contains(' '),
            "a space must reach the uri list encoded, but it is {:?}",
            uris[0]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_with_no_javascript_spelling_is_left_out_rather_than_mangled() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let (paths, uris) = transferable(&[
            PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe".to_vec())),
            PathBuf::from("/tmp/readable.txt"),
        ]);
        assert_eq!(paths, ["/tmp/readable.txt"]);
        assert_eq!(uris, ["file:///tmp/readable.txt"]);
    }
}
