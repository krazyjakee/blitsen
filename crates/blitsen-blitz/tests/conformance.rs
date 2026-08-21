//! Golden-image layout conformance corpus (product requirement P6).
//!
//! Each case is a self-contained document plus the geometry and colours it is
//! *meant* to produce, declared in a comment at the top of the same file. There
//! are two tiers, because they answer two different questions.
//!
//! * **The declared checks are a correctness oracle.** Every number in a case
//!   marked `oracle` was computed from the CSS that case declares, not read back
//!   from a run, so a passing case says the layout is *right* rather than
//!   merely unchanged. They are portable — the corpus pins its own font file,
//!   whose every glyph is a solid em block, so text metrics come out of a
//!   committed file rather than off the host — and are therefore gated
//!   everywhere. The one case that deliberately uses the host's fonts says so
//!   with `host-fonts`, and asserts only what is true of any correct rendering.
//! * **The golden PNG is a change detector.** It covers the whole frame,
//!   including everything nobody thought to assert; the three renderer bugs
//!   found in one day (no font sources, no image codecs, backgrounds a frame
//!   late) were all invisible to DOM-level assertions. Rasterization is not
//!   portable the way layout is, so the goldens are compared only where the
//!   raster fingerprint matches the machine they were recorded on.
//!
//! A case file opens with its expectation header: `@`-prefixed directives are
//! read, everything else in the comment is the arithmetic behind them. Record
//! with `BLITSEN_RECORD_CONFORMANCE=1 cargo test -p blitsen-blitz --test
//! conformance`; recording refuses to run while any declared check fails, so a
//! golden cannot lock in a layout the corpus itself says is wrong. The whole
//! arrangement is documented in `docs/CONFORMANCE.md`.
//!
//! A third kind of case documents a defect in Blitz itself. It says `defect`
//! and marks the individual checks a browser satisfies and Blitz does not with
//! `@!`. Those checks are asserted to *fail*: the gate stays green while the
//! defect stands, and the case fails the moment the defect is fixed, so the
//! entry gets closed rather than rotting. Such a case carries no golden image,
//! because a golden of wrong output is exactly the recording this corpus
//! refuses to keep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitsen_blitz::BlitzDom;
use blitsen_core::replay::FrameDigest;
use blitsen_dom::{DomBackend, LayoutSnapshot};
use blitz::dom::DocumentConfig;
use blitz::traits::shell::{ColorScheme, Viewport};
use peniko::{Color, Fill, kurbo::Rect};

/// Digest domain for the raster fingerprint. Bump when the fixture changes, so
/// a stale fingerprint stops comparison instead of comparing two questions.
const RASTER_DIGEST: &str = "blitsen.conformance.raster.v1";

