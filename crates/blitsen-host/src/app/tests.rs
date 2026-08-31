use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Mutex;

use blitsen_quickjs::QuickJs;
use blitz::traits::net::{Bytes, NetHandler, Request};

use super::*;

#[test]
fn load_options_make_both_document_modes_explicit() {
    let application = LoadOptions::new(800, 600, DocumentMode::Application);
    assert_eq!(application.mode, DocumentMode::Application);
    assert!(application.viewport.is_none());

    let viewport = Viewport::new(320, 240, 2.0, ColorScheme::Dark);
    let harness = LoadOptions::new(320, 240, DocumentMode::TestHarness).with_viewport(viewport);
    assert_eq!(harness.mode, DocumentMode::TestHarness);
    assert!(harness.viewport.is_some());
}

#[test]
fn a_window_script_sees_the_initial_viewport_scale() {
    let directory = tempfile::tempdir().unwrap();
    let entrypoint = directory.path().join("index.html");
    std::fs::write(&entrypoint, "<!doctype html><title>scaled</title>").unwrap();
    let files = AppFiles::directory(&entrypoint).unwrap();
    let net_provider = files.net_provider().unwrap();
    let viewport = Viewport::new(1600, 1200, 2.0, ColorScheme::Light);
    let mut engine = QuickJs::new().expect("a QuickJS runtime");
    let _services =
        crate::runtime_services::RuntimeServices::install(&mut engine).expect("runtime services");

    load_window_document(
        &mut engine,
        &files,
        net_provider,
        LoadOptions::new(800, 600, DocumentMode::Application).with_viewport(viewport),
    )
    .unwrap();

    let observed = engine
        .evaluate_script(
            "devicePixelRatio === 2 && innerWidth === 800 && innerHeight === 600",
            "initial-viewport-scale.js",
        )
        .unwrap();
    assert!(engine.to_boolean(&observed).unwrap());
}

/// Collects what a subresource provider answered, so a test can ask how
/// many bytes a URL was served rather than whether a handler was called.
#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Vec<(String, usize)>>>);

impl NetHandler for Collector {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        self.0.lock().unwrap().push((resolved_url, bytes.len()));
    }
}

impl Collector {
    /// How many bytes `provider` answered `url` with.
    fn served(&self, provider: &Arc<dyn NetProvider>, url: &str) -> usize {
        provider.fetch(
            0,
            Request::get(Url::parse(url).unwrap()),
            Box::new(self.clone()),
        );
        self.0.lock().unwrap().last().unwrap().1
    }
}

/// Serves the three requests the dev-server provider parity check makes.
fn app_server() -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            let target = request
                .lines()
                .next()
                .and_then(|line| line.split_ascii_whitespace().nth(1))
                .unwrap()
                .to_owned();
            requests.push(target.clone());
            let (status, body): (&str, &[u8]) = match target.as_str() {
                "/index.html" => ("200 OK", b"<p>hi"),
                "/assets/app.css?theme=dark" => ("200 OK", b"body{color:white}"),
                "/assets/missing.css" => ("404 Not Found", b""),
                _ => ("500 Unexpected Request", b""),
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            stream.flush().unwrap();
        }
        requests
    });
    (origin, handle)
}

#[test]
fn a_bundle_addresses_its_files_by_the_application_origin() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    std::fs::write(&runtime, vec![0_u8; 512]).unwrap();
    let linked = root.path().join("app");
    blitsen_core::bundle::write_bundle(
        &runtime,
        &linked,
        &[
            ("index.html".to_owned(), b"<p>hi".to_vec()),
            ("assets/app.js".to_owned(), b"globalThis.ran = 1".to_vec()),
        ],
    )
    .unwrap();

    let bundle = AppBundle::open(&linked).unwrap().unwrap();
    let files = AppFiles::bundle(bundle, "index.html").unwrap();
    assert_eq!(files.entrypoint_source().unwrap(), "<p>hi");
    assert_eq!(files.base_url(), APP_ORIGIN);
    assert_eq!(files.entrypoint_name(), "blitsen://app/index.html");
    assert_eq!(
        files
            .script_loader()
            .load(Path::new("."), "assets/app.js")
            .unwrap(),
        (
            "globalThis.ran = 1".to_owned(),
            "blitsen://app/assets/app.js".to_owned()
        )
    );
    assert!(files.net_provider().is_some());
    let provider = files.net_provider().unwrap();
    let collector = Collector::default();
    assert_eq!(
        collector.served(&provider, "blitsen://app/assets/app.js"),
        18
    );
    assert_eq!(
        collector.served(&provider, "blitsen://app/assets/missing.js"),
        0
    );
    assert_eq!(
        files.source().read("assets/app.js").unwrap(),
        b"globalThis.ran = 1"
    );
}

