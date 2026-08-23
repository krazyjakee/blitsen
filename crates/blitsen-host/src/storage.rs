//! Durable Web Storage without a database dependency (issue #91).
//!
//! Each value is one keyed file and the ordered key list is a small index. A
//! write is flushed to a temporary sibling and atomically renamed into place,
//! so interruption leaves either the old value or the new one. A directory lock
//! serialises processes sharing an application identity; every operation reads
//! the current index after taking it, so one process does not overwrite keys a
//! second process just added.

use std::collections::HashSet;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const VERSION: u8 = 1;
const INDEX: &str = "index.json";
const ITEMS: &str = "items";
const LOCK: &str = ".lock";
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_LOCK: Duration = Duration::from_secs(30);
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
/// One application's durable, synchronously accessed localStorage area.
pub struct LocalStorage {
    root: PathBuf,
    process_lock: Arc<Mutex<()>>,
}

#[derive(Deserialize, Serialize)]
struct Index {
    version: u8,
    keys: Vec<String>,
    next_order: u64,
}

#[derive(Deserialize, Serialize)]
struct Item<'a> {
    version: u8,
    order: u64,
    key: &'a str,
    value: &'a str,
}

#[derive(Deserialize)]
struct OwnedItem {
    version: u8,
    order: u64,
    key: String,
    value: String,
}

enum ReadItemError {
    Io(String),
    Corrupt,
}

impl LocalStorage {
    /// Opens a store below an injected application-data directory.
    pub fn open(data_directory: impl Into<PathBuf>, identity: &str) -> Result<Self, String> {
        let root = data_directory
            .into()
            .join(namespace(identity))
            .join("local-storage");
        fs::create_dir_all(root.join(ITEMS)).map_err(|error| {
            format!(
                "could not create localStorage directory {}: {error}",
                root.display()
            )
        })?;
        let storage = Self {
            root,
            process_lock: Arc::new(Mutex::new(())),
        };
        storage.with_lock(|storage| storage.read_index().map(drop))?;
        Ok(storage)
    }

    /// Opens the store under the platform application-data directory.
    #[cfg(not(target_os = "android"))]
    pub fn for_application(identity: &str) -> Result<Self, String> {
        let data =
            blitsen_platform::app::directory(blitsen_platform::app::Directory::Data, "Blitsen")
                .map_err(|error| error.message().to_owned())?;
        Self::open(data, identity)
    }

    /// Android's files directory is already isolated by installed application.
    #[cfg(target_os = "android")]
    pub fn for_application(identity: &str) -> Result<Self, String> {
        Self::open(crate::native_window::notify::files_directory()?, identity)
    }

    /// Returns keys in insertion order.
    pub fn keys(&self) -> Result<Vec<String>, String> {
        self.with_lock(|storage| storage.read_index().map(|index| index.keys))
    }

