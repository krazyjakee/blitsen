//! Subresource loading for images, stylesheets and web fonts.
//!
//! Blitz decodes and paints these itself; what it does not do is remember what
//! happened to a request. A failed fetch drops its handler silently, which
//! leaves an `<img>` in exactly the state it had while still loading. Since
//! `complete` and the `load`/`error` pair have to tell those apart,
//! [`ResourceLog`] records every request's outcome as it settles.
//!
//! Holding the handler as well as the outcome is what makes `window.stop()`
//! real: [`ResourceLog::stop`] cancels the transfers and settles what they were
//! loading, rather than leaving the document waiting on requests it abandoned.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use blitz::traits::net::{
    AbortController, AbortSignal, Bytes, NetHandler, NetProvider, Request, Url,
};

/// Outcome of one subresource request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceState {
    /// Requested; no bytes have arrived yet.
    Loading,
    /// The bytes arrived. Whether they decoded is a separate question.
    Loaded,
    /// The request finished without producing bytes.
    Failed,
}

/// A request that has not settled yet, shared between its handler and the log.
struct Inflight {
    url: String,
    /// Taken by whichever of arrival, abort or drop reaches it first, so the
    /// request Blitz is waiting on is completed exactly once.
    handler: Mutex<Option<Box<dyn NetHandler>>>,
}

#[derive(Default)]
struct ResourceLogState {
    urls: Mutex<HashMap<String, ResourceState>>,
    /// Bumped whenever a request settles, so a caller can tell that resources
    /// landed during a span of work without diffing the whole map.
    settlements: AtomicU64,
    inflight: Mutex<HashMap<u64, Arc<Inflight>>>,
    next_request: AtomicU64,
    /// Cancels the requests of the current load. Replaced by every stop, so a
    /// request the document starts afterwards is not born aborted: `stop()`
    /// ends the load in progress, not the document's ability to load.
    controller: Mutex<AbortController>,
}

/// Shared record of what has been requested for one document, and how it went.
///
/// Cloning shares the record. The log is written from whatever thread the net
/// provider answers on, so every field is behind a lock or an atomic.
#[derive(Clone, Default)]
pub struct ResourceLog(Arc<ResourceLogState>);

impl ResourceLog {
    /// Returns what is known about a resolved subresource URL.
    ///
    /// `None` means the document never asked for it.
    pub fn state(&self, url: &str) -> Option<ResourceState> {
        self.0.urls.lock().expect("resource log").get(url).copied()
    }

    /// Counts settled requests, so callers can detect arrivals across a span.
    pub fn settlements(&self) -> u64 {
        self.0.settlements.load(Ordering::Relaxed)
    }

    /// Aborts every request still in flight, and reports how many that was.
    ///
    /// This is `window.stop()` at the renderer. Two things have to happen and
    /// neither is enough alone. The signal cancels the transfers, so the bytes
    /// stop arriving; and every abandoned handler is *completed* with an empty
    /// body, because Blitz holds a stylesheet as a pending critical resource
    /// until its handler completes and a dropped handler never completes. A
    /// stop that only cancelled would block painting for the life of the
    /// document, which is the opposite of what the caller asked for.
    ///
    /// With nothing in flight this walks an empty table and returns zero: a
    /// no-op in effect rather than a no-op implementation.
    pub fn stop(&self) -> usize {
        let controller = std::mem::take(&mut *self.0.controller.lock().expect("resource log"));
        controller.abort();
        let inflight = std::mem::take(&mut *self.0.inflight.lock().expect("resource log"));
        let mut aborted = 0;
        for entry in inflight.into_values() {
            // No lock is held across the completion: an empty stylesheet can
            // still ask for the resources of an `@import` it never read.
            let handler = entry.handler.lock().expect("resource log").take();
            let Some(handler) = handler else { continue };
            self.settled(&entry.url, ResourceState::Failed);
            handler.bytes(entry.url.clone(), Bytes::new());
            aborted += 1;
        }
        aborted
    }

    /// The signal every request of the current load carries.
    fn signal(&self) -> AbortSignal {
        self.0
            .controller
            .lock()
            .expect("resource log")
            .signal
            .clone()
    }

    /// Registers a request as in flight, holding its handler so a stop can
    /// settle what it was loading.
    fn requested(&self, url: &str, handler: Box<dyn NetHandler>) -> (u64, Arc<Inflight>) {
        self.0
            .urls
            .lock()
            .expect("resource log")
            .entry(url.to_owned())
            .or_insert(ResourceState::Loading);
        let entry = Arc::new(Inflight {
            url: url.to_owned(),
            handler: Mutex::new(Some(handler)),
        });
        let id = self.0.next_request.fetch_add(1, Ordering::Relaxed);
        self.0
            .inflight
            .lock()
            .expect("resource log")
            .insert(id, Arc::clone(&entry));
        (id, entry)
    }

    /// Claims the right to settle a request, or `None` if a stop got there first.
    fn claim(&self, id: u64, entry: &Inflight) -> Option<Box<dyn NetHandler>> {
        self.0.inflight.lock().expect("resource log").remove(&id);
        entry.handler.lock().expect("resource log").take()
    }