#[test]
fn a_dev_server_provider_preserves_queries_missing_errors_and_callbacks() {
    let (origin, server) = app_server();
    let files = AppFiles::server(&origin).unwrap();
    assert_eq!(
        files.storage_identity(),
        format!("server:{origin}/index.html")
    );
    let provider = files.net_provider().expect("a server serves its files");
    let collector = Collector::default();

    assert_eq!(
        collector.served(&provider, "blitsen://app/assets/app.css?theme=dark"),
        17
    );
    assert_eq!(
        collector.served(&provider, "blitsen://app/assets/missing.css"),
        0
    );
    let AppFiles::Server { server: source, .. } = &files else {
        unreachable!()
    };
    assert_eq!(
        source.last_error(),
        Some(format!("{origin}/assets/missing.css answered 404"))
    );
    assert_eq!(
        server.join().unwrap(),
        [
            "/index.html",
            "/assets/app.css?theme=dark",
            "/assets/missing.css"
        ]
    );
}

/// A stock `vite build` writes `/assets/index-<hash>.js`, and Blitz resolves
/// that against the document's `file:` base to the top of the disk. The
/// application root is what it meant.
#[test]
fn a_directory_serves_a_server_root_subresource_from_the_application_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("assets")).unwrap();
    std::fs::write(root.path().join("index.html"), b"<p>hi").unwrap();
    std::fs::write(root.path().join("assets/app.css"), b"body{margin:0}").unwrap();
    let files = AppFiles::directory(root.path().join("index.html")).unwrap();
    assert_eq!(
        files.storage_identity(),
        format!(
            "directory:{}",
            simplified(root.path().canonicalize().unwrap()).display()
        )
    );
    let provider = files
        .net_provider()
        .expect("a directory serves its own files");

    let collector = Collector::default();
    let served = |url: &str| collector.served(&provider, url);
    // What Blitz asks for after resolving `href="/assets/app.css"`.
    assert_eq!(served("file:///assets/app.css"), 14);
    // An ordinary relative reference still reads where it points. Built
    // through `Url::from_file_path` rather than by formatting the path into
    // a string: on Windows that string is `C:\...`, whose drive letter parses
    // as a URL host and whose separators are not URL separators.
    let inside = Url::from_file_path(root.path().join("assets").join("app.css")).unwrap();
    assert_eq!(served(inside.as_str()), 14);
    // And a server-root path the application does not ship stays empty, so
    // the document paints without it rather than waiting on it.
    assert_eq!(served("file:///assets/nothing.css"), 0);
}

/// The reader resolves the application root once, when it is made, and every
/// `file:` target afresh against it — the same snapshot [`DirectorySource`]
/// takes of the root it serves.
#[test]
fn a_reader_confines_file_urls_to_the_root_it_was_made_with() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), b"<p>hi").unwrap();
    std::fs::write(root.path().join("data.json"), b"{}").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("secret.txt"), b"secret").unwrap();

    let reader = AppFiles::directory(root.path().join("index.html"))
        .unwrap()
        .reader();
    let canonical = root.path().canonicalize().unwrap();
    let inside = Url::from_file_path(canonical.join("data.json")).unwrap();
    assert_eq!(reader.read_url(&inside).ok().unwrap(), b"{}");
    // A file the application does not ship is missing, not outside: the
    // reader must send the caller looking for the right mistake.
    let missing = Url::from_file_path(canonical.join("missing.json")).unwrap();
    assert!(matches!(
        reader.read_url(&missing),
        Err(NotRead::Missing(path)) if path == "missing.json"
    ));
    // Another directory's files are outside, however real they are.
    let outside =
        Url::from_file_path(elsewhere.path().canonicalize().unwrap().join("secret.txt")).unwrap();
    assert!(matches!(reader.read_url(&outside), Err(NotRead::Outside)));
}

/// The fourth shape reaching the same session as the other three: same
/// application origin, same script identifiers, same subresource contract.
/// Run against a directory standing in for the APK's `assets/`, which is
/// what the platform mounts one as (issue #144).
#[test]
fn an_apk_addresses_its_files_by_the_application_origin_exactly_as_a_bundle_does() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join(crate::apk::DEFAULT_ASSET_ROOT);
    std::fs::create_dir_all(application.join("assets")).unwrap();
    std::fs::write(application.join("index.html"), b"<p>hi").unwrap();
    std::fs::write(application.join("assets/app.js"), b"globalThis.ran = 1").unwrap();
    std::fs::write(application.join("assets/app.css"), b"body{margin:0}").unwrap();
    std::fs::write(
        application.join(crate::apk::ASSET_INDEX),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "files": [
                { "path": "assets/app.css", "bytes": 14 },
                { "path": "assets/app.js", "bytes": 18 },
                { "path": "index.html", "bytes": 5 },
                { "path": RUNTIME_CONFIG, "bytes": 2 },
            ],
        }))
        .unwrap(),
    )
    .unwrap();

    let assets = ApkAssets::open_directory(root.path(), crate::apk::DEFAULT_ASSET_ROOT);
    let files = AppFiles::assets(assets, "index.html").unwrap();
    assert_eq!(files.entrypoint_source().unwrap(), "<p>hi");
    assert_eq!(files.base_url(), APP_ORIGIN);
    assert_eq!(files.entrypoint_name(), "blitsen://app/index.html");
    assert_eq!(
        files
            .script_loader()
            .load(Path::new("."), "assets/app.js")
            .unwrap(),
        (
            "globalThis.ran = 1".to_owned(),
            "blitsen://app/assets/app.js".to_owned()
        )
    );
    assert_eq!(
        files.source().read("assets/app.js").unwrap(),
        b"globalThis.ran = 1"
    );
    // The runtime's own record is not one of the application's assets, the
    // same way it is not counted inside a bundle.
    assert_eq!(files.asset_count(), 3);
    // A directory being run is the only shape whose files are also `file:`
    // URLs; an asset has no path, so only the application origin reaches it.
    assert_eq!(
        files
            .reader()
            .read_url(&Url::parse("blitsen://app/assets/app.css").unwrap())
            .ok()
            .unwrap(),
        b"body{margin:0}"
    );

    // And the subresource provider answers on the application origin, with
    // an empty body for what the package does not carry — the contract that
    // keeps a missing stylesheet from hanging the frame.
    let provider = files.net_provider().expect("an APK serves its own files");
    let collector = Collector::default();
    let served = |url: &str| collector.served(&provider, url);
    assert_eq!(served("blitsen://app/assets/app.css"), 14);
    assert_eq!(served("blitsen://app/assets/nothing.css"), 0);
    // Nothing outside the application is reachable through the provider.
    assert_eq!(served("blitsen://app/../secret.txt"), 0);
}