/// Text and shapes whose rasterization depends on the host CPU but not its
/// fonts: the family is the corpus' own file, so two machines that disagree
/// here disagree about rasterization alone.
const RASTER_FIXTURE: &str = r#"<!doctype html><html><head><style>
  html, body { margin: 0; background: #ffffff }
  @font-face { font-family: "Block"; src: url("block-regular.ttf") format("truetype") }
  p { margin: 3px; font: 17px "Block"; color: #123456; letter-spacing: 2.5px }
  div { width: 111.5px; height: 13px; border-radius: 6.5px; background: #72e7f2 }
</style></head><body><p>BLITSEN RASTER</p><div></div></body></html>"#;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

/// Base URL for case subresources: the fonts and the image the corpus pins.
fn fixtures_url() -> String {
    format!("file://{}/fixtures/", env!("CARGO_MANIFEST_DIR"))
}

/// `target/conformance-divergence`, derived from the test binary's own path
/// because Cargo tells a test nothing else about where the target directory is.
fn divergence_directory() -> PathBuf {
    let executable = std::env::current_exe().expect("test binary path");
    executable
        .ancestors()
        .nth(3)
        .unwrap_or_else(|| Path::new("."))
        .join("conformance-divergence")
}

fn document(html: &str, width: u32, height: u32) -> BlitzDom {
    BlitzDom::from_html(
        html,
        DocumentConfig {
            base_url: Some(fixtures_url()),
            viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    )
}

/// Rasterizes a document the way a shipped window does: opaque white first,
/// then the display list over it.
fn render(dom: &mut BlitzDom, width: u32, height: u32) -> Vec<u8> {
    render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            scene.fill(
                Fill::NonZero,
                Default::default(),
                Color::WHITE,
                Default::default(),
                &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            );
            blitz_paint::paint_scene(scene, dom.document_mut().as_mut(), 1.0, width, height, 0, 0);
        },
        width,
        height,
    )
}

fn raster_fingerprint() -> String {
    let (width, height) = (256, 64);
    let mut dom = document(RASTER_FIXTURE, width, height);
    if dom.flush_layout().is_err() {
        return "unavailable".into();
    }
    let mut digest = FrameDigest::new(RASTER_DIGEST);
    digest
        .number(f64::from(width))
        .number(f64::from(height))
        .bytes(&render(&mut dom, width, height));
    digest.finish()
}

/// One declared expectation. Everything here is portable: box geometry comes
/// from the pinned font, and the colours are flat fills probed away from any
/// antialiased edge.
enum Check {
    /// Border box of the one element the selector matches. A component written
    /// `-` is not checked, which is how a case asserts the part of a box it can
    /// derive without also pinning the part it cannot.
    Box {
        selector: String,
        rect: [Option<f32>; 4],
    },
    Pixel {
        x: u32,
        y: u32,
        rgba: [u8; 4],
    },
    /// Fraction of a rectangle painted exactly `rgba`. Coverage rather than
    /// equality, so it survives antialiasing at the edges of a run while still
    /// failing when nothing was painted at all.
    Ink {
        rect: [u32; 4],
        rgba: [u8; 4],
        at_least: Option<f64>,
        at_most: Option<f64>,
    },
}

impl Check {
    /// Names the check without its expected value, for the message a case gets
    /// when a check it declared as failing starts holding.
    fn label(&self) -> String {
        match self {
            Check::Box { selector, .. } => format!("{selector} box"),
            Check::Pixel { x, y, .. } => format!("pixel {x},{y}"),
            Check::Ink {
                rect: [x, y, width, height],
                ..
            } => format!("{width}x{height} at {x},{y}"),
        }
    }
}

struct Expectation {
    /// A browser satisfies this check and Blitz does not, so the corpus asserts
    /// the divergence instead of the check. Written `@!`.
    defect: bool,
    check: Check,
}

struct Case {
    html: String,
    width: u32,
    height: u32,
    /// The declared checks were derived from the CSS rather than from a run.
    oracle: bool,
    /// Depends on the host's installed fonts, so it carries no golden image.
    host_fonts: bool,
    /// What upstream defect this case documents. Carries no golden image
    /// either: the frame is the wrong one until the defect is fixed.
    defect: Option<String>,
    checks: Vec<Expectation>,
}

fn color(token: &str) -> [u8; 4] {
    let digits = token.strip_prefix('#').expect("colour starts with '#'");
    assert_eq!(digits.len(), 8, "colour {token} is not #rrggbbaa");
    let mut rgba = [0u8; 4];
    for (channel, slot) in rgba.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&digits[channel * 2..channel * 2 + 2], 16)
            .unwrap_or_else(|_| panic!("colour {token} is not hexadecimal"));
    }
    rgba
}

fn number<T: std::str::FromStr>(token: Option<&&str>, line: &str) -> T {
    token
        .unwrap_or_else(|| panic!("missing operand in `{line}`"))
        .parse()
        .unwrap_or_else(|_| panic!("bad operand in `{line}`"))
}

