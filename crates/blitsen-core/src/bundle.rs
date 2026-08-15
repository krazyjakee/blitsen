//! The application bundle appended to a Phase 2 executable (issue #88).
//!
//! Step ④ of the export pipeline links the runtime and the application into one
//! file. Phase 1 does that with `bun build --compile`, which carries a whole Bun
//! runtime; Phase 2 appends the collected output to Blitsen's own executable as
//! a section this module reads.
//!
//! # Layout
//!
//! ```text
//! [ runtime executable ][ payload ][ trailer ][ code signature, if any ]
//! ```
//!
//! The payload opens with a version header, so anything that finds it can tell
//! what it is holding. The trailer records where the payload starts and what it
//! hashes to, and ends with a magic number so it can be found again.
//!
//! ```text
//! payload  0..8   magic      b"BLITSEN\0"
//!          8..12  version    u32
//!         12..16  flags      u32
//!         16..20  entries    u32
//!         20..24  index_len  u32
//!         24..32  data_len   u64
//!         32..    index, then file data
//!
//! entry    0..4   path_len   u32
//!          4..12  offset     u64, from the start of the data region
//!         12..20  length     u64
//!         20..    path       UTF-8, `/`-separated, relative to the app root
//!
//! trailer  0..32  digest     SHA-256 of the payload
//!         32..40  offset     u64, from the start of the file
//!         40..48  length     u64
//!         48..52  version    u32
//!         52..56  flags      u32
//!         56..64  magic      b"BLITSEN\x1a"
//! ```
//!
//! # Signing
//!
//! **Append first, then sign.** Appending to an already-signed binary is the
//! classic way to break it: a macOS signature covers the file through
//! `__LINKEDIT`, and Authenticode hashes everything outside the certificate
//! table, so bytes added afterwards either invalidate the signature or are
//! silently outside it — which is worse. The export pipeline therefore runs the
//! signing hook last (TECH.md §10, step ⑤), over an executable that already
//! carries its bundle.
//!
//! That ordering is also why the trailer is *found* rather than assumed to be
//! the final bytes: a signature legitimately follows it. The reader takes the
//! trailer at the end when it is there, and otherwise scans backwards for the
//! magic — validating each candidate, because this runtime carries a copy of
//! that magic in its own read-only data.
//!
//! What is verified here is the shape: a linked executable still reads back
//! correctly with arbitrary bytes appended after its trailer, which is what
//! both signing tools do. What is *not* verified is `codesign` and `signtool`
//! themselves — neither runs on the Linux CI this was built on. If either turns
//! out to rewrite the Mach-O or PE rather than only append, the offsets in the
//! trailer move and this reader has to learn that; the first macOS or Windows
//! signing run is where that is found out.
//!
//! # Reading
//!
//! Nothing is unpacked. The index is read once at startup; file contents are
//! read from their recorded offsets on demand, out of the executable itself.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Magic opening the payload.
const PAYLOAD_MAGIC: &[u8; 8] = b"BLITSEN\0";
/// Magic closing the trailer.
const TRAILER_MAGIC: &[u8; 8] = b"BLITSEN\x1a";
/// Size of the fixed payload header.
const HEADER_SIZE: usize = 32;
/// Size of the fixed trailer.
const TRAILER_SIZE: usize = 64;
/// The only format this build writes, and the newest it reads.
pub const FORMAT_VERSION: u32 = 1;
/// Chunk scanned at a time when a signature has been appended after the trailer.
const SCAN_CHUNK: usize = 1 << 20;
/// How far back a trailer may sit behind the end of the file.
///
/// A code signature is the only thing that legitimately follows it, and no
/// signature is anywhere near this large — an Authenticode certificate table is
/// tens of kilobytes, and a notarised macOS signature a few megabytes. Bounding
/// the scan keeps a corrupt or hostile file, or an ordinary unlinked runtime,
/// from turning startup into a full read of a fifty-megabyte executable.
const MAX_TRAILING_BYTES: u64 = 16 << 20;

