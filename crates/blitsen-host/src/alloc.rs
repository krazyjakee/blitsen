//! Optional per-frame allocation audit.
//!
//! Off by default: counting is a global-allocator wrapper, and a shipped addon
//! should not pay two atomics per allocation for a number nobody reads. Build
//! with `--features alloc-audit` to answer "how many heap allocations does one
//! frame cost", then read the deltas the replay report carries.

use serde::Serialize;

/// Allocator activity observed over some interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AllocationCounts {
    /// Calls to `alloc` and `alloc_zeroed`.
    pub(crate) allocations: u64,
    /// Bytes requested by those calls.
    pub(crate) bytes: u64,
    /// Calls to `realloc`.
    pub(crate) reallocations: u64,
    /// Calls to `dealloc`.
    pub(crate) deallocations: u64,
}

impl AllocationCounts {
    /// Activity between an earlier snapshot and this one.
    pub(crate) fn since(self, earlier: Self) -> Self {
        Self {
            allocations: self.allocations.wrapping_sub(earlier.allocations),
            bytes: self.bytes.wrapping_sub(earlier.bytes),
            reallocations: self.reallocations.wrapping_sub(earlier.reallocations),
            deallocations: self.deallocations.wrapping_sub(earlier.deallocations),
        }
    }
}

/// A per-stage allocation counter for the frame pipeline, when audited.
pub(crate) fn stage_counter() -> Option<fn() -> u64> {
    #[cfg(feature = "alloc-audit")]
    {
        Some(|| audit::snapshot().allocations)
    }
    #[cfg(not(feature = "alloc-audit"))]
    {
        None
    }
}

/// Reads the counters, or `None` when the audit is not compiled in.
pub(crate) fn snapshot() -> Option<AllocationCounts> {
    #[cfg(feature = "alloc-audit")]
    {
        Some(audit::snapshot())
    }
    #[cfg(not(feature = "alloc-audit"))]
    {
        None
    }
}

#[cfg(feature = "alloc-audit")]
mod audit {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::AllocationCounts;

    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);

    pub(super) fn snapshot() -> AllocationCounts {
        AllocationCounts {
            allocations: ALLOCATIONS.load(Ordering::Relaxed),
            bytes: BYTES.load(Ordering::Relaxed),
            reallocations: REALLOCATIONS.load(Ordering::Relaxed),
            deallocations: DEALLOCATIONS.load(Ordering::Relaxed),
        }
    }

    /// Counts every Rust allocation in the addon and forwards to the system
    /// allocator. JavaScript-side allocation belongs to the host and is not
    /// visible here.
    struct CountingAllocator;

    // SAFETY: every method forwards its arguments unchanged to `System`, which
    // upholds the `GlobalAlloc` contract; counting has no effect on the returned
    // pointers.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(
                new_size.saturating_sub(layout.size()) as u64,
                Ordering::Relaxed,
            );
            unsafe { System.realloc(pointer, layout, new_size) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}
