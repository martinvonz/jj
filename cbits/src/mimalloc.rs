// Copyright 2024 The Jujutsu Authors
// Copyright 2019 Octavian Oncescu
//
// Taken from mimalloc_rust library. mimalloc_rust contains the following
// license:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! A module for using the **[mimalloc](https://github.com/microsoft/mimalloc)**
//! memory allocator in Rust programs. mimalloc is a small, easy-to-understand,
//! scalable, cache-and-thread friendly memory allocator. It is written in C,
//! has no external dependencies, and is linked and exists entirely inside of
//! this crate.
//!
//! By design, this module is nearly opaque. The only supported APIs are using
//! [`MiMalloc`] with the [`#[global_allocator]` attribute][module@core::alloc],
//! and some tools for diagnostics. In the future this may expand to features
//! like dedicated heaps or arenas, if needed.
//!
//! ## Motivation
//!
//! This is effectively a soft fork of the [mimalloc](https://docs.rs/mimalloc)
//! and [mimalloc-sys](https://docs.rs/mimalloc-sys) crates, merging them into
//! one module for our own needs with some extra cherries on top. There are also
//! an array of third party crates providing mimalloc support.
//!
//! The motivation for our own crates include, but are not limited to:
//!
//! - Usage of mimalloc 2.x, while many upstream crates use seem to still use
//!   the 1.x series,
//! - Reduce "crate bloat", as we have no need for separate `foo`/`foo-sys`
//!   designs,
//! - Have a space for _other_ C code in the future,
//! - Have better control over the exported API for deeper integration.
//!
//! Please read the top-level documentation for the [jj_cbits](../index.html)
//! crate for more information.
//!
//! In the future, it is possible these changes may be integrated back into a
//! more general, widely-usable crate.
//!
//! ## Basic usage
//!
//! ```rust,ignore
//! #[global_allocator]
//! static ALLOC: jj_cbits::mimalloc::MiMalloc = jj_cbits::mimalloc::MiMalloc;
//! ```

use core::alloc::GlobalAlloc;
use core::ffi::{c_char, c_ulonglong, c_void, CStr};

// Full set of external C APIs.
//
// NOTE: These should under no circumstances ever be made `pub`.
extern "C" {
    /// Allocate a block of memory of at least `size` bytes, aligned to the
    /// given alignment, that is completely zeroed out.
    fn mi_zalloc_aligned(size: usize, alignment: usize) -> *mut c_void;

    /// Allocate a block of memory of at least `size` bytes, aligned to the
    /// given alignment. The initial contents of the memory is unspecified, and
    /// accessing it before initialization should be considered undefined
    /// behavior.
    fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void;

    /// Reallocate a block of memory to a new size.
    fn mi_realloc_aligned(p: *mut c_void, newsize: usize, alignment: usize) -> *mut c_void;

    /// Free any previously allocated memory.
    fn mi_free(p: *mut c_void);

    /// Merge all thread-local statistics with the main statistics buffer, and
    /// reset.
    fn mi_stats_merge();

    /// Reset the internal statistics counters.
    fn mi_stats_reset();

    /// Print global mimalloc statistics to a given output. The output is
    /// currently fixed to `stderr`; both the first and second arguments must be
    /// the NULL pointer.
    fn mi_stats_print_out(out: unsafe extern "C" fn(*const c_char, *mut c_void), ctx: *mut c_void);

    /// Register a "deferred free" function which will be called
    /// deterministically over the course of the program.
    fn mi_register_deferred_free(
        f: unsafe extern "C" fn(bool, c_ulonglong, *mut c_void),
        ctx: *mut c_void,
    );
}

/// Provide global mimalloc heap statistics to a user-provided callback.
///
/// The provided callback function will be called multiple times in sequence,
/// each time with a single argument, which is a single null-terminated line,
/// represented as a [CStr]. Collectively, these lines will represent a summary
/// of the global heap statistics for the entire program, meant to be written to
/// the terminal or a log file.
///
/// Therefore, the simplest way to use this function is to simply provide a
/// closure that prints the given log messages to `stderr` immediately:
///
/// ```rust,ignore
/// eprintln!("========================================");
/// eprintln!("mimalloc memory allocation statistics:\n");
/// jj_cbits::mimalloc::stats_print(&|l| {
///   eprint!("{}", l.to_string_lossy());
/// });
/// ```
///
/// Note that this merges all thread-local statistics into the main statistics
/// summary before printing to `stderr`, so while it will give a global summary
/// of the heap, it may cause some performance overhead while thread-local
/// buffers are being flushed and merged.
pub fn stats_print<F: Fn(&CStr)>(f: &'static F) {
    unsafe {
        unsafe extern "C" fn wrapper<F: Fn(&CStr)>(value: *const c_char, ctx: *mut c_void) {
            (*(ctx as *const F))(CStr::from_ptr(value));
        }
        mi_stats_merge();
        mi_stats_print_out(wrapper::<F>, f as *const F as *mut c_void)
    }
}

