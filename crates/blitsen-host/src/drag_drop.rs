//! Files dragged from the desktop into the window, carrying real paths.
//!
//! winit reports one half of HTML drag and drop and it is the half that matters
//! here: a drag that *entered* this window from the file manager, moved over it,
//! and was released on it. There is no drag source — nothing in this module
//! starts a drag out to the desktop — and no in-document drag either, because
//! inventing one would mean writing platform drop-target code the crate
//! already owns.
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
//! winit names a drag by id and hands its files over separately: this host
//! accepts a drag that offers a URI list, asks for the list, and receives it
//! in a later event — on X11, after the drop. The document is told nothing
//! about a drag until its files are known ([`DragSession`]).
//!
//! The DOM sequence — which element is entered, which one is left, and whether
//! the drop is accepted at all — is not here. It lives beside the pointer state
//! machine in `dom_bridge/bootstrap/transfer.js`, for the same reason: the
//! target is a node the DOM chose, and only the DOM knows which node the last
//! event was delivered to.

use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use blitsen_js::{JsEngine, JsError};
use serde::Serialize;
use url::Url;
use winit::data_transfer::{DataTransferId, TypeHint, TypedData};
use winit::dpi::PhysicalPosition;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, DndAction};
use winit::window::WindowId;

use crate::DomRuntime;
use crate::native_window::{
    InputBootstrap, ModifierFlags, WindowApplication, css_pointer_coordinates, take_queued_for,
};

/// Where a drag is, and what it is doing there.
///
/// `Over` covers both winit's `DragEntered` and its `DragPosition`: which DOM
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
/// The files travel with the event rather than being read back at dispatch:
/// a drag that ends and a second that begins inside one turn must not report
/// each other's files. Shared rather than copied, so coalescing a move costs a
/// refcount.
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

/// A winit event that belongs to an incoming drag.
#[derive(Debug)]
pub(crate) enum DragSignal<'a> {
    Entered {
        id: DataTransferId,
        position: Option<PhysicalPosition<f64>>,
    },
    Moved {
        id: DataTransferId,
        position: PhysicalPosition<f64>,
    },
    Dropped {
        id: DataTransferId,
    },
    Left {
        id: DataTransferId,
    },
    /// The data a fetch asked for, which for this host is always the URI list.
    Received {
        id: DataTransferId,
        value: &'a Arc<dyn TypedData>,
    },
}

impl DragSignal<'_> {
    fn id(&self) -> DataTransferId {
        match self {
            Self::Entered { id, .. }
            | Self::Moved { id, .. }
            | Self::Dropped { id }
            | Self::Left { id }
            | Self::Received { id, .. } => *id,
        }
    }
}

/// Reads a drag out of a window event.
pub(crate) fn classify_drag_event(event: &WindowEvent) -> Option<DragSignal<'_>> {
    Some(match event {
        WindowEvent::DragEntered { id, position } => DragSignal::Entered {
            id: *id,
            position: *position,
        },
        WindowEvent::DragPosition { id, position, .. } => DragSignal::Moved {
            id: *id,
            position: *position,
        },
        WindowEvent::DragDropped { id, .. } => DragSignal::Dropped { id: *id },
        WindowEvent::DragLeft { id } => DragSignal::Left { id: *id },
        WindowEvent::DataTransferReceived { id, value, .. } => {
            DragSignal::Received { id: *id, value }
        }
        _ => return None,
    })
}

/// The one drag winit is reporting, from the moment it enters until the
/// document has been told how it ended.
///
/// The document's `DataTransfer.types` names `Files` only when a drag carries
/// paths, so a stage dispatched before its files arrived would describe a
/// different drag — one an application checking for files would refuse. Every
/// stage is held here, in order, until the files are known.
pub(crate) struct DragSession {
    id: DataTransferId,
    /// Where the drag was last reported. A drop names no position of its own.
    position: Option<PhysicalPosition<f64>>,
    files: SessionFiles,
    /// A drop or a leave has been seen. The session outlives it while its files
    /// are still outstanding, because a drop is waiting on them.
    ended: bool,
}

