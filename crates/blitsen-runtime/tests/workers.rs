//! Web workers, end to end, against the real runtime binary.
//!
//! A directory rather than a linked executable: linking copies the whole runtime
//! for every case, and nothing here is about the bundle format. What it is about
//! is the part no unit test can reach — a second engine on a second thread, with
//! a real message crossing between them — so the assertions are made on what the
//! application printed, from the process that ran it.
//!
//! The standalone check is the harness. It boots the document, turns the frame
//! loop until its asynchronous work settles and exits, which is exactly the
//! sequence a windowed run would perform without needing a display.

use std::path::{Path, PathBuf};
use std::process::Command;

fn runtime_binary() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_blitsen-runtime"));
    path.is_file().then_some(path)
}

/// A unique application directory removed when the test that made it ends.
struct App(tempfile::TempDir);

impl App {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&scratch).expect("application directory root");
        let root = tempfile::Builder::new()
            .prefix(&format!("workers-{name}-"))
            .tempdir_in(scratch)
            .expect("application directory");
        for (path, source) in files {
            std::fs::write(root.path().join(path), source).expect("write application file");
        }
        Self(root)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

/// Runs the application headlessly and returns everything it printed.
///
/// Both streams: an uncaught worker exception is reported on stderr as well as
/// to the application, and a test that only read stdout would call a worker that
/// died on startup a worker that never answered.
fn run(app: &App) -> Option<String> {
    let runtime = runtime_binary()?;
    let output = Command::new(runtime)
        .arg(app.path())
        .env("BLITSEN_STANDALONE_CHECK", "1")
        // Long enough for a thread to start, load a module graph and answer.
        .env("BLITSEN_STANDALONE_CHECK_DELAY", "1500")
        .output()
        .expect("run the application");
    let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
    printed.push_str(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "the application did not exit cleanly:\n{printed}"
    );
    Some(printed)
}

const PAGE: &str = "<!doctype html><body><main id=out></main>\
                    <script type=\"module\" src=\"app.js\"></script>";

#[test]
fn a_worker_runs_its_own_module_graph_and_answers_on_its_own_thread() {
    let app = App::new(
        "round-trip",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                r#"const worker = new Worker("./work.js", { type: "module", name: "adder" });
                   worker.onmessage = event => console.log("SUM", event.data.sum, event.data.name);
                   worker.onerror = event => console.log("FAILED", event.message);
                   worker.postMessage({ values: [1, 2, 3] });"#,
            ),
            (
                "work.js",
                r#"import { total } from "./sum.js";
                   self.onmessage = event =>
                     postMessage({ sum: total(event.data.values), name: self.name });"#,
            ),
            (
                "sum.js",
                "export const total = values => values.reduce((a, b) => a + b, 0);",
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("SUM 6 adder"),
        "the worker did not answer: {printed}"
    );
}