/// Why a bundle could not be read.
#[derive(Debug)]
pub enum BundleError {
    /// The file could not be read.
    Io(io::Error),
    /// The bundle is structurally invalid, and how.
    Malformed(String),
    /// The bundle was written by a newer Blitsen.
    UnsupportedVersion {
        /// Version recorded in the file.
        found: u32,
        /// Newest version this build understands.
        supported: u32,
    },
    /// The payload does not hash to the digest the trailer records.
    DigestMismatch,
    /// A path was asked for that the bundle does not carry.
    NotFound(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read the application bundle: {error}"),
            Self::Malformed(reason) => write!(formatter, "the application bundle is {reason}"),
            Self::UnsupportedVersion { found, supported } => write!(
                formatter,
                "the application bundle is format version {found}, but this runtime reads at most \
                 {supported}: the executable and the bundle came from different Blitsen releases"
            ),
            Self::DigestMismatch => formatter.write_str(
                "the application bundle does not match its recorded digest: the executable is \
                 damaged or was modified after it was built",
            ),
            Self::NotFound(path) => {
                write!(formatter, "the application bundle has no file named {path}")
            }
        }
    }
}

impl std::error::Error for BundleError {}

impl From<io::Error> for BundleError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn malformed(reason: impl Into<String>) -> BundleError {
    BundleError::Malformed(reason.into())
}

/// One file's position inside the bundle's data region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BundleEntry {
    /// Offset from the start of the file, ready to seek to.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
}

/// An application bundle read in place from the executable carrying it.
pub struct AppBundle {
    // A mutex rather than a cell: the bundle is also the document's subresource
    // provider, which Blitz may call from a worker thread. Contention is not a
    // concern — every hold is one seek and one read.
    file: Mutex<File>,
    entries: BTreeMap<String, BundleEntry>,
    digest: [u8; 32],
    payload: BundleEntry,
}

impl fmt::Debug for AppBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppBundle")
            .field("files", &self.entries.len())
            .field("bytes", &self.payload.length)
            .finish()
    }
}

impl AppBundle {
    /// Opens the bundle appended to `path`, or `None` when it carries none.
    ///
    /// A missing bundle is not an error: the same executable runs a directory
    /// given on the command line, which is how `blitsen run` works.
    pub fn open(path: &Path) -> Result<Option<Self>, BundleError> {
        let mut file = File::open(path)?;
        let size = file.seek(SeekFrom::End(0))?;
        let Some(trailer) = find_trailer(&mut file, size)? else {
            return Ok(None);
        };

        let payload = trailer.payload;
        let mut header = [0_u8; HEADER_SIZE];
        file.seek(SeekFrom::Start(payload.offset))?;
        file.read_exact(&mut header)?;
        let count = read_u32(&header[16..20]) as usize;
        let index_len = read_u32(&header[20..24]) as u64;
        let data_len = read_u64(&header[24..32]);
        if HEADER_SIZE as u64 + index_len + data_len != payload.length {
            return Err(malformed("inconsistent: its parts do not add up"));
        }

        let mut index = vec![0_u8; index_len as usize];
        file.read_exact(&mut index)?;
        let data_start = payload.offset + HEADER_SIZE as u64 + index_len;
        let entries = parse_index(&index, count, data_start, data_len)?;

        Ok(Some(Self {
            file: Mutex::new(file),
            entries,
            digest: trailer.digest,
            payload,
        }))
    }

    /// Number of files carried.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the bundle carries no files.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total payload size, header and index included.
    pub fn byte_length(&self) -> u64 {
        self.payload.length
    }

    /// Every path carried, in sorted order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Whether a path is carried.
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Where a path lives inside the executable.
    pub fn entry(&self, path: &str) -> Option<BundleEntry> {
        self.entries.get(path).copied()
    }

