//! The one place an Android artifact can say anything.
//!
//! A desktop failure is a line on stderr and a non-zero exit code, and the shell
//! that typed the command is there to read both. Neither exists here: an APK is
//! launched by the system, its stdout and stderr go nowhere a user or a test can
//! reach, and there is no exit code because nothing waited for one. Logcat is
//! what replaces them, so a failure that is not written here is a blank screen
//! and nothing else — which is the diagnostic issue #143 would otherwise be
//! handed.
//!
//! `__android_log_write` rather than a logging framework: `android-activity`
//! already calls that exact symbol to report a panic out of `android_main`, so
//! this adds no library to link and no initialisation that could itself fail
//! before the first message. What it does not do is route the `log` and
//! `tracing` records the engine stack emits — that is a logger installation,
//! it belongs with the work that runs an APK and reads its output, and this
//! module is deliberately smaller than it.

/// The tag `adb logcat -s Blitsen:V` filters on.
#[cfg(target_os = "android")]
const TAG: &std::ffi::CStr = c"Blitsen";

/// The bytes `__android_log_write` will accept for a message.
///
/// A C string, so an interior NUL truncates it at that byte and
/// [`CString::new`](std::ffi::CString::new) refuses to build one at all. That is
/// not hypothetical: the messages this module carries are engine errors, an
/// engine error carries a JavaScript string, and a JavaScript string is
/// arbitrary UTF-16 in which `\0` is an ordinary character. Losing the whole
/// diagnostic to one byte inside it would lose it exactly when an application is
/// misbehaving enough to have produced it.
///
/// Replaced with `\u{fffd}` rather than dropped, because a message with a
/// character removed reads as if that is what the runtime said.
pub fn line(message: &str) -> std::ffi::CString {
    let cleaned = if message.contains('\0') {
        message.replace('\0', "\u{fffd}")
    } else {
        message.to_owned()
    };
    std::ffi::CString::new(cleaned).expect("every NUL was replaced above")
}

/// Writes one message to logcat at the given priority.
#[cfg(target_os = "android")]
fn write(priority: ndk_sys::android_LogPriority, message: &str) {
    let line = line(message);
    // SAFETY: `TAG` and `line` are NUL-terminated C strings that outlive the
    // call, which is what `__android_log_write` reads and the only requirement
    // it has. It is documented safe to call from any thread and at any point
    // after process start, including before the activity exists.
    unsafe {
        ndk_sys::__android_log_write(
            priority.0 as std::os::raw::c_int,
            TAG.as_ptr(),
            line.as_ptr(),
        );
    }
}

/// Reports something that went right, at a priority `adb logcat` shows by
/// default.
#[cfg(target_os = "android")]
pub fn info(message: &str) {
    write(ndk_sys::android_LogPriority::ANDROID_LOG_INFO, message);
}

/// Reports something that went wrong, and was not a panic.
///
/// A panic out of `android_main` is already logged by `android-activity`, which
/// catches it so it can finish the Activity gracefully. This is for the other
/// kind of failure — an `Err` returned by the session, which on desktop would
/// have been `eprintln!` and a non-zero exit code.
#[cfg(target_os = "android")]
pub fn error(message: &str) {
    write(ndk_sys::android_LogPriority::ANDROID_LOG_ERROR, message);
}

#[cfg(test)]
mod tests {
    use super::line;

    #[test]
    fn a_message_survives_the_nul_a_javascript_string_is_allowed_to_contain() {
        assert_eq!(line("could not start").to_bytes(), b"could not start");
        // The failure this guards against: `CString::new` refuses an interior
        // NUL, so the unhandled case loses the whole diagnostic rather than one
        // byte of it.
        assert_eq!(
            line("could not\0start").to_str().unwrap(),
            "could not\u{fffd}start"
        );
        assert_eq!(line("\0").to_str().unwrap(), "\u{fffd}");
        assert_eq!(line("").to_bytes(), b"");
        // Nothing else is rewritten, including the multi-byte characters an
        // error message from a document is entitled to carry.
        assert_eq!(
            line("no ‘index.html’ 🦌").to_str().unwrap(),
            "no ‘index.html’ 🦌"
        );
    }
}