#[test]
fn a_message_keeps_the_shapes_structured_clone_promises() {
    let app = App::new(
        "clone",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                r#"const worker = new Worker("./work.js", { type: "module" });
                   const shared = new ArrayBuffer(8);
                   // A class instance is cloned, not refused: its own properties
                   // cross and the prototype does not. Monaco posts its protocol
                   // messages this way, and so does anything built on Comlink.
                   class Point { constructor(x, y) { this.x = x; this.y = y; }
                                 length() { return Math.hypot(this.x, this.y); } }
                   const message = {
                     point: new Point(3, 4),
                     when: new Date(1000), set: new Set([1, 2, 3]), map: new Map([["k", "v"]]),
                     viewA: new Uint8Array(shared, 0, 4), viewB: new Uint8Array(shared, 4, 4),
                     re: /ab+c/gi, err: new TypeError("bad"), big: 9007199254740993n,
                     negZero: -0, nan: NaN, sparse: [0, , 2],
                   };
                   message.self = message;
                   worker.onmessage = event => console.log("SHAPES", JSON.stringify(event.data));
                   worker.onerror = event => console.log("FAILED", event.message);
                   worker.postMessage(message);"#,
            ),
            (
                "work.js",
                r#"self.onmessage = event => {
                     const data = event.data;
                     postMessage({
                       instance: [data.point.x, data.point.y],
                       instancePrototype: Object.getPrototypeOf(data.point) === Object.prototype,
                       instanceMethod: typeof data.point.length,
                       date: data.when.getTime(),
                       set: data.set.size, map: data.map.get("k"),
                       cyclic: data.self === data,
                       sharedBuffer: data.viewA.buffer === data.viewB.buffer,
                       offsets: [data.viewA.byteOffset, data.viewB.byteOffset],
                       re: data.re.source + data.re.flags,
                       error: data.err instanceof TypeError && data.err.message,
                       big: String(data.big),
                       negZero: Object.is(data.negZero, -0), nan: Number.isNaN(data.nan),
                       hole: (1 in data.sparse), length: data.sparse.length,
                     });
                   };"#,
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    let line = printed
        .lines()
        .find(|line| line.starts_with("SHAPES "))
        .unwrap_or_else(|| panic!("the worker did not answer: {printed}"));
    let shapes: serde_json::Value =
        serde_json::from_str(line.trim_start_matches("SHAPES ")).expect("the reply is JSON");
    assert_eq!(shapes["instance"], serde_json::json!([3, 4]));
    assert_eq!(
        shapes["instancePrototype"], true,
        "a class instance arrives as a plain object, as structured clone specifies"
    );
    assert_eq!(
        shapes["instanceMethod"], "undefined",
        "its methods live on the prototype, which is not part of the message"
    );
    assert_eq!(shapes["date"], 1000);
    assert_eq!(shapes["set"], 3);
    assert_eq!(shapes["map"], "v");
    assert_eq!(shapes["cyclic"], true, "a cycle survives the crossing");
    assert_eq!(
        shapes["sharedBuffer"], true,
        "two views over one buffer stay two views over one buffer"
    );
    assert_eq!(shapes["offsets"], serde_json::json!([0, 4]));
    assert_eq!(shapes["re"], "ab+cgi");
    assert_eq!(shapes["error"], "bad");
    assert_eq!(
        shapes["big"], "9007199254740993",
        "a BigInt past the double's integers arrives exact"
    );
    assert_eq!(shapes["negZero"], true, "-0 is not 0");
    assert_eq!(shapes["nan"], true);
    assert_eq!(shapes["hole"], false, "a hole is not an undefined element");
    assert_eq!(shapes["length"], 3);
}

#[test]
fn a_transferred_buffer_is_detached_here_and_whole_there() {
    let app = App::new(
        "transfer",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                r#"const worker = new Worker("./work.js", { type: "module" });
                   const buffer = new Uint8Array([7, 8, 9]).buffer;
                   worker.onmessage = event =>
                     console.log("MOVED", JSON.stringify(event.data), "here", buffer.byteLength);
                   worker.onerror = event => console.log("FAILED", event.message);
                   worker.postMessage({ buffer }, [buffer]);
                   try { worker.postMessage({ callback: () => 1 }); }
                   catch (error) { console.log("REFUSED", error.name); }"#,
            ),
            (
                "work.js",
                r#"self.onmessage = event =>
                     postMessage([...new Uint8Array(event.data.buffer)]);"#,
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("MOVED [7,8,9] here 0"),
        "a transfer must arrive whole and leave nothing behind: {printed}"
    );
    assert!(
        printed.contains("REFUSED DataCloneError"),
        "a function is not cloneable and must say so: {printed}"
    );
}

#[test]
fn a_port_handed_to_a_worker_carries_messages_from_there_on() {
    let app = App::new(
        "ports",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                r#"const worker = new Worker("./work.js", { type: "module" });
                   const channel = new MessageChannel();
                   channel.port1.onmessage = event => console.log("PORT", event.data);
                   worker.onmessage = () => channel.port1.postMessage("ping");
                   worker.onerror = event => console.log("FAILED", event.message);
                   worker.postMessage({ take: "this" }, [channel.port2]);"#,
            ),
            (
                "work.js",
                r#"self.onmessage = event => {
                     const port = event.ports[0];
                     port.onmessage = message => port.postMessage(`echo:${message.data}`);
                     postMessage({ ports: event.ports.length });
                   };"#,
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("PORT echo:ping"),
        "a transferred port must deliver on its new thread: {printed}"
    );
}