enum SessionFiles {
    /// Asked for and not yet read. `unread` is a delivery whose read would have
    /// blocked, retried on the session's next event.
    Awaiting {
        held: Vec<DragStage>,
        unread: Option<Arc<dyn TypedData>>,
    },
    Known(Rc<[PathBuf]>),
    /// The drag names no local file — a web link, say — so it was refused and
    /// the document hears nothing of it.
    Refused,
}

/// What an event let a session hand on.
#[derive(Debug, PartialEq)]
pub(crate) enum Delivery {
    Nothing,
    /// These stages, in order, with the files they carry.
    Stages(Vec<DragStage>, Rc<[PathBuf]>),
    /// The files arrived and none of them is local.
    Refuse,
}

impl DragSession {
    pub(crate) fn new(id: DataTransferId) -> Self {
        Self {
            id,
            position: None,
            files: SessionFiles::Awaiting {
                held: Vec::new(),
                unread: None,
            },
            ended: false,
        }
    }

    /// Advances the session by one of its own events.
    pub(crate) fn advance(&mut self, signal: DragSignal<'_>) -> Delivery {
        let over = |position: PhysicalPosition<f64>| DragStage::Over {
            physical_x: position.x,
            physical_y: position.y,
        };
        let stage = match signal {
            DragSignal::Entered { position, .. } => {
                self.position = position;
                position.map(over)
            }
            DragSignal::Moved { position, .. } => {
                self.position = Some(position);
                Some(over(position))
            }
            // A drop at a point never reported cannot be hit tested, and is the
            // same as a drag that left as far as the document is concerned.
            DragSignal::Dropped { .. } => {
                self.ended = true;
                Some(
                    self.position
                        .map_or(DragStage::Leave, |position| DragStage::Drop {
                            physical_x: position.x,
                            physical_y: position.y,
                        }),
                )
            }
            DragSignal::Left { .. } => {
                self.ended = true;
                Some(DragStage::Leave)
            }
            DragSignal::Received { value, .. } => {
                if let SessionFiles::Awaiting { unread, .. } = &mut self.files {
                    *unread = Some(Arc::clone(value));
                }
                None
            }
        };
        match &mut self.files {
            SessionFiles::Refused => Delivery::Nothing,
            SessionFiles::Known(paths) => match stage {
                Some(stage) => Delivery::Stages(vec![stage], Rc::clone(paths)),
                None => Delivery::Nothing,
            },
            SessionFiles::Awaiting { held, .. } => {
                match stage {
                    // Nothing held has reached the document, so a drag that
                    // leaves before its files arrive has nothing to tell it.
                    Some(DragStage::Leave) => held.clear(),
                    Some(stage @ DragStage::Over { .. })
                        if matches!(held.last(), Some(DragStage::Over { .. })) =>
                    {
                        *held.last_mut().expect("matched a last stage") = stage;
                    }
                    Some(stage) => held.push(stage),
                    None => {}
                }
                self.read_files()
            }
        }
    }

    /// Whether the document has been told everything this drag will tell it.
    pub(crate) fn finished(&self) -> bool {
        self.ended && !matches!(self.files, SessionFiles::Awaiting { .. })
    }

    fn read_files(&mut self) -> Delivery {
        let SessionFiles::Awaiting {
            held,
            unread: Some(value),
        } = &mut self.files
        else {
            return Delivery::Nothing;
        };
        let paths = match value.try_as_uris() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Delivery::Nothing,
            Ok(uris) => local_paths(&uris),
            Err(_) => Vec::new(),
        };
        if paths.is_empty() {
            self.files = SessionFiles::Refused;
            return Delivery::Refuse;
        }
        let held = std::mem::take(held);
        let paths: Rc<[PathBuf]> = paths.into();
        self.files = SessionFiles::Known(Rc::clone(&paths));
        Delivery::Stages(held, paths)
    }
}