    /// Reads one value without loading the rest of the store.
    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.with_lock(|storage| {
            let mut index = storage.read_index()?;
            if !index.keys.iter().any(|candidate| candidate == key) {
                return Ok(None);
            }
            match storage.read_item(key) {
                Ok(item) => Ok(Some(item.value)),
                Err(ReadItemError::Corrupt) => {
                    index.keys.retain(|candidate| candidate != key);
                    storage.write_index(&index)?;
                    storage.quarantine(&storage.item_path(key));
                    Ok(None)
                }
                Err(ReadItemError::Io(error)) => Err(error),
            }
        })
    }

    /// Atomically creates or replaces one string value.
    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.with_lock(|storage| {
            let mut index = storage.read_index()?;
            let existing = index.keys.iter().any(|candidate| candidate == key);
            let order = if existing {
                storage.read_item(key).map_or_else(
                    |_| {
                        index
                            .keys
                            .iter()
                            .position(|candidate| candidate == key)
                            .unwrap_or(0) as u64
                            + 1
                    },
                    |item| item.order,
                )
            } else {
                index.next_order
            };
            let bytes = serde_json::to_vec(&Item {
                version: VERSION,
                order,
                key,
                value,
            })
            .map_err(|error| format!("could not encode localStorage value: {error}"))?;
            storage.atomic_write(&storage.item_path(key), &bytes)?;
            if !existing {
                index.keys.push(key.to_owned());
                index.next_order = index.next_order.saturating_add(1);
                storage.write_index(&index)?;
            }
            Ok(())
        })
    }

    /// Removes one key if present.
    pub fn remove(&self, key: &str) -> Result<(), String> {
        self.with_lock(|storage| {
            let mut index = storage.read_index()?;
            let before = index.keys.len();
            index.keys.retain(|candidate| candidate != key);
            if index.keys.len() != before {
                storage.write_index(&index)?;
                ignore_missing(fs::remove_file(storage.item_path(key)))?;
            }
            Ok(())
        })
    }

    /// Removes every key in the application area.
    pub fn clear(&self) -> Result<(), String> {
        self.with_lock(|storage| {
            storage.write_index(&Index {
                version: VERSION,
                keys: Vec::new(),
                next_order: 1,
            })?;
            for entry in fs::read_dir(storage.root.join(ITEMS))
                .map_err(|error| format!("could not read localStorage items: {error}"))?
                .flatten()
            {
                if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    ignore_missing(fs::remove_file(entry.path()))?;
                }
            }
            Ok(())
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let _process = self
            .process_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _directory = DirectoryLock::acquire(&self.root)?;
        operation(self)
    }

    fn read_index(&self) -> Result<Index, String> {
        match fs::read(self.root.join(INDEX)) {
            Ok(bytes) => match serde_json::from_slice::<Index>(&bytes) {
                Ok(index)
                    if index.version == VERSION && index.next_order > 0 && unique(&index.keys) =>
                {
                    Ok(index)
                }
                _ => {
                    self.quarantine(&self.root.join(INDEX));
                    self.rebuild_index()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.rebuild_index(),
            Err(error) => Err(format!("could not read localStorage index: {error}")),
        }
    }

    fn rebuild_index(&self) -> Result<Index, String> {
        let mut items = Vec::new();
        for entry in fs::read_dir(self.root.join(ITEMS))
            .map_err(|error| format!("could not rebuild localStorage index: {error}"))?
            .flatten()
        {
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            let Ok(item) = serde_json::from_slice::<OwnedItem>(&bytes) else {
                self.quarantine(&entry.path());
                continue;
            };
            if item.version == VERSION && self.item_path(&item.key) == entry.path() {
                items.push((item.order, item.key));
            } else {
                self.quarantine(&entry.path());
            }
        }
        items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let next_order = items.last().map_or(1, |(order, _)| order.saturating_add(1));
        let index = Index {
            version: VERSION,
            keys: items.into_iter().map(|(_, key)| key).collect(),
            next_order,
        };
        self.write_index(&index)?;
        Ok(index)
    }

    fn read_item(&self, key: &str) -> Result<OwnedItem, ReadItemError> {
        let bytes = fs::read(self.item_path(key)).map_err(|error| {
            ReadItemError::Io(format!("could not read localStorage key {key:?}: {error}"))
        })?;
        let item: OwnedItem = serde_json::from_slice(&bytes).map_err(|_| ReadItemError::Corrupt)?;
        if item.version != VERSION || item.key != key {
            return Err(ReadItemError::Corrupt);
        }
        Ok(item)
    }

    fn write_index(&self, index: &Index) -> Result<(), String> {
        let bytes = serde_json::to_vec(index)
            .map_err(|error| format!("could not encode localStorage index: {error}"))?;
        self.atomic_write(&self.root.join(INDEX), &bytes)
    }

    fn atomic_write(&self, target: &Path, bytes: &[u8]) -> Result<(), String> {
        let temporary = self.root.join(format!(
            ".write-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| format!("could not create localStorage write: {error}"))?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("could not flush localStorage write: {error}"))?;
            fs::rename(&temporary, target)
                .map_err(|error| format!("could not commit localStorage write: {error}"))?;
            sync_directory(&self.root)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn item_path(&self, key: &str) -> PathBuf {
        self.root
            .join(ITEMS)
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
    }

    fn quarantine(&self, path: &Path) {
        if !path.exists() {
            return;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record");
        let quarantined = self.root.join(format!(
            ".corrupt-{name}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::rename(path, quarantined);
    }

    #[cfg(test)]
    fn root(&self) -> &Path {
        &self.root
    }
}

fn namespace(identity: &str) -> String {
    format!("app-{:x}", Sha256::digest(identity.as_bytes()))
}

fn unique(keys: &[String]) -> bool {
    let mut seen = HashSet::new();
    keys.iter().all(|key| seen.insert(key))
}

fn ignore_missing(result: std::io::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove localStorage record: {error}")),
    }
}

// The file itself is fsynced on every platform. Unix also permits opening and
// syncing the containing directory, which makes the rename durable across a
// power loss. std does not expose Windows' directory-handle flags, so asking
// File::open there would make every otherwise-valid Web Storage write fail.
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not flush localStorage directory: {error}"))
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

struct DirectoryLock {
    path: PathBuf,
}

impl DirectoryLock {
    fn acquire(root: &Path) -> Result<Self, String> {
        let path = root.join(LOCK);
        let started = std::time::Instant::now();
        loop {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if lock_is_contended(&error) => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        let stale_path = root.join(format!(
                            ".stale-lock-{}-{}",
                            std::process::id(),
                            TEMP_ID.fetch_add(1, Ordering::Relaxed)
                        ));
                        if fs::rename(&path, &stale_path).is_ok() {
                            let _ = fs::remove_dir_all(stale_path);
                        }
                        continue;
                    }
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err("timed out waiting for another localStorage writer".to_owned());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(format!("could not lock localStorage: {error}")),
            }
        }
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return true;
    }

    // Windows can report ERROR_ACCESS_DENIED while another thread is deleting
    // the just-released lock directory. That is transient contention, not a
    // refusal to use the application's storage directory. Retrying also keeps
    // the operation serialized across independently opened LocalStorage areas.
    #[cfg(windows)]
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }

    false
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use blitsen_blitz::BlitzDom;
    use blitsen_js::{JsEngine, JsError};
    use blitsen_quickjs::QuickJs;

    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "blitsen-storage-{name}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn in_realm(storage: LocalStorage, script: &str) -> Result<(), JsError> {
        let mut engine = QuickJs::new()?;
        let _services = crate::runtime_services::RuntimeServices::install(&mut engine)?;
        let runtime =
            crate::DomRuntime::new(BlitzDom::from_html("<body></body>", Default::default()));
        crate::dom_bridge::install(
            &mut engine,
            runtime,
            crate::dom_bridge::InstallOptions::new(
                320,
                240,
                1.0,
                crate::dom_bridge::DocumentMode::TestHarness,
                None,
            )
            .with_storage(storage),
        )?;
        engine.evaluate_script(script, "storage-test.js")?;
        Ok(())
    }

    #[test]
    fn injected_directory_reopens_the_same_application_and_separates_another() {
        let root = directory("reopen");
        LocalStorage::open(&root, "app-a")
            .unwrap()
            .set("theme", "dark")
            .unwrap();
        assert_eq!(
            LocalStorage::open(&root, "app-a")
                .unwrap()
                .get("theme")
                .unwrap(),
            Some("dark".into())
        );
        assert_eq!(
            LocalStorage::open(&root, "app-b")
                .unwrap()
                .get("theme")
                .unwrap(),
            None
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_corrupt_index_is_rebuilt_and_a_corrupt_value_is_removed() {
        let root = directory("corrupt");
        let storage = LocalStorage::open(&root, "app").unwrap();
        storage.set("good", "kept").unwrap();
        storage.set("bad", "lost").unwrap();
        fs::write(storage.root().join(INDEX), "not json").unwrap();
        fs::write(storage.item_path("bad"), "not json").unwrap();
        let reopened = LocalStorage::open(&root, "app").unwrap();
        assert_eq!(reopened.get("good").unwrap(), Some("kept".into()));
        assert_eq!(reopened.get("bad").unwrap(), None);
        assert_eq!(reopened.keys().unwrap(), vec!["good"]);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sustained_writes_replace_one_value_without_growing_the_key_set() {
        let root = directory("writes");
        let storage = LocalStorage::open(&root, "app").unwrap();
        for value in 0..500 {
            storage.set("sample", &value.to_string()).unwrap();
        }
        assert_eq!(storage.get("sample").unwrap(), Some("499".into()));
        assert_eq!(storage.keys().unwrap(), vec!["sample"]);
        assert_eq!(fs::read_dir(storage.root().join(ITEMS)).unwrap().count(), 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn independently_opened_writers_do_not_lose_each_others_keys() {
        let root = directory("concurrent");
        let first = LocalStorage::open(&root, "app").unwrap();
        let second = LocalStorage::open(&root, "app").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer =
            |prefix: &'static str, storage: LocalStorage, barrier: Arc<std::sync::Barrier>| {
                std::thread::spawn(move || {
                    barrier.wait();
                    for index in 0..20 {
                        storage
                            .set(&format!("{prefix}-{index}"), &index.to_string())
                            .unwrap();
                    }
                })
            };
        let left = writer("left", first, Arc::clone(&barrier));
        let right = writer("right", second, barrier);
        left.join().unwrap();
        right.join().unwrap();

        let reopened = LocalStorage::open(&root, "app").unwrap();
        assert_eq!(reopened.keys().unwrap().len(), 40);
        assert_eq!(reopened.get("left-19").unwrap(), Some("19".into()));
        assert_eq!(reopened.get("right-19").unwrap(), Some("19".into()));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn key_order_remove_clear_and_string_values_match_web_storage() {
        let root = directory("parity");
        let storage = LocalStorage::open(&root, "app").unwrap();
        storage.set("theme", "dark").unwrap();
        storage.set("count", "2").unwrap();
        storage.set("theme", "light").unwrap();
        assert_eq!(storage.keys().unwrap(), vec!["theme", "count"]);
        assert_eq!(storage.get("theme").unwrap(), Some("light".into()));
        storage.remove("theme").unwrap();
        assert_eq!(storage.keys().unwrap(), vec!["count"]);
        storage.clear().unwrap();
        assert!(storage.keys().unwrap().is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn javascript_property_and_method_keys_share_the_durable_store_but_not_the_session() {
        let root = directory("javascript-parity");
        let storage = LocalStorage::open(&root, "app").unwrap();
        in_realm(
            storage.clone(),
            r#"
              localStorage.clear();
              localStorage.setItem("theme", "dark");
              localStorage.count = 2;
              sessionStorage.setItem("ephemeral", "yes");
              if (localStorage.theme !== "dark" || localStorage.getItem("count") !== "2"
                  || localStorage.length !== 2 || localStorage.key(0) !== "theme"
                  || !("count" in localStorage)) throw new Error("Storage key parity failed: "
                    + JSON.stringify({ theme: localStorage.theme, count: localStorage.getItem("count"),
                      length: localStorage.length, first: localStorage.key(0),
                      has: "count" in localStorage }));
            "#,
        )
        .unwrap();
        assert_eq!(storage.keys().unwrap(), vec!["theme", "count"]);
        in_realm(
            LocalStorage::open(&root, "app").unwrap(),
            r#"
              if (localStorage.theme !== "dark" || localStorage.count !== "2")
                throw new Error("localStorage did not survive a realm");
              if (sessionStorage.getItem("ephemeral") !== null)
                throw new Error("sessionStorage escaped its realm");
              delete localStorage.theme;
              if (localStorage.getItem("theme") !== null || Object.keys(localStorage).join() !== "count")
                throw new Error("property deletion diverged from removeItem");
            "#,
        )
        .unwrap();
        fs::remove_dir_all(root).ok();
    }
}
