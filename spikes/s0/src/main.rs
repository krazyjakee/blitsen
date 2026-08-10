#[cfg(feature = "jsc")]
use std::{ffi::CString, ptr};

#[cfg(feature = "blitz")]
use anyrender::{PaintScene as _, render_to_buffer};
#[cfg(feature = "blitz")]
use anyrender_vello_cpu::VelloCpuImageRenderer;
#[cfg(feature = "blitz")]
use blitz_dom::{DocumentConfig, util::Color};
#[cfg(feature = "blitz")]
use blitz_html::HtmlDocument;
#[cfg(feature = "blitz")]
use blitz_paint::paint_scene;
#[cfg(feature = "blitz")]
use blitz_traits::shell::{ColorScheme, Viewport};
#[cfg(feature = "blitz")]
use peniko::{Fill, kurbo::Rect};

#[cfg(feature = "jsc")]
type JSClassRef = *const core::ffi::c_void;
#[cfg(feature = "jsc")]
type JSContextRef = *const core::ffi::c_void;
#[cfg(feature = "jsc")]
type JSGlobalContextRef = *const core::ffi::c_void;
#[cfg(feature = "jsc")]
type JSStringRef = *const core::ffi::c_void;
#[cfg(feature = "jsc")]
type JSValueRef = *const core::ffi::c_void;

#[cfg(feature = "jsc")]
unsafe extern "C" {
    fn JSGlobalContextCreate(global_object_class: JSClassRef) -> JSGlobalContextRef;
    fn JSStringCreateWithUTF8CString(string: *const core::ffi::c_char) -> JSStringRef;
    fn JSStringRelease(string: JSStringRef);
    fn JSEvaluateScript(
        context: JSContextRef,
        script: JSStringRef,
        this_object: *const core::ffi::c_void,
        source_url: JSStringRef,
        starting_line_number: i32,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSValueToNumber(context: JSContextRef, value: JSValueRef, exception: *mut JSValueRef)
    -> f64;
}

#[cfg(feature = "jsc")]
fn evaluate_javascript() -> f64 {
    let source = CString::new("6 * 7").unwrap();
    let mut exception = ptr::null();
    unsafe {
        let context = JSGlobalContextCreate(ptr::null());
        assert!(!context.is_null());
        let script = JSStringCreateWithUTF8CString(source.as_ptr());
        let value = JSEvaluateScript(context, script, ptr::null(), ptr::null(), 1, &mut exception);
        assert!(exception.is_null());
        let number = JSValueToNumber(context, value, &mut exception);
        assert!(exception.is_null());
        JSStringRelease(script);
        // Blitsen owns one process-lifetime context. Bun's JSC build asserts while tearing down
        // its atom table through the public release call when embedded without Bun's host glue.
        // Keeping the global alive matches the intended runtime lifetime and lets the OS reclaim
        // it at process exit.
        number
    }
}

#[cfg(feature = "blitz")]
fn render_html() -> (u32, u64) {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 180;
    let mut document = HtmlDocument::from_html(
        "<style>body{margin:0;display:grid;place-items:center;background:#112233;color:white}</style><h1>Blitsen</h1>",
        DocumentConfig {
            viewport: Some(Viewport::new(WIDTH, HEIGHT, 1.0, ColorScheme::Light)),
            ..Default::default()
        },
    );
    document.as_mut().resolve(0.0);

    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            scene.fill(
                Fill::NonZero,
                Default::default(),
                Color::WHITE,
                Default::default(),
                &Rect::new(0.0, 0.0, WIDTH.into(), HEIGHT.into()),
            );
            paint_scene(scene, document.as_mut(), 1.0, WIDTH, HEIGHT, 0, 0);
        },
        WIDTH,
        HEIGHT,
    );
    let checksum = pixels.iter().map(|byte| u64::from(*byte)).sum();
    (pixels.len() as u32, checksum)
}

fn main() {
    #[cfg(feature = "jsc")]
    {
        let js_result = evaluate_javascript();
        assert_eq!(js_result, 42.0);
        print!("jsc={js_result}");
    }
    #[cfg(feature = "blitz")]
    {
        let (rendered_bytes, checksum) = render_html();
        assert_ne!(checksum, 0);
        print!(" rgba_bytes={rendered_bytes} checksum={checksum}");
    }
    println!();
}