    fn settled(&self, url: &str, state: ResourceState) {
        self.0
            .urls
            .lock()
            .expect("resource log")
            .insert(url.to_owned(), state);
        self.0.settlements.fetch_add(1, Ordering::Relaxed);
    }
}

/// Records the outcome of every request made through an inner provider.
struct TrackedProvider {
    inner: Arc<dyn NetProvider>,
    log: ResourceLog,
}

impl NetProvider for TrackedProvider {
    fn fetch(&self, doc_id: usize, mut request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        let (id, entry) = self.log.requested(&url, handler);
        // Stamped here rather than through `DocumentConfig::abort_signal`,
        // which cannot be replaced once the document holds it: a stop has to
        // cancel this load without disabling the next one.
        request.signal = Some(self.log.signal());
        self.inner.fetch(
            doc_id,
            request,
            Box::new(TrackedHandler {
                id,
                entry,
                log: self.log.clone(),
            }),
        );
    }

    fn is_noop(&self) -> bool {
        self.inner.is_noop()
    }
}

/// Marks a request loaded when bytes arrive, and failed otherwise.
///
/// Two things count as failure. A provider can drop the handler, which is the
/// only failure notification the trait has. Or it can answer with nothing:
/// [`LocalResources`] does that for a resource it will not serve, because a
/// dropped handler leaves a stylesheet pending and blocks painting forever. An
/// empty body is indistinguishable from a failure here, and treating it as one
/// is what makes a broken `<img>` reach its errored state — an empty stylesheet
/// has nothing to contribute either way.
///
/// A request the log already settled — through [`ResourceLog::stop`] — arrives
/// to nothing: the answer it was overtaken by is the one the document has.
struct TrackedHandler {
    id: u64,
    entry: Arc<Inflight>,
    log: ResourceLog,
}

impl NetHandler for TrackedHandler {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let Some(inner) = self.log.claim(self.id, &self.entry) else {
            return;
        };
        let state = if bytes.is_empty() {
            ResourceState::Failed
        } else {
            ResourceState::Loaded
        };
        self.log.settled(&self.entry.url, state);
        inner.bytes(resolved_url, bytes);
    }
}

impl Drop for TrackedHandler {
    fn drop(&mut self) {
        if self.log.claim(self.id, &self.entry).is_some() {
            self.log.settled(&self.entry.url, ResourceState::Failed);
        }
    }
}

/// Wraps a document's net provider so its request outcomes become observable.
///
/// Installs [`local`] when the configuration carries no provider of its own.
pub(crate) fn track(provider: Option<Arc<dyn NetProvider>>) -> (Arc<dyn NetProvider>, ResourceLog) {
    let log = ResourceLog::default();
    let inner = provider.unwrap_or_else(|| Arc::new(LocalResources) as Arc<dyn NetProvider>);
    let tracked = TrackedProvider {
        inner,
        log: log.clone(),
    };
    (Arc::new(tracked) as Arc<dyn NetProvider>, log)
}

/// Resolves `file:` and `data:` subresources synchronously, on the calling thread.
///
/// A windowed host answers subresource requests through its event loop, which a
/// headless harness does not run. Without a provider the document parses with
/// its stylesheets, fonts and images missing, so every assertion silently
/// measures an unstyled, textless, imageless box. This one hands the bytes back
/// before `fetch` returns, so the document is complete by the time parsing is.
///
/// Remote subresources are refused rather than fetched: an offline harness that
/// silently skipped them would report a different frame than a machine that
/// could reach the network.
pub struct LocalResources;

impl NetProvider for LocalResources {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        // Answer even when there is nothing to serve. Blitz holds a stylesheet as
        // a pending critical resource until its handler completes, and a handler
        // that is only dropped never completes — so a refused remote sheet, or a
        // local one that is missing, blocks painting for the life of the document
        // rather than degrading. Empty bytes are a real answer: an empty
        // stylesheet contributes nothing, and an empty image fails to decode into
        // the errored state `complete` already reports.
        handler.bytes(url, read(&request.url).map(Bytes::from).unwrap_or_default());
    }
}