    /// Locks the handle without propagating poisoning: a panic elsewhere must
    /// not make the application's own files unreadable.
    fn locked(&self) -> std::sync::MutexGuard<'_, File> {
        self.file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads one file out of the executable.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, BundleError> {
        let entry = self
            .entries
            .get(path)
            .ok_or_else(|| BundleError::NotFound(path.to_owned()))?;
        let mut file = self.locked();
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut bytes = vec![0_u8; entry.length as usize];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Reads one file as UTF-8.
    pub fn read_to_string(&self, path: &str) -> Result<String, BundleError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| malformed(format!("not UTF-8 at {path}")))
    }

    /// Recomputes the payload digest and compares it with the trailer's.
    ///
    /// Not done at startup: it reads the whole payload, and product requirement
    /// P2 is a cold start measured in milliseconds. Offered for `blitsen doctor`
    /// and for a build that wants to prove what it produced.
    pub fn verify(&self) -> Result<(), BundleError> {
        let mut file = self.locked();
        file.seek(SeekFrom::Start(self.payload.offset))?;
        let mut hasher = Sha256::new();
        let mut remaining = self.payload.length;
        let mut buffer = vec![0_u8; SCAN_CHUNK.min(remaining.max(1) as usize)];
        while remaining > 0 {
            let wanted = buffer.len().min(remaining as usize);
            file.read_exact(&mut buffer[..wanted])?;
            hasher.update(&buffer[..wanted]);
            remaining -= wanted as u64;
        }
        if hasher.finalize().as_slice() == self.digest {
            Ok(())
        } else {
            Err(BundleError::DigestMismatch)
        }
    }

    /// The digest the trailer records, as lowercase hex.
    pub fn digest(&self) -> String {
        self.digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// Appends `files` to a copy of `runtime`, producing a linked executable.
///
/// The reference implementation of the writer. The CLI has its own in
/// JavaScript, because a cross-target export cannot run the target's runtime;
/// `cli-bundle.test.mjs` holds the two to the same bytes.
pub fn write_bundle(
    runtime: &Path,
    output: &Path,
    files: &[(String, Vec<u8>)],
) -> Result<u64, BundleError> {
    let mut payload = Vec::new();
    payload.extend_from_slice(PAYLOAD_MAGIC);
    payload.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload.extend_from_slice(&(files.len() as u32).to_le_bytes());

    let mut index = Vec::new();
    let mut data = Vec::new();
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    for (path, bytes) in sorted {
        index.extend_from_slice(&(path.len() as u32).to_le_bytes());
        index.extend_from_slice(&(data.len() as u64).to_le_bytes());
        index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        index.extend_from_slice(path.as_bytes());
        data.extend_from_slice(bytes);
    }
    payload.extend_from_slice(&(index.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(data.len() as u64).to_le_bytes());
    payload.extend_from_slice(&index);
    payload.extend_from_slice(&data);

    let mut executable = std::fs::read(runtime)?;
    let payload_offset = executable.len() as u64;
    let digest = Sha256::digest(&payload);

    executable.extend_from_slice(&payload);
    executable.extend_from_slice(&digest);
    executable.extend_from_slice(&payload_offset.to_le_bytes());
    executable.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    executable.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    executable.extend_from_slice(&0_u32.to_le_bytes());
    executable.extend_from_slice(TRAILER_MAGIC);

    let mut file = File::create(output)?;
    file.write_all(&executable)?;
    file.flush()?;
    Ok(payload.len() as u64)
}

fn parse_index(
    index: &[u8],
    count: usize,
    data_start: u64,
    data_len: u64,
) -> Result<BTreeMap<String, BundleEntry>, BundleError> {
    let mut entries = BTreeMap::new();
    let mut cursor = 0_usize;
    for _ in 0..count {
        if cursor + 20 > index.len() {
            return Err(malformed("truncated: its index ends mid-entry"));
        }
        let path_len = read_u32(&index[cursor..cursor + 4]) as usize;
        let offset = read_u64(&index[cursor + 4..cursor + 12]);
        let length = read_u64(&index[cursor + 12..cursor + 20]);
        cursor += 20;
        if cursor + path_len > index.len() {
            return Err(malformed("truncated: its index ends mid-path"));
        }
        let path = std::str::from_utf8(&index[cursor..cursor + path_len])
            .map_err(|_| malformed("invalid: a path in its index is not UTF-8"))?
            .to_owned();
        cursor += path_len;
        if offset.saturating_add(length) > data_len {
            return Err(malformed(format!(
                "inconsistent: {path} runs past the end of its data"
            )));
        }
        if !is_safe_path(&path) {
            return Err(malformed(format!(
                "unsafe: {path} is not a path inside the application"
            )));
        }
        if entries
            .insert(
                path.clone(),
                BundleEntry {
                    offset: data_start + offset,
                    length,
                },
            )
            .is_some()
        {
            return Err(malformed(format!("inconsistent: {path} appears twice")));
        }
    }
    if cursor != index.len() {
        return Err(malformed("inconsistent: its index has trailing bytes"));
    }
    Ok(entries)
}

/// A bundle path addresses a file inside the application and nothing else.
///
/// Enforced when the index is read rather than when a file is asked for, so a
/// bundle that could serve something outside the application is refused before
/// anything reads from it.
///
/// Public because a container that has no index to check up front needs the same
/// rule at the moment a file is asked for: an APK's `assets/` is read through
/// `AAssetManager`, which has no canonicalisation to fall back on the way a
/// directory does (issue #144). One spelling of "inside the application" rather
/// than two that agree on most inputs.
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && !path.split('/').any(|segment| {
            segment.is_empty() || segment == "." || segment == ".." || segment.ends_with(':')
        })
}

/// A trailer that has been read and checked against the payload it points at.
struct Trailer {
    digest: [u8; 32],
    payload: BundleEntry,
}

/// Finds the trailer, allowing for a code signature appended after it.
///
/// The magic alone is not enough to accept a candidate. This runtime carries
/// the constant in its own read-only data, so a scan through an unbundled
/// executable finds it and would otherwise read whatever follows as a header —
/// which is how a plain `blitsen-runtime` came to report itself as format
/// version 1221590857. A candidate is therefore only taken when the trailer it
/// begins is self-consistent *and* points at bytes that open with the payload
/// magic; anything else is skipped and the scan continues.
fn find_trailer(file: &mut File, size: u64) -> Result<Option<Trailer>, BundleError> {
    if size < TRAILER_SIZE as u64 {
        return Ok(None);
    }
    let mut newest_rejected_version = None;
    let mut consider = |file: &mut File, magic_at: u64| -> Result<Option<Trailer>, BundleError> {
        if magic_at < 56 {
            return Ok(None);
        }
        let trailer_at = magic_at - 56;
        let mut bytes = [0_u8; TRAILER_SIZE];
        file.seek(SeekFrom::Start(trailer_at))?;
        file.read_exact(&mut bytes)?;

        let version = read_u32(&bytes[48..52]);
        let payload = BundleEntry {
            offset: read_u64(&bytes[32..40]),
            length: read_u64(&bytes[40..48]),
        };
        if payload.length < HEADER_SIZE as u64
            || payload.offset.saturating_add(payload.length) > trailer_at
        {
            return Ok(None);
        }
        let mut magic = [0_u8; 8];
        file.seek(SeekFrom::Start(payload.offset))?;
        file.read_exact(&mut magic)?;
        if &magic != PAYLOAD_MAGIC {
            return Ok(None);
        }
        // Structurally a bundle, and only now is a version worth complaining
        // about: this really is one, written by a newer Blitsen.
        if version > FORMAT_VERSION {
            newest_rejected_version = Some(version);
            return Ok(None);
        }
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&bytes[..32]);
        Ok(Some(Trailer { digest, payload }))
    };

    let at_end = size - TRAILER_SIZE as u64;
    let mut magic = [0_u8; 8];
    file.seek(SeekFrom::Start(at_end + 56))?;
    file.read_exact(&mut magic)?;
    if &magic == TRAILER_MAGIC
        && let Some(trailer) = consider(file, at_end + 56)?
    {
        return Ok(Some(trailer));
    }

    let floor = size.saturating_sub(MAX_TRAILING_BYTES);
    let mut window_end = size;
    let mut carry = Vec::new();
    while window_end > floor {
        let window_start = window_end.saturating_sub(SCAN_CHUNK as u64).max(floor);
        let mut chunk = vec![0_u8; (window_end - window_start) as usize];
        file.seek(SeekFrom::Start(window_start))?;
        file.read_exact(&mut chunk)?;
        // The magic can straddle two windows, so the first seven bytes of the
        // window already searched are searched again with this one.
        chunk.extend_from_slice(&carry);
        let mut search = chunk.len();
        while let Some(found) = last_occurrence(&chunk[..search], TRAILER_MAGIC) {
            if let Some(trailer) = consider(file, window_start + found as u64)? {
                return Ok(Some(trailer));
            }
            search = found + TRAILER_MAGIC.len() - 1;
        }
        carry = chunk[..chunk.len().min(7)].to_vec();
        window_end = window_start;
    }
    match newest_rejected_version {
        Some(found) => Err(BundleError::UnsupportedVersion {
            found,
            supported: FORMAT_VERSION,
        }),
        None => Ok(None),
    }
}

