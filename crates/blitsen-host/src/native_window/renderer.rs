//! Renderer selection: the window renderer that is safe for each target.

/// The window renderer safe for this target.
///
/// Vello's Metal compute path has caused full-session GPU resets on Intel Macs
/// (#229), while the API 32/33 Android AVD's lavapipe adapter exposes no usable
/// storage buffer and Vello panics during device creation (#151). Those targets
/// use the CPU rasterizer and a software framebuffer. Android retains an
/// explicit GPU qualification build; it is never selected automatically.
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "android", not(feature = "android-vello-gpu"))
))]
pub type NativeWindowRenderer = anyrender_vello_cpu::VelloCpuWindowRenderer;

/// The window renderer safe for this target.
#[cfg(not(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "android", not(feature = "android-vello-gpu"))
)))]
pub type NativeWindowRenderer = anyrender_vello::VelloWindowRenderer;

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-cpu window-backend=softbuffer \
         reason=Intel-macOS-Metal-safety-fallback"
    );
    NativeWindowRenderer::new()
}

#[cfg(all(target_os = "android", not(feature = "android-vello-gpu")))]
pub(crate) fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-cpu window-backend=softbuffer \
         reason=Android-safe-default gpu-qualification-feature=android-vello-gpu"
    );
    NativeWindowRenderer::new()
}

#[cfg(all(target_os = "android", feature = "android-vello-gpu"))]
pub(crate) fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!(
        "blitsen: renderer=vello-gpu backend=wgpu \
         qualification=Android-mobile-GPU feature=android-vello-gpu"
    );
    NativeWindowRenderer::new()
}

#[cfg(not(any(
    target_os = "android",
    all(target_os = "macos", target_arch = "x86_64")
)))]
pub(crate) fn native_window_renderer() -> NativeWindowRenderer {
    eprintln!("blitsen: renderer=vello-gpu backend=wgpu");
    NativeWindowRenderer::new()
}
