//! Free-list / out-of-heap allocation validator — CRASH_DEBUG_NOTES.md #5.
//!
//! Wraps [`embedded_alloc::Heap`] with **zero padding** (layout-identical to the
//! bare allocator, so it does not perturb the layout-sensitive #5 bug) and a
//! single check: every non-null pointer returned by `alloc` must lie fully inside
//! the heap region. `embedded_alloc` (a linked-list allocator) can only return an
//! out-of-heap pointer if its **free-list is corrupted** — the prime suspect for
//! crash #5's wild `Vec` pointers (e.g. the `__aeabi_memset4` HardFault that
//! faulted at GB address `0x8000` while zeroing a freshly-allocated block).
//!
//! On a bad allocation it records a clean Panic (`guarded_:NN`) naming the wild
//! pointer, instead of the corruption surfacing later as a wild memset/HardFault.
//!
//! Enabled by the `heap-guard` cargo feature (the `#[global_allocator]` swap is
//! in `main.rs`).
//!
//! (Earlier this file was a redzone "electric fence" wrapper; the redzones never
//! fired across the #5 hunt, and a free-list *pointer* smash is not a contiguous
//! overrun, so it was replaced with this direct out-of-heap check. See git
//! history for the redzone version.)

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_alloc::Heap;

const HEAP_GUARD_SITE_NULL_ALLOC: u32 = 0xA110_0001;
const HEAP_GUARD_SITE_OUT_OF_HEAP: u32 = 0xA110_0002;

pub struct GuardedHeap {
    inner: Heap,
    start: AtomicUsize,
    end: AtomicUsize,
}

impl GuardedHeap {
    pub const fn empty() -> Self {
        Self {
            inner: Heap::empty(),
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
        }
    }

    /// # Safety
    /// Same contract as [`embedded_alloc::Heap::init`].
    pub unsafe fn init(&self, start_addr: usize, size: usize) {
        self.start.store(start_addr, Ordering::Relaxed);
        self.end
            .store(start_addr.wrapping_add(size), Ordering::Relaxed);
        unsafe { self.inner.init(start_addr, size) }
    }
}

unsafe impl GlobalAlloc for GuardedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { self.inner.alloc(layout) };
        if p.is_null() {
            let start = self.start.load(Ordering::Relaxed);
            let end = self.end.load(Ordering::Relaxed);
            crate::crash::handler::rustyboy_heap_alloc_guard(
                HEAP_GUARD_SITE_NULL_ALLOC,
                0,
                layout.size() as u32,
                layout.align() as u32,
                start as u32,
                end as u32,
            );
        }
        if !p.is_null() {
            let a = p as usize;
            let start = self.start.load(Ordering::Relaxed);
            let end = self.end.load(Ordering::Relaxed);
            // A linked-list allocator can only return out-of-heap memory if its
            // free-list was corrupted. Catch it red-handed with the wild pointer.
            let out_of_heap = a < start || a.checked_add(layout.size()).map_or(true, |hi| hi > end);
            if out_of_heap {
                defmt::error!(
                    "free-list: alloc OUT-OF-HEAP ptr={=usize:#010x} size={=usize} align={=usize} heap=[{=usize:#010x},{=usize:#010x})",
                    a,
                    layout.size(),
                    layout.align(),
                    start,
                    end,
                );
                crate::crash::handler::rustyboy_heap_alloc_guard(
                    HEAP_GUARD_SITE_OUT_OF_HEAP,
                    a as u32,
                    layout.size() as u32,
                    layout.align() as u32,
                    start as u32,
                    end as u32,
                );
            }
        }
        p
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.inner.dealloc(ptr, layout) }
    }
}