/// The local files a URI list names, in its order.
///
/// Platforms spell an entry as a `file:` URL, falling back to a bare absolute
/// path where they have no URL for it. Anything else is not a file this
/// application can open, and is left out.
fn local_paths(uris: &[String]) -> Vec<PathBuf> {
    uris.iter()
        .filter_map(|uri| match Url::parse(uri) {
            Ok(url) if url.scheme() == "file" => file_url_path(&url),
            Ok(_) => None,
            Err(_) => Some(PathBuf::from(uri)).filter(|path| path.is_absolute()),
        })
        .collect()
}

/// A `file:` URL as the path it names.
///
/// Finder puts file *reference* URLs on the pasteboard — `file:///.file/id=…`,
/// naming a file by volume and inode — and winit passes them on. The path they
/// spell opens nothing through POSIX, so it is resolved to the file's current
/// path the way AppKit does.
#[cfg(target_os = "macos")]
fn file_url_path(url: &Url) -> Option<PathBuf> {
    use objc2_foundation::{NSString, NSURL};

    if !url.path().starts_with("/.file/id=") {
        return url.to_file_path().ok();
    }
    let reference = NSURL::URLWithString(&NSString::from_str(url.as_str()))?;
    let path = reference.filePathURL()?.path()?;
    Some(PathBuf::from(path.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn file_url_path(url: &Url) -> Option<PathBuf> {
    url.to_file_path().ok()
}

/// Accepts a drag that offers a URI list and asks the platform for the list.
///
/// A drag with no URI list — selected text, an image dragged out of a browser —
/// is not a file drop, and is left for the platform to refuse.
fn accept_file_drag(event_loop: &dyn ActiveEventLoop, id: DataTransferId) -> bool {
    let offers_files = event_loop
        .data_transfer(id)
        .is_ok_and(|transfer| transfer.has_type(&TypeHint::UriList));
    if !offers_files
        || event_loop
            .set_valid_dnd_actions(id, &[DndAction::Copy])
            .is_err()
    {
        return false;
    }
    if event_loop
        .fetch_data_transfer(id, &TypeHint::UriList)
        .is_err()
    {
        let _ = event_loop.set_valid_dnd_actions(id, &[]);
        return false;
    }
    true
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
    /// Advances the incoming drag and holds whatever it releases until the
    /// frame that will dispatch it.
    ///
    /// Reports whether anything was queued, which is what makes the window ask
    /// for the redraw that drains it.
    pub(crate) fn queue_drag_input(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: &WindowEvent,
    ) -> bool {
        let Some(signal) = classify_drag_event(event) else {
            return false;
        };
        // winit reports one drag at a time, so a new one replaces whatever the
        // last left behind.
        if let DragSignal::Entered { id, .. } = signal {
            self.drag_session = accept_file_drag(event_loop, id).then(|| DragSession::new(id));
        }
        let Some(session) = self
            .drag_session
            .as_mut()
            .filter(|session| session.id == signal.id())
        else {
            return false;
        };
        let id = session.id;
        let delivery = session.advance(signal);
        if session.finished() {
            self.drag_session = None;
        }
        match delivery {
            Delivery::Nothing => false,
            Delivery::Refuse => {
                // After a drop the platform may have forgotten the drag, and
                // there is nothing left to refuse.
                let _ = event_loop.set_valid_dnd_actions(id, &[]);
                false
            }
            Delivery::Stages(stages, paths) => {
                let queued = !stages.is_empty();
                for stage in stages {
                    self.push_drag(window_id, stage, &paths);
                }
                queued
            }
        }
    }

    fn push_drag(&mut self, window_id: WindowId, stage: DragStage, paths: &Rc<[PathBuf]>) {
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
                paths: Rc::clone(paths),
            },
        ));
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
        let Some((scale, screen_origin_x, screen_origin_y)) = self.window_geometry(window_id)
        else {
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
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};

    use winit::data_transfer::TransferType;

    /// A URI list as a platform hands it over, optionally blocking on first read.
    #[derive(Debug)]
    struct UriList {
        uris: Vec<String>,
        blocks_once: AtomicBool,
    }

    impl UriList {
        fn arriving(uris: &[&str]) -> Arc<dyn TypedData> {
            Arc::new(Self {
                uris: uris.iter().map(|uri| (*uri).to_owned()).collect(),
                blocks_once: AtomicBool::new(false),
            })
        }
    }

    impl TypedData for UriList {
        fn type_(&self) -> &dyn TransferType {
            &TypeHint::UriList
        }

        fn try_read(&self) -> Option<Box<dyn io::BufRead>> {
            None
        }

        fn try_as_uris(&self) -> io::Result<Vec<String>> {
            if self.blocks_once.swap(false, Ordering::SeqCst) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            Ok(self.uris.clone())
        }

        fn try_as_string(&self) -> io::Result<String> {
            Err(io::ErrorKind::InvalidData.into())
        }
    }

    const ID: DataTransferId = DataTransferId::from_raw(7);

    fn at(x: f64, y: f64) -> PhysicalPosition<f64> {
        PhysicalPosition::new(x, y)
    }

    fn over(x: f64, y: f64) -> DragStage {
        DragStage::Over {
            physical_x: x,
            physical_y: y,
        }
    }

    fn file_uri(name: &str) -> (PathBuf, String) {
        let path = absolute(name);
        let uri = Url::from_file_path(&path)
            .expect("a temporary path is absolute")
            .to_string();
        (path, uri)
    }

    #[test]
    fn winit_drag_events_are_read_by_transfer() {
        let entered = WindowEvent::DragEntered {
            id: ID,
            position: None,
        };
        assert!(matches!(
            classify_drag_event(&entered),
            Some(DragSignal::Entered {
                id: ID,
                position: None
            })
        ));
        let dropped = WindowEvent::DragDropped {
            id: ID,
            proposed_action: Some(DndAction::Copy),
        };
        assert_eq!(
            classify_drag_event(&dropped).map(|signal| signal.id()),
            Some(ID)
        );
        assert!(classify_drag_event(&WindowEvent::CloseRequested).is_none());
    }

    #[test]
    fn stages_wait_for_the_files_and_then_carry_them() {
        let (path, uri) = file_uri("held.txt");
        let mut session = DragSession::new(ID);
        let entered = DragSignal::Entered {
            id: ID,
            position: Some(at(4.0, 8.0)),
        };
        assert_eq!(session.advance(entered), Delivery::Nothing);
        let moved = DragSignal::Moved {
            id: ID,
            position: at(6.0, 9.0),
        };
        assert_eq!(session.advance(moved), Delivery::Nothing);
        let value = UriList::arriving(&[&uri]);
        let Delivery::Stages(stages, paths) = session.advance(DragSignal::Received {
            id: ID,
            value: &value,
        }) else {
            panic!("the files release what was held");
        };
        assert_eq!(
            stages,
            [over(6.0, 9.0)],
            "moves held for the files coalesce like queued ones"
        );
        assert_eq!(&*paths, [path.clone()]);
        let moved = DragSignal::Moved {
            id: ID,
            position: at(7.0, 9.0),
        };
        assert_eq!(
            session.advance(moved),
            Delivery::Stages(vec![over(7.0, 9.0)], [path].into()),
            "once the files are known each stage passes straight through"
        );
        assert!(!session.finished());
    }

    #[test]
    fn a_drop_before_the_files_arrive_is_delivered_with_them() {
        let (path, uri) = file_uri("late.txt");
        // X11 names no position on entry and answers the fetch after the drop.
        let mut session = DragSession::new(ID);
        session.advance(DragSignal::Entered {
            id: ID,
            position: None,
        });
        session.advance(DragSignal::Moved {
            id: ID,
            position: at(3.0, 5.0),
        });
        assert_eq!(
            session.advance(DragSignal::Dropped { id: ID }),
            Delivery::Nothing
        );
        assert!(
            !session.finished(),
            "a drop waiting on its files is not over"
        );
        let value = UriList::arriving(&[&uri]);
        assert_eq!(
            session.advance(DragSignal::Received {
                id: ID,
                value: &value,
            }),
            Delivery::Stages(
                vec![
                    over(3.0, 5.0),
                    DragStage::Drop {
                        physical_x: 3.0,
                        physical_y: 5.0,
                    },
                ],
                [path].into(),
            )
        );
        assert!(session.finished());
    }

    #[test]
    fn a_drag_naming_no_local_file_is_refused_and_never_reaches_the_document() {
        let mut session = DragSession::new(ID);
        session.advance(DragSignal::Moved {
            id: ID,
            position: at(1.0, 1.0),
        });
        let value = UriList::arriving(&["https://example.com/page", "relative/name.txt"]);
        assert_eq!(
            session.advance(DragSignal::Received {
                id: ID,
                value: &value,
            }),
            Delivery::Refuse
        );
        let moved = DragSignal::Moved {
            id: ID,
            position: at(2.0, 2.0),
        };
        assert_eq!(session.advance(moved), Delivery::Nothing);
        assert_eq!(
            session.advance(DragSignal::Left { id: ID }),
            Delivery::Nothing
        );
        assert!(session.finished());
    }

    #[test]
    fn a_drag_that_leaves_before_its_files_arrive_tells_the_document_nothing() {
        let (_, uri) = file_uri("gone.txt");
        let mut session = DragSession::new(ID);
        session.advance(DragSignal::Moved {
            id: ID,
            position: at(1.0, 1.0),
        });
        assert_eq!(
            session.advance(DragSignal::Left { id: ID }),
            Delivery::Nothing
        );
        let value = UriList::arriving(&[&uri]);
        let Delivery::Stages(stages, _) = session.advance(DragSignal::Received {
            id: ID,
            value: &value,
        }) else {
            panic!("late files still settle the session");
        };
        assert!(stages.is_empty());
        assert!(session.finished());
    }

    #[test]
    fn a_read_that_would_block_is_retried_on_the_next_event() {
        let (path, uri) = file_uri("blocked.txt");
        let mut session = DragSession::new(ID);
        let value: Arc<dyn TypedData> = Arc::new(UriList {
            uris: vec![uri],
            blocks_once: AtomicBool::new(true),
        });
        let received = DragSignal::Received {
            id: ID,
            value: &value,
        };
        assert_eq!(session.advance(received), Delivery::Nothing);
        let moved = DragSignal::Moved {
            id: ID,
            position: at(2.0, 3.0),
        };
        assert_eq!(
            session.advance(moved),
            Delivery::Stages(vec![over(2.0, 3.0)], [path].into())
        );
    }

    #[test]
    fn a_uri_list_names_files_by_url_or_by_bare_absolute_path() {
        let (spaced, spaced_uri) = file_uri("a b.txt");
        let bare = absolute("bare.txt");
        let uris = [
            spaced_uri,
            bare.to_str()
                .expect("a temporary path spells in UTF-8")
                .to_owned(),
            "https://example.com/a.txt".to_owned(),
            "relative.txt".to_owned(),
        ];
        assert_eq!(local_paths(&uris), [spaced, bare]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_finder_file_reference_url_resolves_to_the_path_it_refers_to() {
        use objc2_foundation::{NSString, NSURL};

        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = directory.path().join("referenced file.txt");
        std::fs::write(&file, "referenced").expect("the file is written");
        let file = file.canonicalize().expect("the file exists");
        let reference = NSURL::fileURLWithPath(&NSString::from_str(
            file.to_str().expect("a temporary path spells in UTF-8"),
        ))
        .fileReferenceURL()
        .and_then(|url| url.absoluteString())
        .expect("a file on disk has a reference URL")
        .to_string();
        assert!(reference.starts_with("file:///.file/id="), "{reference}");
        assert_eq!(local_paths(&[reference]), [file]);
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