/// Reset heap statistics counters and histograms.
///
/// Primarily useful to clear out any existing statistics, so that a subsequent
/// call to `mimalloc_stats_print` will only show statistics since the last
/// reset.
///
/// This should also reset and merge all thread-local statistics, too.
pub fn stats_reset() {
    unsafe {
        mi_stats_reset();
    }
}

/// Register a "deferred free" function, which will be called by the memory
/// allocator after some (deterministic) number of calls to
/// [`dealloc`](core::alloc::GlobalAlloc::dealloc) in the heap.
///
/// Typically, the callback function is provided as a simple closure with static
/// lifetime, as it may be called at any point in the program's lifetime. The
/// result of the closure is ignored and has no meaning.
///
/// The provided closure will be invoked at an unspecified future point with the
/// following arguments:
///
/// * `force`, type `bool`: If `true`, the deferred free function should free
///   any memory it has allocated, or that may be possible to free to reduce
///   heap pressure.
/// * `count`, type `c_ulonglong`: A monotonically increasing "heartbeat
///   counter." May be assigned any semantic meaning to your program that you
///   desire. This counter MUST NOT be assumed to have any relation to the
///   structure of the heap, in any way.
///
/// These two parameters are completely independent from each other; that is,
/// any combination of `force` and `count` may be provided to the callback and
/// should not be assumed to influence each other in any meaningful way.
///
/// Note that this function is called _deterministically_ based on heap
/// allocations. Therefore, assuming the program itself exhibits deterministic
/// allocation behavior the resulting deferred free callback will also be called
/// deterministically over the program's lifetime. The number of allocations
/// between invocations is unspecified.
///
/// Despite the name, this registered callback does not need to free any extra
/// memory in any way, and can be used purely as a "heartbeat" mechanism to
/// implement other functionality, such as periodic state logging or timeouts
/// that are not tied to the wall clock.
///
/// There may be only a single deferred free function registered at any given
/// time. If this function is called multiple times, the last registered
/// function will be used.
///
/// Reference:
///
/// - Section 2.3 _The Local Free List_; Leijen 2019, "[Mimalloc: Free List
/// Sharding in Action][mimalloc-pdf]", MSR-TR 2019-18.
///
/// [mimalloc-pdf]:
///     https://www.microsoft.com/en-us/research/uploads/prod/2019/06/mimalloc-tr-v1.pdf
pub fn register_deferred_free<F: Fn(bool, c_ulonglong)>(f: &'static F) {
    unsafe {
        unsafe extern "C" fn wrapper<F: Fn(bool, c_ulonglong)>(
            force: bool,
            count: c_ulonglong,
            ctx: *mut c_void,
        ) {
            (*(ctx as *const F))(force, count);
        }
        mi_register_deferred_free(wrapper::<F>, f as *const F as *mut c_void)
    }
}

/// Global memory allocator, based on the mimalloc library.
///
/// ## Usage
///
/// Inside of the `main.rs` for any binary:
///
/// ```rust,ignore
/// #[global_allocator]
/// static ALLOC: jj_cbits::mimalloc::MiMalloc = jj_cbits::mimalloc::MiMalloc;
/// ```
pub struct MiMalloc;

unsafe impl GlobalAlloc for MiMalloc {
    #[inline]
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        mi_malloc_aligned(layout.size(), layout.align()) as *mut u8
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: core::alloc::Layout) -> *mut u8 {
        mi_zalloc_aligned(layout.size(), layout.align()) as *mut u8
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        mi_free(ptr as *mut c_void)
    }

    #[inline]
    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: core::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        mi_realloc_aligned(ptr as *mut c_void, new_size, layout.align()) as *mut u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_free_malloc() {
        let ptr = unsafe { mi_malloc_aligned(8, 8) } as *mut u8;
        unsafe { mi_free(ptr as *mut c_void) };
    }

    #[test]
    fn ok_free_zalloc() {
        let ptr = unsafe { mi_zalloc_aligned(8, 8) } as *mut u8;
        unsafe { mi_free(ptr as *mut c_void) };
    }

    #[test]
    fn ok_free_realloc() {
        let ptr = unsafe { mi_malloc_aligned(8, 8) } as *mut u8;
        let ptr = unsafe { mi_realloc_aligned(ptr as *mut c_void, 8, 8) } as *mut u8;
        unsafe { mi_free(ptr as *mut c_void) };
    }

    #[test]
    fn ok_usable_size() {
        extern "C" {
            // not used elsewhere, so scope it here to the tests to avoid
            // spurious dead_code warnings
            fn mi_usable_size(p: *const c_void) -> usize;
        }

        let ptr = unsafe { mi_malloc_aligned(32, 64) } as *mut u8;
        let usable_size = unsafe { mi_usable_size(ptr as *mut c_void) };
        assert!(
            usable_size >= 32,
            "usable_size should at least equal to the allocated size"
        );
    }
}