#[test]
fn a_worker_that_throws_reports_it_to_whoever_started_it() {
    let app = App::new(
        "throwing",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                r#"const worker = new Worker("./work.js", { type: "module" });
                   worker.onerror = event => console.log("REPORTED");
                   let missing;
                   try { new Worker("./nothing.js", { type: "module" }); }
                   catch (error) { missing = error.message; }
                   console.log("MISSING", String(missing).includes("nothing.js"));"#,
            ),
            ("work.js", "throw new Error(\"the worker gave up\");"),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("REPORTED"),
        "an uncaught worker exception must reach the Worker object: {printed}"
    );
    assert!(
        printed.contains("MISSING true"),
        "a worker script the application does not ship is refused at the constructor: {printed}"
    );
}

#[test]
fn terminating_a_worker_stops_it_even_inside_a_loop() {
    let app = App::new(
        "terminate",
        &[
            ("index.html", PAGE),
            (
                "app.js",
                // The worker never yields, so nothing but the engine's interrupt
                // can stop it. If `terminate` only took effect between turns
                // this would run until the check's deadline and print nothing.
                r#"const worker = new Worker("./work.js", { type: "module" });
                   worker.onmessage = () => {
                     worker.terminate();
                     console.log("TERMINATED");
                     worker.postMessage("ignored");
                     setTimeout(() => console.log("STILL RUNNING"), 200);
                   };
                   worker.postMessage("go");"#,
            ),
            (
                "work.js",
                r#"self.onmessage = () => {
                     postMessage("started");
                     for (;;) {}
                   };"#,
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("TERMINATED"),
        "the worker never started: {printed}"
    );
    assert!(
        printed.contains("STILL RUNNING"),
        "the document must go on turning after a worker is terminated: {printed}"
    );
}

/// Issue #90's property, at the level a module can observe it: a directory being
/// run and the executable it exports to must resolve the same graph the same
/// way. They did not — a directory named its scripts by their path on disk, and
/// `import` refused that as a referrer, so every document module failed there
/// and worked once exported.
#[test]
fn a_directory_run_resolves_modules_by_application_url_as_a_bundle_does() {
    let app = App::new(
        "identity",
        &[
            (
                "index.html",
                "<!doctype html><body><main id=out></main>\
                 <script type=\"module\" src=\"app.js\"></script>\
                 <script type=\"module\">import { mark } from \"./lib.js\";\
                 console.log(\"INLINE\", mark);</script>",
            ),
            (
                "app.js",
                r#"import { mark } from "./lib.js";
                   console.log("STATIC", mark, "META", import.meta.url);
                   const loaded = await import("./lib.js");
                   console.log("DYNAMIC", loaded.mark);
                   try { await import("./missing.js"); }
                   catch (error) { console.log("MISSING", error.message.split("\n")[0]); }
                   // A worker is resolved against the document the same way. The
                   // `new URL(..., import.meta.url)` spelling every bundler emits
                   // needs `URL`, which this runtime does not have yet.
                   const worker = new Worker("./work.js", { type: "module" });
                   worker.onmessage = event => console.log("WORKER", event.data);
                   worker.onerror = event => console.log("WORKER FAILED", event.message);
                   worker.postMessage(0);"#,
            ),
            ("lib.js", "export const mark = \"resolved\";"),
            (
                "work.js",
                "self.onmessage = () => postMessage(self.location.href);",
            ),
        ],
    );
    let Some(printed) = run(&app) else {
        eprintln!("skipping: the runtime binary was not built");
        return;
    };
    assert!(
        printed.contains("STATIC resolved META blitsen://app/app.js"),
        "a document module is named by its application URL: {printed}"
    );
    assert!(
        printed.contains("DYNAMIC resolved"),
        "dynamic import resolves against it too: {printed}"
    );
    assert!(
        printed.contains("INLINE resolved"),
        "and so does an inline module script: {printed}"
    );
    assert!(
        printed.contains("WORKER blitsen://app/work.js"),
        "a worker resolves against the document that started it: {printed}"
    );
    // The failure names the module and the importer rather than arriving as
    // QuickJS's uninitialized marker, which is what every import error used to
    // look like.
    assert!(
        printed.contains("MISSING the application has no module at missing.js"),
        "an import that fails says why: {printed}"
    );
}
