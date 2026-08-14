//! Low-memory process defaults applied before any runtime thread starts.
//!
//! wgpu otherwise initializes every backend it was compiled with, and Vulkan's
//! loader otherwise opens every installed ICD. On a developer workstation that
//! can map LLVM, Mesa and several hardware drivers for one window which will use
//! exactly one of them. User settings always win; ambiguous hardware keeps full
//! discovery rather than risking a window that cannot open.

#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::path::Path;

pub(crate) fn apply() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    apply_glibc_allocator_default();
    #[cfg(target_os = "linux")]
    apply_linux_gpu_defaults();
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn apply_glibc_allocator_default() {
    if allocator_is_configured(
        std::env::var_os("MALLOC_ARENA_MAX").as_deref(),
        std::env::var_os("GLIBC_TUNABLES").as_deref(),
    ) {
        return;
    }
    // SAFETY: this is the first operation in `main`, before Blitsen creates any
    // threads. M_ARENA_MAX is a process-wide allocator setting and `2` is a
    // valid positive arena limit. A libc which refuses it leaves its default in
    // force; there is no useful recovery action for that case.
    let _ = unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn allocator_is_configured(arena_max: Option<&OsStr>, tunables: Option<&OsStr>) -> bool {
    arena_max.is_some_and(|value| !value.is_empty())
        || tunables.is_some_and(|value| {
            value
                .to_string_lossy()
                .split(':')
                .any(|item| item.starts_with("glibc.malloc.arena_max="))
        })
}

#[cfg(target_os = "linux")]
fn apply_linux_gpu_defaults() {
    let configured_backend = std::env::var_os("WGPU_BACKEND");
    let backend = configured_backend.is_none().then_some("vulkan");
    let vulkan_enabled = configured_backend.as_deref().is_none_or(|value| {
        value.to_str().is_some_and(|value| {
            value
                .split(',')
                .any(|backend| backend.trim().eq_ignore_ascii_case("vulkan"))
        })
    });
    let driver_override = [
        "VK_LOADER_DRIVERS_SELECT",
        "VK_LOADER_DRIVERS_DISABLE",
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "VK_ADD_DRIVER_FILES",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some());
    let driver_filter = (vulkan_enabled && !driver_override)
        .then(|| drm_render_drivers(Path::new("/sys/class/drm")))
        .and_then(|drivers| driver_filter(&drivers));

    // SAFETY: `apply` is the first operation in `main`, before Blitsen starts a
    // runtime or driver thread. No other thread can concurrently read or write
    // the process environment. Existing values were inspected above and are
    // never replaced.
    unsafe {
        if let Some(value) = backend {
            std::env::set_var("WGPU_BACKEND", value);
        }
        if let Some(value) = driver_filter {
            std::env::set_var("VK_LOADER_DRIVERS_SELECT", value);
        }
    }
}

#[cfg(target_os = "linux")]
fn drm_render_drivers(root: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
        .filter_map(|entry| std::fs::read_link(entry.path().join("device/driver")).ok())
        .filter_map(|driver| {
            driver
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .map(|driver| driver.to_ascii_lowercase())
        .collect()
}

#[cfg(target_os = "linux")]
fn driver_filter(drivers: &BTreeSet<String>) -> Option<&'static str> {
    if drivers.len() != 1 {
        return None;
    }
    match drivers.first()?.as_str() {
        "nvidia" => Some("*nvidia*"),
        "nouveau" => Some("*nouveau*"),
        "amdgpu" => Some("*radeon*,*amd*"),
        "i915" | "xe" => Some("*intel*"),
        "virtio_gpu" => Some("*virtio*"),
        "asahi" => Some("*asahi*"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use std::collections::BTreeSet;
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    use std::ffi::OsStr;

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    use super::allocator_is_configured;
    #[cfg(target_os = "linux")]
    use super::driver_filter;

    #[cfg(target_os = "linux")]
    fn drivers(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn one_known_drm_driver_selects_only_its_icd() {
        assert_eq!(driver_filter(&drivers(&["nvidia"])), Some("*nvidia*"));
        assert_eq!(driver_filter(&drivers(&["amdgpu"])), Some("*radeon*,*amd*"));
        assert_eq!(driver_filter(&drivers(&["xe"])), Some("*intel*"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn hybrid_and_unknown_graphics_keep_full_driver_discovery() {
        assert_eq!(driver_filter(&drivers(&[])), None);
        assert_eq!(driver_filter(&drivers(&["nvidia", "i915"])), None);
        assert_eq!(driver_filter(&drivers(&["panfrost"])), None);
    }

    #[test]
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    fn explicit_glibc_allocator_settings_win() {
        assert!(!allocator_is_configured(None, None));
        assert!(allocator_is_configured(Some(OsStr::new("8")), None));
        assert!(allocator_is_configured(
            None,
            Some(OsStr::new(
                "glibc.malloc.trim_threshold=1:glibc.malloc.arena_max=4"
            ))
        ));
    }
}