/// Reads a case file: the leading HTML comment declares the expectations, and
/// everything after it is the document. Only `@`-prefixed lines are read, so
/// the rest of the comment is free for the reasoning behind the numbers —
/// which is the part that makes a golden a claim rather than a recording.
fn parse(name: &str, source: &str) -> Case {
    let (header, html) = source
        .split_once("-->")
        .unwrap_or_else(|| panic!("{name} has no expectation header"));
    let header = header
        .trim_start()
        .strip_prefix("<!--")
        .unwrap_or_else(|| panic!("{name} does not open with a comment"));

    let mut case = Case {
        html: html.trim_start().into(),
        width: 400,
        height: 200,
        oracle: false,
        host_fonts: false,
        defect: None,
        checks: Vec::new(),
    };
    for line in header
        .lines()
        .filter(|line| line.trim_start().starts_with('@'))
    {
        let directive = line.trim_start().trim_start_matches('@');
        let defect = directive.starts_with('!');
        let words: Vec<&str> = directive
            .trim_start_matches('!')
            .split_whitespace()
            .collect();
        let check = match words.first().copied() {
            Some("size") => {
                case.width = number(words.get(1), line);
                case.height = number(words.get(2), line);
                None
            }
            Some("oracle") => {
                case.oracle = true;
                None
            }
            Some("host-fonts") => {
                case.host_fonts = true;
                None
            }
            Some("defect") => {
                case.defect = Some(words[1..].join(" "));
                None
            }
            // Taken from the end, because a selector can contain spaces.
            Some("box") => {
                let split = words
                    .len()
                    .checked_sub(4)
                    .filter(|split| *split > 1)
                    .unwrap_or_else(|| panic!("missing operand in `{line}`"));
                Some(Check::Box {
                    selector: words[1..split].join(" "),
                    rect: std::array::from_fn(|component| {
                        let token = words.get(split + component);
                        (token.copied() != Some("-")).then(|| number(token, line))
                    }),
                })
            }
            Some("pixel") => Some(Check::Pixel {
                x: number(words.get(1), line),
                y: number(words.get(2), line),
                rgba: color(
                    words
                        .get(3)
                        .unwrap_or_else(|| panic!("missing colour in `{line}`")),
                ),
            }),
            Some("ink") => {
                let fraction: f64 = number(words.get(7), line);
                let comparison = *words
                    .get(6)
                    .filter(|word| ["<=", ">="].contains(word))
                    .unwrap_or_else(|| panic!("missing >= or <= in `{line}`"));
                Some(Check::Ink {
                    rect: [
                        number(words.get(1), line),
                        number(words.get(2), line),
                        number(words.get(3), line),
                        number(words.get(4), line),
                    ],
                    rgba: color(
                        words
                            .get(5)
                            .unwrap_or_else(|| panic!("missing colour in `{line}`")),
                    ),
                    at_least: (comparison == ">=").then_some(fraction),
                    at_most: (comparison == "<=").then_some(fraction),
                })
            }
            verb => panic!(
                "{name} declares an unknown directive `{}`",
                verb.unwrap_or_default()
            ),
        };
        assert!(
            !defect || check.is_some(),
            "{name} marks `{line}` as a defect, but only a check can diverge"
        );
        case.checks
            .extend(check.map(|check| Expectation { defect, check }));
    }
    // A case with only a golden image is a recording of current behaviour and
    // nothing more, which is the failure mode this corpus exists to avoid.
    assert!(
        !case.checks.is_empty(),
        "{name} declares nothing it must be true of"
    );
    // Both halves of a defect case have to be present, or it silently becomes
    // an ordinary case that happens to pass, or one that fails the gate.
    assert_eq!(
        case.defect.is_some(),
        case.checks.iter().any(|expectation| expectation.defect),
        "{name} declares `defect` without any `@!` check, or the other way round"
    );
    case
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("rgba8 pixel")
}