fn last_occurrence(haystack: &[u8], needle: &[u8; 8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("four bytes"))
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("eight bytes"))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn temporary(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("blitsen-bundle-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join(name)
    }

    fn runtime_stub(name: &str) -> std::path::PathBuf {
        let path = temporary(name);
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![0x7f_u8; 4096]).unwrap();
        path
    }

    fn sample() -> Vec<(String, Vec<u8>)> {
        vec![
            ("index.html".to_owned(), b"<!doctype html><p>hi".to_vec()),
            ("assets/app.js".to_owned(), b"console.log(1)".to_vec()),
            ("assets/logo.png".to_owned(), vec![0x89, b'P', b'N', b'G']),
        ]
    }

    #[test]
    fn a_linked_executable_reads_back_every_file_it_carries() {
        let runtime = runtime_stub("runtime-readback");
        let output = temporary("app-readback");
        let written = write_bundle(&runtime, &output, &sample()).unwrap();
        assert!(written > 0);

        let bundle = AppBundle::open(&output).unwrap().unwrap();
        assert_eq!(bundle.len(), 3);
        assert_eq!(
            bundle.paths().collect::<Vec<_>>(),
            ["assets/app.js", "assets/logo.png", "index.html"]
        );
        assert_eq!(
            bundle.read_to_string("index.html").unwrap(),
            "<!doctype html><p>hi"
        );
        assert_eq!(
            bundle.read("assets/logo.png").unwrap(),
            [0x89, b'P', b'N', b'G']
        );
        assert!(bundle.contains("assets/app.js"));
        assert!(!bundle.contains("missing.js"));
        assert!(matches!(
            bundle.read("missing.js"),
            Err(BundleError::NotFound(path)) if path == "missing.js"
        ));
        bundle.verify().unwrap();
        assert_eq!(bundle.digest().len(), 64);
    }

    #[test]
    fn an_executable_without_a_bundle_reports_no_bundle_rather_than_an_error() {
        let runtime = runtime_stub("runtime-bare");
        assert!(AppBundle::open(&runtime).unwrap().is_none());
    }

    #[test]
    fn a_signature_appended_after_the_trailer_does_not_hide_it() {
        let runtime = runtime_stub("runtime-signed");
        let output = temporary("app-signed");
        write_bundle(&runtime, &output, &sample()).unwrap();
        // What `codesign` and `signtool` do: more bytes after everything else.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&output)
            .unwrap();
        file.write_all(&vec![0xa5_u8; 9_000]).unwrap();
        drop(file);

        let bundle = AppBundle::open(&output).unwrap().unwrap();
        assert_eq!(bundle.len(), 3);
        assert_eq!(
            bundle.read_to_string("assets/app.js").unwrap(),
            "console.log(1)"
        );
        bundle.verify().unwrap();
    }

    #[test]
    fn a_damaged_payload_fails_verification_rather_than_serving_wrong_bytes() {
        let runtime = runtime_stub("runtime-damaged");
        let output = temporary("app-damaged");
        write_bundle(&runtime, &output, &sample()).unwrap();
        let mut bytes = std::fs::read(&output).unwrap();
        let last = bytes.len() - TRAILER_SIZE - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&output, &bytes).unwrap();

        let bundle = AppBundle::open(&output).unwrap().unwrap();
        assert!(matches!(bundle.verify(), Err(BundleError::DigestMismatch)));
    }

    #[test]
    fn the_magic_appearing_in_the_runtime_itself_is_not_mistaken_for_a_bundle() {
        // The real case: this crate carries `TRAILER_MAGIC` in its read-only
        // data, so every unbundled `blitsen-runtime` contains a copy of it.
        let path = temporary("runtime-selfreference");
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![0x7f_u8; 2048]).unwrap();
        file.write_all(TRAILER_MAGIC).unwrap();
        file.write_all(PAYLOAD_MAGIC).unwrap();
        file.write_all(&vec![0x7f_u8; 2048]).unwrap();
        drop(file);
        assert!(AppBundle::open(&path).unwrap().is_none());

        // And it is still found when a real bundle follows the decoy.
        let output = temporary("app-selfreference");
        write_bundle(&path, &output, &sample()).unwrap();
        let bundle = AppBundle::open(&output).unwrap().unwrap();
        assert_eq!(bundle.len(), 3);
        bundle.verify().unwrap();
    }

    #[test]
    fn a_newer_format_is_named_rather_than_misread() {
        let runtime = runtime_stub("runtime-future");
        let output = temporary("app-future");
        write_bundle(&runtime, &output, &sample()).unwrap();
        let mut bytes = std::fs::read(&output).unwrap();
        let version_at = bytes.len() - TRAILER_SIZE + 48;
        bytes[version_at..version_at + 4].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        std::fs::write(&output, &bytes).unwrap();

        assert!(matches!(
            AppBundle::open(&output),
            Err(BundleError::UnsupportedVersion { found, supported })
                if found == FORMAT_VERSION + 1 && supported == FORMAT_VERSION
        ));
    }

    #[test]
    fn a_path_that_escapes_the_application_is_refused_when_the_index_is_read() {
        for escape in ["../secret", "/etc/passwd", "a/../../b", "", "a//b"] {
            let runtime = runtime_stub("runtime-escape");
            let output = temporary("app-escape");
            write_bundle(&runtime, &output, &[(escape.to_owned(), b"x".to_vec())]).unwrap();
            let error = AppBundle::open(&output).unwrap_err();
            assert!(
                matches!(error, BundleError::Malformed(_)),
                "{escape} was accepted"
            );
        }
    }
}