#[test]
fn an_apk_without_an_entrypoint_says_so_rather_than_opening_a_blank_window() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join(crate::apk::DEFAULT_ASSET_ROOT);
    std::fs::create_dir_all(&application).unwrap();
    std::fs::write(application.join("readme.txt"), b"x").unwrap();
    let assets = ApkAssets::open_directory(root.path(), crate::apk::DEFAULT_ASSET_ROOT);
    let error = AppFiles::assets(assets, "index.html")
        .err()
        .expect("no entrypoint");
    assert!(error.message().contains("no index.html"), "{error}");
}

/// The notices are a file at a known path, not a property of the trailer,
/// which is what lets them survive onto a container with no trailer (#121,
/// #144).
#[test]
fn the_third_party_notices_are_read_from_an_apk_the_same_way_as_from_a_bundle() {
    use std::io::Write as _;

    let root = tempfile::tempdir().unwrap();
    let application = root.path().join(crate::apk::DEFAULT_ASSET_ROOT);
    std::fs::create_dir_all(&application).unwrap();
    std::fs::write(application.join("index.html"), b"<p>hi").unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"THIRD-PARTY NOTICES\n").unwrap();
    std::fs::write(application.join(NOTICES), encoder.finish().unwrap()).unwrap();

    let assets = ApkAssets::open_directory(root.path(), crate::apk::DEFAULT_ASSET_ROOT);
    let files = AppFiles::assets(assets, "index.html").unwrap();
    assert_eq!(
        notices(files.source().as_ref()).unwrap(),
        "THIRD-PARTY NOTICES\n"
    );

    // An artifact that carries none says so, which is the whole of the
    // acceptance gate: it has to be answerable by the thing that ships.
    let bare = ApkAssets::open_directory(root.path().join("nowhere"), "");
    assert!(notices(&bare).is_none());
}

/// `aapt` strips `.gz` from an asset and inflates it, so what an APK
/// actually holds is the plain text under the shortened name (#148). This
/// is the shape a real APK was measured to have; the test above is the
/// shape an appended bundle has. Both have to read.
#[test]
fn notices_are_read_from_an_apk_whose_packager_stripped_the_gzip() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join(crate::apk::DEFAULT_ASSET_ROOT);
    std::fs::create_dir_all(&application).unwrap();
    std::fs::write(application.join("index.html"), b"<p>hi").unwrap();
    std::fs::write(
        application.join(NOTICES_UNCOMPRESSED),
        b"THIRD-PARTY NOTICES\n",
    )
    .unwrap();

    let assets = ApkAssets::open_directory(root.path(), crate::apk::DEFAULT_ASSET_ROOT);
    let files = AppFiles::assets(assets, "index.html").unwrap();
    assert_eq!(
        notices(files.source().as_ref()).unwrap(),
        "THIRD-PARTY NOTICES\n"
    );
    // The gzipped name still wins where both are present, so an artifact
    // built before this and one built after cannot be read differently.
    assert!(!application.join(NOTICES).exists());
}

#[test]
fn a_bundle_without_an_entrypoint_says_so_rather_than_opening_a_blank_window() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    std::fs::write(&runtime, vec![0_u8; 512]).unwrap();
    let linked = root.path().join("app");
    blitsen_core::bundle::write_bundle(
        &runtime,
        &linked,
        &[("readme.txt".to_owned(), b"x".to_vec())],
    )
    .unwrap();
    let bundle = AppBundle::open(&linked).unwrap().unwrap();
    let error = AppFiles::bundle(bundle, "index.html")
        .err()
        .expect("no entrypoint");
    assert!(error.message().contains("no index.html"));
}