/// Describes how a check did not hold, or `None` if it did.
fn evaluate(
    check: &Check,
    dom: &BlitzDom,
    snapshot: LayoutSnapshot,
    pixels: &[u8],
    width: u32,
) -> Option<String> {
    match check {
        Check::Box { selector, rect } => {
            let matched = dom
                .query_selector_all(dom.document(), selector)
                .unwrap_or_else(|error| panic!("{selector} did not parse: {error:?}"));
            let [node] = matched[..] else {
                return Some(format!("{selector} matched {} elements", matched.len()));
            };
            let found = dom.bounding_rect(node, snapshot).expect("layout flushed");
            let found = [found.x, found.y, found.width, found.height];
            found
                .iter()
                .zip(rect)
                .any(|(found, expected)| {
                    expected.is_some_and(|expected| (found - expected).abs() > 0.01)
                })
                .then(|| format!("{selector} box is {found:?}, declared {rect:?}"))
        }
        Check::Pixel { x, y, rgba } => {
            let found = pixel(pixels, width, *x, *y);
            (found != *rgba)
                .then(|| format!("pixel {x},{y} is {}, declared {}", hex(found), hex(*rgba)))
        }
        Check::Ink {
            rect: [x, y, ink_width, height],
            rgba,
            at_least,
            at_most,
        } => {
            let matching = (*y..y + height)
                .flat_map(|row| (*x..x + ink_width).map(move |column| (column, row)))
                .filter(|(column, row)| pixel(pixels, width, *column, *row) == *rgba)
                .count();
            let coverage = matching as f64 / f64::from(ink_width * height);
            let short = at_least.is_some_and(|bound| coverage < bound);
            let over = at_most.is_some_and(|bound| coverage > bound);
            (short || over).then(|| {
                format!(
                    "{}x{} at {x},{y} is {:.4} {}, declared {}{:.4}",
                    ink_width,
                    height,
                    coverage,
                    hex(*rgba),
                    if short { ">= " } else { "<= " },
                    at_least.or(*at_most).unwrap_or_default()
                )
            })
        }
    }
}

/// Renders a case and reports every declared check it failed, plus every
/// divergence a `@!` check declared and found — which is the defect the case
/// documents, and is not a failure until it goes away.
fn check(case: &Case) -> (Vec<u8>, Vec<String>, Vec<String>) {
    let mut dom = document(&case.html, case.width, case.height);
    let snapshot = dom.flush_layout().expect("layout");
    let pixels = render(&mut dom, case.width, case.height);
    let mut failures = Vec::new();
    let mut divergences = Vec::new();

    for expectation in &case.checks {
        let outcome = evaluate(&expectation.check, &dom, snapshot, &pixels, case.width);
        match (expectation.defect, outcome) {
            (false, Some(failure)) => failures.push(failure),
            (true, Some(divergence)) => divergences.push(divergence),
            (true, None) => failures.push(format!(
                "{} now holds; the defect this case documents looks fixed — \
                 drop the `!` and close the gap",
                expectation.check.label()
            )),
            (false, None) => {}
        }
    }
    (pixels, failures, divergences)
}

fn hex(rgba: [u8; 4]) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        rgba[0], rgba[1], rgba[2], rgba[3]
    )
}

fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut reader = png::Decoder::new(file)
        .read_info()
        .unwrap_or_else(|error| panic!("{} is not a PNG: {error}", path.display()));
    let mut pixels = vec![0; reader.output_buffer_size().expect("golden size")];
    let info = reader.next_frame(&mut pixels).expect("golden frame");
    pixels.truncate(info.buffer_size());
    Some((info.width, info.height, pixels))
}

fn encode(path: &Path, pixels: &[u8], width: u32, height: u32) {
    let file = std::fs::File::create(path)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(pixels)
        .expect("png data");
}

/// Magenta where the two frames disagree, and a dimmed copy of the golden
/// everywhere else, so a CI artifact shows *where* a case moved.
fn difference(golden: &[u8], actual: &[u8]) -> Vec<u8> {
    golden
        .as_chunks::<4>()
        .0
        .iter()
        .zip(actual.as_chunks::<4>().0)
        .flat_map(|(golden, actual)| {
            if golden == actual {
                let grey = 128 + golden[..3].iter().map(|c| u32::from(*c) / 12).sum::<u32>() as u8;
                [grey, grey, grey, 255]
            } else {
                [255, 0, 255, 255]
            }
        })
        .collect()
}

