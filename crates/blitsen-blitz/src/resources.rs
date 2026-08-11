//! Subresource loading for images, stylesheets and web fonts.
//!
//! Blitz decodes and paints these itself; what it does not do is remember what
//! happened to a request. A failed fetch drops its handler silently, which
//! leaves an `<img>` in exactly the state it had while still loading. Since
//! `complete` and the `load`/`error` pair have to tell those apart,
//! [`ResourceLog`] records every request's outcome as it settles.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use blitz::traits::net::{Bytes, NetHandler, NetProvider, Request, Url};

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

#[derive(Default)]
struct ResourceLogState {
    urls: Mutex<HashMap<String, ResourceState>>,
    /// Bumped whenever a request settles, so a caller can tell that resources
    /// landed during a span of work without diffing the whole map.
    settlements: AtomicU64,
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

    fn requested(&self, url: &str) {
        self.0
            .urls
            .lock()
            .expect("resource log")
            .entry(url.to_owned())
            .or_insert(ResourceState::Loading);
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
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_owned();
        self.log.requested(&url);
        self.inner.fetch(
            doc_id,
            request,
            Box::new(TrackedHandler {
                url,
                log: self.log.clone(),
                inner: Some(handler),
            }),
        );
    }

    fn is_noop(&self) -> bool {
        self.inner.is_noop()
    }
}

/// Marks a request loaded when bytes arrive, and failed when they never do.
///
/// A provider signals failure by dropping the handler, so the drop is the only
/// notification there is.
struct TrackedHandler {
    url: String,
    log: ResourceLog,
    inner: Option<Box<dyn NetHandler>>,
}

impl NetHandler for TrackedHandler {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: Bytes) {
        self.log.settled(&self.url, ResourceState::Loaded);
        if let Some(inner) = self.inner.take() {
            inner.bytes(resolved_url, bytes);
        }
    }
}

impl Drop for TrackedHandler {
    fn drop(&mut self) {
        if self.inner.is_some() {
            self.log.settled(&self.url, ResourceState::Failed);
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
        if let Some(bytes) = read(&request.url) {
            handler.bytes(url, Bytes::from(bytes));
        }
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

        assert_eq!(
            collector.0.lock().unwrap().as_slice(),
            [(data.to_owned(), 5)]
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
        struct NeverAnswers(Mutex<Vec<Box<dyn NetHandler>>>);
        impl NetProvider for NeverAnswers {
            fn fetch(&self, _doc_id: usize, _request: Request, handler: Box<dyn NetHandler>) {
                // Held rather than dropped: an in-flight request is neither
                // loaded nor failed until the provider decides.
                self.0.lock().unwrap().push(handler);
            }
        }

        let (provider, log) = track(Some(Arc::new(NeverAnswers(Mutex::new(Vec::new())))));
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
}