fn read(url: &Url) -> Option<Vec<u8>> {
    match url.scheme() {
        "file" => url
            .to_file_path()
            .ok()
            .and_then(|path| std::fs::read(path).ok()),
        "data" => data_url::DataUrl::process(url.as_str())
            .ok()
            .and_then(|data| data.decode_to_vec().ok())
            .map(|(bytes, _)| bytes),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Clone, Default)]
    struct Collector(Arc<Mutex<Vec<(String, usize)>>>);

    impl NetHandler for Collector {
        fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
            self.0.lock().unwrap().push((resolved_url, bytes.len()));
        }
    }

    fn fetch(provider: &dyn NetProvider, url: &str, collector: &Collector) {
        provider.fetch(
            0,
            Request::get(Url::parse(url).unwrap()),
            Box::new(collector.clone()),
        );
    }

    /// Holds every request rather than answering it: an in-flight request is
    /// neither loaded nor failed until the provider decides.
    #[derive(Default)]
    struct NeverAnswers(Mutex<Vec<(Request, Box<dyn NetHandler>)>>);

    impl NetProvider for NeverAnswers {
        fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
            self.0.lock().unwrap().push((request, handler));
        }
    }

    impl NeverAnswers {
        /// Answers everything held, the way a transfer already on the wire when
        /// a stop landed would.
        fn deliver(&self) {
            let held = self.0.lock().unwrap().drain(..).collect::<Vec<_>>();
            for (request, handler) in held {
                handler.bytes(request.url.as_str().to_owned(), Bytes::from_static(b"late"));
            }
        }

        fn aborted(&self) -> Vec<bool> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|(request, _)| request.signal.as_ref().is_some_and(AbortSignal::aborted))
                .collect()
        }
    }

    #[test]
    fn local_resources_answer_data_and_file_urls_and_refuse_the_network() {
        let collector = Collector::default();
        let (provider, log) = track(None);
        let data = "data:text/plain;base64,aGVsbG8=";
        fetch(&*provider, data, &collector);
        fetch(&*provider, "https://example.com/a.png", &collector);
        fetch(
            &*provider,
            "file:///nonexistent-blitsen-fixture.png",
            &collector,
        );

        // Everything is answered, including what cannot be served: Blitz holds a
        // stylesheet pending until its handler completes, so a dropped handler
        // blocks painting for the life of the document.
        assert_eq!(
            collector.0.lock().unwrap().as_slice(),
            [
                (data.to_owned(), 5),
                ("https://example.com/a.png".to_owned(), 0),
                ("file:///nonexistent-blitsen-fixture.png".to_owned(), 0),
            ]
        );
        assert_eq!(log.state(data), Some(ResourceState::Loaded));
        assert_eq!(
            log.state("https://example.com/a.png"),
            Some(ResourceState::Failed),
            "a refused remote request settles as failed, not as forever-loading"
        );
        assert_eq!(
            log.state("file:///nonexistent-blitsen-fixture.png"),
            Some(ResourceState::Failed)
        );
        assert_eq!(log.state("file:///never-asked-for"), None);
        assert_eq!(log.settlements(), 3);
    }

    #[test]
    fn a_request_that_has_not_answered_yet_stays_loading() {
        let (provider, log) = track(Some(Arc::new(NeverAnswers::default())));
        provider.fetch(
            0,
            Request::get(Url::parse("https://example.com/slow.woff2").unwrap()),
            Box::new(Collector::default()),
        );
        assert_eq!(
            log.state("https://example.com/slow.woff2"),
            Some(ResourceState::Loading)
        );
        assert_eq!(log.settlements(), 0);
    }

    #[test]
    fn stopping_cancels_every_transfer_in_flight_and_settles_what_it_was_loading() {
        let collector = Collector::default();
        let network = Arc::new(NeverAnswers::default());
        let (provider, log) = track(Some(Arc::clone(&network) as Arc<dyn NetProvider>));
        let sheet = "https://example.com/app.css";
        let font = "https://example.com/app.woff2";
        fetch(&*provider, sheet, &collector);
        fetch(&*provider, font, &collector);
        assert_eq!(log.state(sheet), Some(ResourceState::Loading));
        assert_eq!(network.aborted(), [false, false]);

        assert_eq!(log.stop(), 2);
        assert_eq!(
            network.aborted(),
            [true, true],
            "the transfer is cancelled, not merely forgotten"
        );
        // Completed rather than dropped: a handler that is only dropped never
        // completes, and Blitz waits on it for the life of the document. The
        // order requests are abandoned in is not defined.
        let mut answered = collector.0.lock().unwrap().clone();
        answered.sort();
        assert_eq!(answered, [(sheet.to_owned(), 0), (font.to_owned(), 0)]);
        assert_eq!(log.state(sheet), Some(ResourceState::Failed));
        assert_eq!(log.state(font), Some(ResourceState::Failed));
        assert_eq!(log.settlements(), 2);

        // Bytes that were already on the wire arrive to a settled document, and
        // must not reopen what the stop closed.
        network.deliver();
        assert_eq!(log.state(sheet), Some(ResourceState::Failed));
        assert_eq!(log.settlements(), 2);
        assert_eq!(collector.0.lock().unwrap().len(), 2);

        // A request the document starts afterwards is a new load: stopping ends
        // the load in progress, not the document's ability to load.
        fetch(&*provider, "https://example.com/late.png", &collector);
        assert_eq!(network.aborted(), [false]);
        assert_eq!(log.stop(), 1);
    }

    #[test]
    fn stopping_with_nothing_in_flight_settles_nothing_and_disturbs_nothing() {
        let collector = Collector::default();
        let (provider, log) = track(None);
        let data = "data:text/plain;base64,aGVsbG8=";
        fetch(&*provider, data, &collector);

        assert_eq!(log.stop(), 0);
        assert_eq!(log.stop(), 0);
        assert_eq!(
            log.state(data),
            Some(ResourceState::Loaded),
            "a load that already finished is not something a stop can undo"
        );
        assert_eq!(log.settlements(), 1);
        assert_eq!(collector.0.lock().unwrap().len(), 1);
    }
}