#[test]
fn layout_conformance_corpus() {
    let record = std::env::var_os("BLITSEN_RECORD_CONFORMANCE").is_some();
    let cases_directory = corpus().join("cases");
    let goldens = corpus().join("goldens");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&cases_directory)
        .unwrap_or_else(|error| panic!("no corpus at {}: {error}", cases_directory.display()))
        .map(|entry| entry.expect("corpus entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "html")
        })
        .collect();
    sources.sort();
    assert!(!sources.is_empty(), "the corpus is empty");

    let fingerprint = raster_fingerprint();
    let recorded: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(goldens.join("rasterizer.json")).unwrap_or_default(),
    )
    .unwrap_or_default();
    let portable = recorded
        .get("fingerprint")
        .is_some_and(|it| *it == fingerprint);

    let divergence = divergence_directory();
    let _ = std::fs::remove_dir_all(&divergence);
    let mut failures = Vec::new();
    let mut documented = Vec::new();
    let mut frames = Vec::new();
    let mut oracles = 0;

    for source in &sources {
        let name = source
            .file_stem()
            .expect("case name")
            .to_string_lossy()
            .into_owned();
        let case = parse(
            &name,
            &std::fs::read_to_string(source).expect("case source"),
        );
        let (pixels, mut reported, divergences) = check(&case);
        oracles += usize::from(case.oracle);
        for failure in reported.drain(..) {
            failures.push(format!("{name}: {failure}"));
        }
        if let Some(defect) = &case.defect {
            documented.push(format!("{name} ({defect})"));
            for divergence in divergences {
                documented.push(format!("    {divergence}"));
            }
        }

        // A defect case's frame is the wrong one by construction, so it gets no
        // golden; the divergence it declares is the whole of its gate.
        if case.host_fonts || case.defect.is_some() {
            continue;
        }
        let golden = goldens.join(format!("{name}.png"));
        if record {
            frames.push((golden, pixels, case.width, case.height));
            continue;
        }
        let Some((width, height, expected)) = decode(&golden) else {
            failures.push(format!(
                "{name}: no golden image; record one with \
                 `BLITSEN_RECORD_CONFORMANCE=1 cargo test -p blitsen-blitz --test conformance`"
            ));
            continue;
        };
        if !portable || (width, height, &expected) == (case.width, case.height, &pixels) {
            continue;
        }
        let differing = expected
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks::<4>().0)
            .filter(|(golden, actual)| golden != actual)
            .count();
        failures.push(format!(
            "{name}: {differing} pixels differ from the golden image"
        ));
        std::fs::create_dir_all(&divergence).expect("divergence directory");
        encode(
            &divergence.join(format!("{name}.actual.png")),
            &pixels,
            case.width,
            case.height,
        );
        encode(
            &divergence.join(format!("{name}.golden.png")),
            &expected,
            width,
            height,
        );
        if (width, height) == (case.width, case.height) {
            encode(
                &divergence.join(format!("{name}.diff.png")),
                &difference(&expected, &pixels),
                case.width,
                case.height,
            );
        }
    }

    if record {
        assert!(
            failures.is_empty(),
            "refusing to record goldens while the corpus fails its own expectations:\n  {}",
            failures.join("\n  ")
        );
        std::fs::create_dir_all(&goldens).expect("goldens directory");
        for (path, pixels, width, height) in &frames {
            encode(path, pixels, *width, *height);
        }
        let manifest = serde_json::json!({ "fingerprint": fingerprint });
        let manifest = serde_json::to_string_pretty(&manifest).expect("manifest");
        std::fs::write(goldens.join("rasterizer.json"), format!("{manifest}\n")).expect("manifest");
        println!(
            "recorded {} golden images to {}",
            frames.len(),
            goldens.display()
        );
        return;
    }

    assert!(
        failures.is_empty(),
        "layout conformance corpus failed:\n  {}\nimages in {}",
        failures.join("\n  "),
        divergence.display()
    );
    println!(
        "Layout conformance verified: {} cases, {oracles} of them correctness oracles; \
         golden images {}.",
        sources.len(),
        if portable {
            "compared"
        } else {
            "not compared — this host's raster fingerprint differs from the goldens'"
        }
    );
    if !documented.is_empty() {
        println!(
            "Known Blitz defects, still diverging as declared:\n  {}",
            documented.join("\n  ")
        );
    }
}
